//! Reading `Print Format` rows and mapping them onto the engine's definition.

use ferris_core::{DocumentError, Error, Result, ValidationError, Value};
use ferris_document::{DocumentEngine, EngineContext, FilterOp, ListFilter, ListQuery};
use serde_json::json;

use crate::{
    FormatEngine, Margins, Orientation, PaperSize, PrintFormatDef, PrintSettings,
    http::{BUILTIN_FORMAT, host::PrintHost},
};

/// The layout `DocType` read to resolve a `Print Format`.
pub const PRINT_FORMAT: &str = "Print Format";
/// The Single that supplies install-wide fallbacks.
pub const PRINT_SETTINGS: &str = "Print Settings";

/// The `Print Format` rows for `doctype` (system read) plus the builtin entry.
///
/// # Errors
///
/// A storage error while listing the rows. A malformed row is reported as
/// `valid: false` instead of making the whole list unusable.
pub async fn available_formats(host: &PrintHost, doctype: &str) -> Result<Vec<Value>> {
    let settings = super::render::load_print_settings(host).await?;
    let docs = print_format_docs(host, doctype).await?;
    let mut out: Vec<Value> = docs
        .iter()
        .map(|doc| match format_from_doc(doc, doctype, settings) {
            Ok(format) => {
                let mut entry = summary(&format, "db", true, None);
                set_title(&mut entry, doc);
                entry
            }
            Err(error) => {
                let mut entry = summary(
                    &PrintFormatDef::new(doc_str(doc, "name"), doctype.to_string(), String::new()),
                    "db",
                    false,
                    Some(error.to_string()),
                );
                set_title(&mut entry, doc);
                entry.set("enabled".to_string(), Value::bool(format_is_enabled(doc)));
                let engine = doc_str(doc, "engine");
                entry.set(
                    "engine".to_string(),
                    Value::string(if engine.trim().is_empty() {
                        doc_str(doc, "print_format_type")
                    } else {
                        engine
                    }),
                );
                entry
            }
        })
        .collect();
    out.push(Value::from_json(json!({
        "name": BUILTIN_FORMAT,
        "format_name": BUILTIN_FORMAT,
        "source": "builtin",
        "engine": "typst",
        "enabled": true,
        "valid": true,
        "description": "Generic two-column table of the document's fields",
    })));
    Ok(out)
}

fn summary(format: &PrintFormatDef, source: &str, valid: bool, error: Option<String>) -> Value {
    let engine = match format.engine {
        FormatEngine::Typst => "typst",
        FormatEngine::Jinja => "jinja",
    };
    let mut entry = Value::from_json(json!({
        "name": format.name.clone(),
        "format_name": format.name.clone(),
        "source": source,
        "engine": engine,
        "enabled": format.enabled,
        "valid": valid,
        "description": format.description.clone(),
        "template_dir": format.template_dir.clone(),
        "parameters": serde_json::Value::Object(
            format.parameters.iter().map(|(k, v)| (k.clone(), v.to_json())).collect()
        ),
        "sample_data": format.sample_data.to_json(),
    }));
    if let Some(error) = error {
        entry.set("error".to_string(), Value::string(error));
    }
    entry
}

/// The human name is stored in `format_name`; the document's `name` is the id
/// the API uses to select the row.
fn set_title(entry: &mut Value, doc: &Value) {
    let title = doc_str(doc, "format_name");
    if !title.trim().is_empty() {
        entry.set("format_name".to_string(), Value::string(title));
    }
}

/// Resolve the layout to render: a named row, the first enabled row, or the
/// builtin fallback.
// The explicit branches document the three outcomes; `map_or_else` would hide
// the "no format configured -> builtin" rule that is the point of the fallback.
#[allow(clippy::option_if_let_else)]
pub async fn load_format(
    host: &PrintHost,
    doctype: &str,
    requested: Option<&str>,
    settings: PrintSettings,
) -> Result<PrintFormatDef> {
    if requested == Some(BUILTIN_FORMAT) {
        return Ok(builtin_format(doctype, settings));
    }
    let docs = print_format_docs(host, doctype).await?;
    let chosen = match requested {
        Some(name) => docs.iter().find(|doc| doc_str(doc, "name") == name),
        None => docs.iter().find(|doc| format_is_enabled(doc)),
    };
    match chosen {
        Some(doc) => format_from_doc(doc, doctype, settings),
        None => match requested {
            Some(name) => Err(Error::Document(DocumentError::NotFound {
                doctype: PRINT_FORMAT.to_string(),
                name: name.to_string(),
            })),
            // No layout configured is the normal state of a fresh DocType, not
            // an error: print the generic table instead of refusing.
            None => Ok(builtin_format(doctype, settings)),
        },
    }
}

/// Every `Print Format` row whose `doc_type` is `doctype`, newest first.
async fn print_format_docs(host: &PrintHost, doctype: &str) -> Result<Vec<Value>> {
    let meta = ferris_api::store::effective_meta(&host.api.pool, PRINT_FORMAT).await?;
    let mut ctx = EngineContext::new(PRINT_FORMAT.to_string(), "");
    ctx.meta = meta.as_ref();
    // A print layout is install-wide presentation data. The document being
    // printed carries the permission decision; requiring `Read` on
    // `Print Format` would hide every layout from the people who print.
    ctx.user = None;
    ctx.allow_anonymous = true;
    let query = ListQuery {
        filter_ops: vec![ListFilter {
            field: "doc_type".to_string(),
            op: FilterOp::Equals,
            value: Value::string(doctype),
            or: false,
        }],
        limit: Some(100),
        order_by: Some("modified desc".to_string()),
        ..Default::default()
    };
    let (docs, _) = DocumentEngine::new(host.api.pool.clone())
        .list(&ctx, &query)
        .await?;
    Ok(docs)
}

fn format_is_enabled(doc: &Value) -> bool {
    doc_bool(doc, "enabled", true) && !doc_bool(doc, "disabled", false)
}

/// Map a stored `Print Format` row onto the engine's definition.
fn format_from_doc(doc: &Value, doctype: &str, settings: PrintSettings) -> Result<PrintFormatDef> {
    let name = doc_str(doc, "name");
    if !format_is_enabled(doc) {
        return Err(unsupported(format!("print format `{name}` is disabled")));
    }
    let template = doc_str(doc, "template");
    if template.trim().is_empty() {
        return Err(unsupported(format!(
            "print format `{name}` has no template"
        )));
    }

    let engine = resolve_engine(doc, &template)?;
    let parameters = json_object(doc, "parameters")?;
    let sample_data = json_value(doc, "sample_data")?;
    let paper_size = match doc_str(doc, "paper_size").trim() {
        "" => settings.paper_size,
        raw => raw.parse::<PaperSize>()?,
    };
    let orientation = match doc_str(doc, "orientation").trim() {
        "" => settings.orientation,
        raw => raw.parse::<Orientation>()?,
    };
    let scale = match doc_float(doc, "scale") {
        Some(scale) if scale.is_finite() && scale > 0.0 => scale,
        Some(_) => {
            return Err(Error::Validation(ValidationError::Invalid(format!(
                "print format `{name}` has an invalid `scale`; expected a positive number"
            ))));
        }
        None => settings.scale,
    };
    let margins = Margins {
        top: doc_float(doc, "margin_top").unwrap_or(20.0),
        bottom: doc_float(doc, "margin_bottom").unwrap_or(20.0),
        left: doc_float(doc, "margin_left").unwrap_or(20.0),
        right: doc_float(doc, "margin_right").unwrap_or(20.0),
    };

    let mut format = PrintFormatDef::new(name, doctype.to_string(), template);
    format.description = doc_str(doc, "description");
    format.engine = engine;
    format.template_dir = optional_str(doc, "template_dir");
    format.parameters = parameters;
    format.sample_data = sample_data;
    format.paper_size = paper_size;
    format.orientation = orientation;
    format.scale = scale;
    format.margins = margins;
    Ok(format)
}

/// The engine of a row. `engine` is current; `print_format_type` is the
/// historical field and only `Server` (raw Typst) is still meaningful there.
fn resolve_engine(doc: &Value, template: &str) -> Result<FormatEngine> {
    let raw = doc_str(doc, "engine");
    if !raw.trim().is_empty() {
        return raw.parse();
    }
    let legacy = doc_str(doc, "print_format_type");
    if !legacy.trim().is_empty() {
        return legacy.parse();
    }
    // A row written before the field existed: recognize the syntax instead of
    // compiling a Jinja template as Typst (or the reverse).
    if template.contains("{{") || template.contains("{%") {
        Ok(FormatEngine::Jinja)
    } else {
        Ok(FormatEngine::Typst)
    }
}

/// The generic layout used when the `DocType` has no `Print Format`.
fn builtin_format(doctype: &str, settings: PrintSettings) -> PrintFormatDef {
    let mut format = PrintFormatDef::new(
        BUILTIN_FORMAT.to_string(),
        doctype.to_string(),
        crate::GENERIC_TEMPLATE.to_string(),
    );
    format.description = "Generic two-column table of the document's fields".to_string();
    format.paper_size = settings.paper_size;
    format.orientation = settings.orientation;
    format.scale = settings.scale;
    format
}

fn json_value(doc: &Value, key: &str) -> Result<Value> {
    match doc.get(key) {
        None | Some(Value::Null) => Ok(Value::object(std::collections::HashMap::new())),
        Some(Value::String(raw)) if raw.trim().is_empty() => {
            Ok(Value::object(std::collections::HashMap::new()))
        }
        Some(Value::String(raw)) => serde_json::from_str::<serde_json::Value>(raw)
            .map(Value::from_json)
            .map_err(|error| {
                Error::Validation(ValidationError::Invalid(format!(
                    "print format `{key}` is not valid JSON: {error}"
                )))
            }),
        Some(other) => Ok(other.clone()),
    }
}

fn json_object(doc: &Value, key: &str) -> Result<std::collections::HashMap<String, Value>> {
    let value = json_value(doc, key)?;
    match value {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(std::collections::HashMap::new()),
        _ => Err(Error::Validation(ValidationError::Invalid(format!(
            "print format `{key}` must be a JSON object"
        )))),
    }
}

fn doc_str(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn optional_str(doc: &Value, key: &str) -> Option<String> {
    doc.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn doc_bool(doc: &Value, key: &str, default: bool) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn doc_float(doc: &Value, key: &str) -> Option<f32> {
    doc.get(key).and_then(Value::as_f64).and_then(|value| {
        // `TryFrom<f64> for f32` is still unstable, so range-check before
        // narrowing; an out-of-range or non-finite value is ignored.
        if value.is_finite() && value.abs() <= f64::from(f32::MAX) {
            #[allow(clippy::cast_possible_truncation)]
            {
                Some(value as f32)
            }
        } else {
            None
        }
    })
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::Validation(ValidationError::Invalid(format!(
        "unsupported print option: {what}"
    )))
}
