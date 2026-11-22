//! Preview: render a stored or inline format against sample data.
//!
//! A preview has no document to read, so it is gated by the `DocType`'s `print`
//! right. Everything else (limits, engine, options) is the production path: a
//! format that renders in the preview renders in the document endpoint.

use ferris_core::{DocumentError, Error, Result, User, ValidationError, Value};

use crate::{
    FormatEngine, Margins, PrintFormatDef,
    http::formats::load_format,
    http::host::PrintHost,
    http::render::{can_print_doctype, deny_print, load_print_settings},
};

use super::request::{PreviewRequest, PrintOutput};

pub async fn preview_document(
    host: &PrintHost,
    user: &User,
    doctype: &str,
    request: &PreviewRequest,
) -> Result<PrintOutput> {
    let meta = ferris_api::store::effective_meta(&host.api.pool, doctype)
        .await?
        .ok_or_else(|| {
            Error::Document(DocumentError::NotFound {
                doctype: "DocType".to_string(),
                name: doctype.to_string(),
            })
        })?;
    if !can_print_doctype(user, &meta) {
        return Err(deny_print(doctype));
    }
    if request.template.is_some() && request.print.print_format.is_some() {
        return Err(Error::Validation(ValidationError::Invalid(
            "send either `template` or `print_format`, not both".to_string(),
        )));
    }

    let settings = load_print_settings(host).await?;
    let format = match &request.template {
        Some(template) => {
            if template.trim().is_empty() {
                return Err(Error::Validation(ValidationError::Invalid(
                    "the preview template is empty".to_string(),
                )));
            }
            let engine = match request.engine.as_deref() {
                Some(raw) => raw.parse::<FormatEngine>()?,
                None => FormatEngine::Jinja,
            };
            let mut format = PrintFormatDef::new(
                "<preview>".to_string(),
                doctype.to_string(),
                template.clone(),
            );
            format.engine = engine;
            format.template_dir.clone_from(&request.template_dir);
            format.paper_size = settings.paper_size;
            format.orientation = settings.orientation;
            format.scale = settings.scale;
            format.margins = Margins::default();
            format
        }
        None => {
            load_format(
                host,
                doctype,
                request.print.print_format.as_deref(),
                settings,
            )
            .await?
        }
    };

    let data = match request.data.clone() {
        Some(Value::Null) => Value::object(std::collections::HashMap::new()),
        Some(data) => data,
        None => match format.sample_data.clone() {
            Value::Null => Value::object(std::collections::HashMap::new()),
            sample => sample,
        },
    };

    let mut options = request.print.options()?;
    let output_format = options.format;
    options.meta = Some(super::request::to_value(&meta)?);
    options.user = Some(super::render::user_context(user));

    // The same bounded path as a document render: an inline template is the
    // easiest way to send Typst a runaway loop, and a preview must not get a
    // private, unbounded compiler (PRINT-01/PRINT-06).
    let bytes = super::render::render_with_limits(host, &format, &data, &options).await?;

    Ok(PrintOutput {
        format: output_format,
        bytes,
    })
}
