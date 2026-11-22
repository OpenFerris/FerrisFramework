//! Base API methods for printing.
//!
//! They are framework bases, not app methods: an app can declare
//! `print.render`, `print.preview`, `print.formats.list` or `print.format.save`
//! with `overrides = true` and reach this implementation through
//! `data::call-method` (the explicit `super`).
//!
//! The caller is the **ambient user** set by the API router
//! (`ferris_document::current_user`). A method never trusts a user id from its
//! params: an override could otherwise spoof the permission check on its way in.

use std::collections::HashMap;
use std::path::{Component, Path};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ferris_api::{MethodHandler, MethodRegistry};
use ferris_core::{AuthzError, DocumentError, Error, Result, User, ValidationError, Value};
use ferris_document::{DocumentEngine, EngineContext};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::{FormatEngine, Orientation, PaperSize};

use super::host::PrintHost;
use super::render::{can_print_doctype, deny_print, render_document};
use super::request::{PreviewRequest, PrintOutput, PrintRequest};
use super::{formats, preview};

pub const RENDER_METHOD: &str = "print.render";
pub const PREVIEW_METHOD: &str = "print.preview";
pub const FORMATS_METHOD: &str = "print.formats.list";
pub const SAVE_FORMAT_METHOD: &str = "print.format.save";

/// A finite float as a JSON number.
///
/// `serde_json::Number` has no `From<f64>` (a non-finite float is not valid
/// JSON), so the conversion is fallible and the caller reports the field.
fn json_number(value: f64, what: &str) -> Result<Value> {
    serde_json::Number::from_f64(value)
        .map(Value::number)
        .ok_or_else(|| invalid(format!("{what} is not a finite number")))
}

/// Register the four framework bases.
///
/// A duplicate base is a composition error (for example two builds in one
/// process), not something to overwrite: it is logged and the first registration
/// stays in place.
pub fn install(methods: &MethodRegistry, host: Arc<PrintHost>) {
    let bases: [(&'static str, Arc<dyn MethodHandler>); 4] = [
        (
            RENDER_METHOD,
            Arc::new(RenderMethod {
                host: Arc::clone(&host),
            }),
        ),
        (
            PREVIEW_METHOD,
            Arc::new(PreviewMethod {
                host: Arc::clone(&host),
            }),
        ),
        (
            FORMATS_METHOD,
            Arc::new(FormatsMethod {
                host: Arc::clone(&host),
            }),
        ),
        (SAVE_FORMAT_METHOD, Arc::new(SaveFormatMethod { host })),
    ];
    for (name, handler) in bases {
        if methods.register_base(name, handler).is_err() {
            tracing::warn!(method = name, "print: base method already registered");
        }
    }
}

fn current_user(action: &str) -> Result<User> {
    ferris_document::current_user().ok_or_else(|| {
        Error::Authz(AuthzError::PermissionDenied {
            action: action.to_string(),
            resource: "print".to_string(),
        })
    })
}

fn from_params<T: DeserializeOwned>(params: &Value) -> Result<T> {
    serde_json::from_value(params.to_json()).map_err(|error| {
        Error::Validation(ValidationError::Invalid(format!(
            "invalid print method params: {error}"
        )))
    })
}

fn output_value(output: &PrintOutput) -> Value {
    let bytes = STANDARD.encode(&output.bytes);
    Value::from_json(json!({
        "format": output.format,
        "content_type": output.content_type(),
        "extension": output.extension(),
        "size": output.bytes.len(),
        "bytes_base64": bytes,
    }))
}

#[derive(Debug, Deserialize)]
struct RenderParams {
    doctype: String,
    name: String,
    #[serde(flatten)]
    request: PrintRequest,
}

struct RenderMethod {
    host: Arc<PrintHost>,
}

#[async_trait]
impl MethodHandler for RenderMethod {
    async fn call(&self, params: Value) -> Result<Value> {
        let params: RenderParams = from_params(&params)?;
        let user = current_user(RENDER_METHOD)?;
        let output = render_document(
            &self.host,
            &user,
            &params.doctype,
            &params.name,
            &params.request,
        )
        .await?;
        Ok(output_value(&output))
    }
}

#[derive(Debug, Deserialize)]
struct PreviewParams {
    doctype: String,
    #[serde(flatten)]
    print: PrintRequest,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    template_dir: Option<String>,
    #[serde(default)]
    data: Option<Value>,
}

impl From<PreviewParams> for PreviewRequest {
    fn from(params: PreviewParams) -> Self {
        Self {
            print: params.print,
            template: params.template,
            engine: params.engine,
            template_dir: params.template_dir,
            data: params.data,
        }
    }
}

struct PreviewMethod {
    host: Arc<PrintHost>,
}

#[async_trait]
impl MethodHandler for PreviewMethod {
    async fn call(&self, params: Value) -> Result<Value> {
        let params: PreviewParams = from_params(&params)?;
        let user = current_user(PREVIEW_METHOD)?;
        let doctype = params.doctype.clone();
        let request = PreviewRequest::from(params);
        let output = preview::preview_document(&self.host, &user, &doctype, &request).await?;
        Ok(output_value(&output))
    }
}

#[derive(Debug, Deserialize)]
struct FormatsParams {
    doctype: String,
}

struct FormatsMethod {
    host: Arc<PrintHost>,
}

#[async_trait]
impl MethodHandler for FormatsMethod {
    async fn call(&self, params: Value) -> Result<Value> {
        let params: FormatsParams = from_params(&params)?;
        let user = current_user(FORMATS_METHOD)?;
        // Reading the format list is metadata, not document data; it still
        // requires an authenticated caller and the DocType must exist.
        let meta = ferris_api::store::effective_meta(&self.host.api.pool, &params.doctype)
            .await?
            .ok_or_else(|| {
                Error::Document(DocumentError::NotFound {
                    doctype: "DocType".to_string(),
                    name: params.doctype.clone(),
                })
            })?;
        if !can_print_doctype(&user, &meta) {
            return Err(deny_print(&params.doctype));
        }
        Ok(Value::Array(
            formats::available_formats(&self.host, &params.doctype).await?,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct SaveFormatParams {
    doctype: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    format_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    template_dir: Option<String>,
    #[serde(default)]
    parameters: Option<Value>,
    #[serde(default)]
    sample_data: Option<Value>,
    #[serde(default)]
    paper_size: Option<String>,
    #[serde(default)]
    orientation: Option<String>,
    #[serde(default)]
    scale: Option<f32>,
    #[serde(default)]
    margin_top: Option<f32>,
    #[serde(default)]
    margin_bottom: Option<f32>,
    #[serde(default)]
    margin_left: Option<f32>,
    #[serde(default)]
    margin_right: Option<f32>,
    #[serde(default)]
    enabled: Option<bool>,
}

struct SaveFormatMethod {
    host: Arc<PrintHost>,
}

#[async_trait]
impl MethodHandler for SaveFormatMethod {
    async fn call(&self, params: Value) -> Result<Value> {
        let params: SaveFormatParams = from_params(&params)?;
        let user = current_user(SAVE_FORMAT_METHOD)?;
        save_format(&self.host, &user, params).await
    }
}

// The validation is deliberately one linear pass: each field has a distinct
// error and splitting it would hide the order in which a row is assembled.
#[allow(clippy::too_many_lines)]
async fn save_format(host: &PrintHost, user: &User, params: SaveFormatParams) -> Result<Value> {
    // The target DocType must exist; this also catches a typo before a row is
    // created.
    ferris_api::store::effective_meta(&host.api.pool, &params.doctype)
        .await?
        .ok_or_else(|| {
            Error::Document(DocumentError::NotFound {
                doctype: "DocType".to_string(),
                name: params.doctype.clone(),
            })
        })?;

    let mut body = Value::object(HashMap::new());
    body.set(
        "doc_type".to_string(),
        Value::string(params.doctype.clone()),
    );
    if let Some(format_name) = params.format_name {
        required_text(&format_name, "format_name")?;
        body.set("format_name".to_string(), Value::string(format_name));
    } else if params.name.is_none() {
        return Err(Error::Validation(ValidationError::Required(
            "format_name".to_string(),
        )));
    }
    if let Some(description) = params.description {
        body.set("description".to_string(), Value::string(description));
    }
    if let Some(engine) = params.engine {
        let engine: FormatEngine = engine.parse()?;
        body.set(
            "engine".to_string(),
            Value::string(match engine {
                FormatEngine::Typst => "Typst",
                FormatEngine::Jinja => "Jinja",
            }),
        );
    }
    if let Some(template) = params.template {
        required_text(&template, "template")?;
        body.set("template".to_string(), Value::string(template));
    } else if params.name.is_none() {
        return Err(Error::Validation(ValidationError::Required(
            "template".to_string(),
        )));
    }
    if let Some(template_dir) = params.template_dir {
        validate_relative_dir(&template_dir)?;
        body.set("template_dir".to_string(), Value::string(template_dir));
    }
    if let Some(parameters) = params.parameters {
        require_object(&parameters, "parameters")?;
        body.set("parameters".to_string(), parameters);
    }
    if let Some(sample_data) = params.sample_data {
        require_object(&sample_data, "sample_data")?;
        body.set("sample_data".to_string(), sample_data);
    }
    if let Some(paper_size) = params.paper_size {
        body.set(
            "paper_size".to_string(),
            Value::string(paper_size.parse::<PaperSize>()?.as_str()),
        );
    }
    if let Some(orientation) = params.orientation {
        body.set(
            "orientation".to_string(),
            Value::string(orientation.parse::<Orientation>()?.as_str()),
        );
    }
    if let Some(scale) = params.scale {
        if !scale.is_finite() || scale <= 0.0 {
            return Err(invalid("scale must be a positive number"));
        }
        body.set("scale".to_string(), json_number(f64::from(scale), "scale")?);
    }
    for (key, value) in [
        ("margin_top", params.margin_top),
        ("margin_bottom", params.margin_bottom),
        ("margin_left", params.margin_left),
        ("margin_right", params.margin_right),
    ] {
        if let Some(value) = value {
            if !value.is_finite() || value < 0.0 {
                return Err(invalid(format!("{key} must be finite and non-negative")));
            }
            body.set(key.to_string(), json_number(f64::from(value), key)?);
        }
    }
    if let Some(enabled) = params.enabled {
        body.set("enabled".to_string(), Value::bool(enabled));
    }

    let meta = ferris_api::store::effective_meta(&host.api.pool, formats::PRINT_FORMAT)
        .await?
        .ok_or_else(|| {
            Error::Document(DocumentError::NotFound {
                doctype: "DocType".to_string(),
                name: formats::PRINT_FORMAT.to_string(),
            })
        })?;
    let mut ctx = EngineContext::new(formats::PRINT_FORMAT.to_string(), "");
    ctx.meta = Some(&meta);
    ctx.user = Some(user.clone());

    let saved = match params.name {
        Some(name) => {
            DocumentEngine::new(host.api.pool.clone())
                .update(&ctx, &name, body)
                .await?
        }
        None => {
            DocumentEngine::new(host.api.pool.clone())
                .create(&ctx, body)
                .await?
        }
    };

    Ok(Value::from_json(json!({
        "name": doc_string(&saved, "name"),
        "format_name": doc_string(&saved, "format_name"),
    })))
}

fn required_text(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Validation(ValidationError::Required(
            field.to_string(),
        )));
    }
    Ok(())
}

fn require_object(value: &Value, field: &str) -> Result<()> {
    if matches!(value, Value::Object(_)) {
        Ok(())
    } else {
        Err(invalid(format!("{field} must be a JSON object")))
    }
}

fn validate_relative_dir(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid("template_dir must be a relative path without `..`"));
    }
    Ok(())
}

fn doc_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Validation(ValidationError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_folders_cannot_escape_the_root() {
        validate_relative_dir("invoice/parts").unwrap();
        assert!(validate_relative_dir("../secret").is_err());
        assert!(validate_relative_dir("/etc").is_err());
    }

    #[test]
    fn a_format_parameter_must_be_an_object() {
        assert!(require_object(&Value::object(HashMap::new()), "parameters").is_ok());
        assert!(require_object(&Value::string("x"), "parameters").is_err());
    }
}
