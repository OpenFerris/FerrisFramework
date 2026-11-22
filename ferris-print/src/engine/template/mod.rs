//! `MiniJinja` rendering for user-authored print formats.
//!
//! The module has one job: turn a [`PrintFormatDef`] plus a document into the
//! Typst source the compiler consumes. It never compiles Typst, touches the
//! database or knows about HTTP; [`super::PrintEngine`] owns that seam.
//!
//! `Typst` formats bypass `MiniJinja` and return the source unchanged. `Jinja`
//! formats get a stable context and a small, total filter set:
//!
//! | Name | Meaning |
//! |---|---|
//! | `doc` / `document` | the document being printed |
//! | `params` | format defaults merged with the request's overrides |
//! | `meta` | the Document Type metadata, when the caller has it |
//! | `user` | the asking user, when the caller has it |
//! | `now`, `today`, `now_unix` | render time |
//! | `format` | name/doctype/engine/description of the format |
//!
//! Filters: `typst` (markup), `typst_str` (string literal), `typst_value`
//! (code literal), `date`/`datetime`, `money`, `number`, `percent`, `json`.

mod escape;
mod filters;
mod root;

pub use root::resolve as resolve_dir;

use chrono::Utc;
use ferris_core::{Error, Result, ValidationError, Value};
use minijinja::{AutoEscape, Environment, UndefinedBehavior, path_loader};

use super::config::{FormatEngine, PrintConfig, PrintFormatDef, PrintOptions};

/// Render the Typst source of `format` for `data`.
pub(super) fn render_source(
    format: &PrintFormatDef,
    data: &Value,
    options: &PrintOptions,
    config: &PrintConfig,
) -> Result<String> {
    check_size(format, config)?;

    if format.engine == FormatEngine::Typst {
        return Ok(format.template.clone());
    }

    let mut env: Environment<'static> = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::None);
    // Chainable makes `doc.optional.deep` render empty instead of failing the
    // document, which is what a print layout wants for an optional relation.
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_recursion_limit(config.limits.recursion_limit);
    env.set_fuel(Some(config.limits.max_fuel));
    filters::register(&mut env);
    if let Some(dir) = root::resolve(config, format)? {
        env.set_loader(path_loader(dir));
        env.set_auto_reload(false);
    }

    let context = minijinja::value::Value::from(minijinja::value::Serde(build_context(
        format, data, options,
    )));
    let rendered = env
        .render_named_str("print-format", &format.template, context)
        .map_err(|error| template_error(format, &error))?;
    if rendered.len() > config.limits.max_output_bytes {
        return Err(invalid(format!(
            "print format `{}` produced {} bytes, over the {} byte limit",
            format.name,
            rendered.len(),
            config.limits.max_output_bytes
        )));
    }
    Ok(rendered)
}

fn check_size(format: &PrintFormatDef, config: &PrintConfig) -> Result<()> {
    if format.template.len() > config.limits.max_template_bytes {
        return Err(invalid(format!(
            "print format `{}` is {} bytes, over the {} byte template limit",
            format.name,
            format.template.len(),
            config.limits.max_template_bytes
        )));
    }
    Ok(())
}

/// Build the JSON context `MiniJinja` receives.
///
/// It is built as plain JSON on purpose: the template cannot reach a Rust
/// object, a handle or a method, only data.
fn build_context(
    format: &PrintFormatDef,
    data: &Value,
    options: &PrintOptions,
) -> serde_json::Value {
    let mut params = format.parameters.clone();
    for (key, value) in &options.params {
        params.insert(key.clone(), value.clone());
    }

    let now = options.now.unwrap_or_else(Utc::now);
    let mut context = serde_json::Map::new();
    context.insert("doc".to_string(), data.to_json());
    context.insert("document".to_string(), data.to_json());
    context.insert(
        "params".to_string(),
        serde_json::Value::Object(
            params
                .iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect(),
        ),
    );
    if let Some(meta) = &options.meta {
        context.insert("meta".to_string(), meta.to_json());
    }
    if let Some(user) = &options.user {
        context.insert("user".to_string(), user.to_json());
    }
    context.insert(
        "now".to_string(),
        serde_json::Value::String(now.to_rfc3339()),
    );
    context.insert(
        "today".to_string(),
        serde_json::Value::String(now.format("%Y-%m-%d").to_string()),
    );
    context.insert(
        "now_unix".to_string(),
        serde_json::Value::Number(now.timestamp().into()),
    );
    context.insert(
        "format".to_string(),
        serde_json::json!({
            "name": format.name.clone(),
            "doctype": format.doctype.clone(),
            "engine": match format.engine {
                FormatEngine::Typst => "typst",
                FormatEngine::Jinja => "jinja",
            },
            "description": format.description.clone(),
        }),
    );
    serde_json::Value::Object(context)
}

fn template_error(format: &PrintFormatDef, error: &minijinja::Error) -> Error {
    invalid(format!(
        "print format `{}` failed to render: {error}",
        format.name
    ))
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Validation(ValidationError::Invalid(message.into()))
}
