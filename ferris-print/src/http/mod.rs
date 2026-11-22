//! Print endpoints: render a document with a `Print Format` (or the builtin
//! generic layout) through [`crate::PrintEngine`].
//!
//! Three decisions shape this module:
//!
//! * **The document's own permission is the gate.** `POST /api/print/{doctype}/{name}`
//!   reads the document with [`Permission::Print`], so a caller who can read but
//!   not print is refused by the engine before a single byte is rendered. A
//!   `Print Format` is a layout definition, not per-user data, so the format row
//!   is read with a system context: requiring `Read` on `Print Format` would make
//!   printing an admin-only operation.
//! * **Formats live in one place.** A `Print Format` row references its
//!   `doc_type`, chooses `jinja` or `typst`, carries the template, default
//!   parameters and sample data, and owns its paper/margins/orientation/scale.
//!   There is no second registry.
//! * **Every option is honoured or rejected.** `paper_size`, `margins`,
//!   `orientation`, `scale`, `watermark` and the output `format` reach the Typst
//!   source; an unusable value is a typed error instead of a silent no-op.

use std::sync::{Arc, OnceLock};

use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use ferris_api::{ApiState, ModuleState, OpenApiFragment};
use ferris_auth::middleware::AuthenticatedUser;
use ferris_core::workspaces::WorkspaceRegistration;
use serde_json::json;

mod formats;
mod host;
mod methods;
mod preview;
mod render;
mod request;

#[cfg(test)]
pub(crate) use formats::{available_formats, load_format};
#[cfg(test)]
pub(crate) use preview::preview_document;
#[cfg(test)]
pub(crate) use render::render_document;
pub use request::{PreviewRequest, PrintOutput, PrintRequest};

/// Name of the builtin layout offered when the `DocType` has no enabled format.
pub const BUILTIN_FORMAT: &str = "builtin";

/// `POST /api/print/{doctype}/{name}`.
pub async fn print_document(
    State(state): State<Arc<ApiState>>,
    user: AuthenticatedUser,
    Path((doctype, name)): Path<(String, String)>,
    Json(request): Json<request::PrintRequest>,
) -> Response {
    let host = match host::host_from_api(state) {
        Ok(host) => host,
        Err(error) => return ferris_api::router::error_response(error),
    };
    match render::render_document(&host, &user.0, &doctype, &name, &request).await {
        Ok(output) => output.into_response(&doctype, &name),
        Err(error) => ferris_api::router::error_response(error),
    }
}

/// `GET /api/print/formats/{doctype}`.
pub async fn formats(
    State(state): State<Arc<ApiState>>,
    _user: AuthenticatedUser,
    Path(doctype): Path<String>,
) -> Response {
    let host = match host::host_from_api(state) {
        Ok(host) => host,
        Err(error) => return ferris_api::router::error_response(error),
    };
    match formats::available_formats(&host, &doctype).await {
        Ok(formats) => Json(formats).into_response(),
        Err(error) => ferris_api::router::error_response(error),
    }
}

/// `POST /api/print/preview/{doctype}`.
pub async fn preview(
    State(state): State<Arc<ApiState>>,
    user: AuthenticatedUser,
    Path(doctype): Path<String>,
    Json(request): Json<request::PreviewRequest>,
) -> Response {
    let host = match host::host_from_api(state) {
        Ok(host) => host,
        Err(error) => return ferris_api::router::error_response(error),
    };
    match preview::preview_document(&host, &user.0, &doctype, &request).await {
        Ok(output) => output.into_response(&doctype, "preview"),
        Err(error) => ferris_api::router::error_response(error),
    }
}

/// The print feature as a native module.
///
/// It carries the API state captured at boot so the base methods can reach the
/// same database as the HTTP handlers. Routes still resolve their state through
/// `ModuleState`, so they work before (and without) the method registration.
#[derive(Default, Clone)]
pub struct PrintModule {
    api: Arc<OnceLock<ApiState>>,
}

#[async_trait::async_trait]
impl ferris_api::Module for PrintModule {
    fn name(&self) -> &'static str {
        "print"
    }

    fn register_doctypes(&self) {
        crate::doctypes::register_doctypes();
    }

    fn register_workspaces(&self) -> WorkspaceRegistration {
        crate::workspaces::register_workspaces()
    }

    // `Module::boot` is async by contract; this module has no background task
    // yet, so the future completes without an await.
    #[allow(clippy::unused_async_trait_impl)]
    async fn boot(&self, ctx: &ferris_api::ServiceContext) -> ferris_core::Result<()> {
        // Initialise the process-wide engine here so a bad font/template
        // configuration fails at boot, not on the first request.
        let _engine = host::print_engine()?;
        if self.api.set(ApiState::new(ctx.db.clone())).is_err() {
            tracing::warn!("print: boot ran twice; keeping the first API state");
        }
        Ok(())
    }

    fn register_methods(&self, methods: &ferris_api::MethodRegistry) {
        let Some(api) = self.api.get().cloned() else {
            tracing::warn!("print: methods requested before boot; not registering");
            return;
        };
        let engine = match host::print_engine() {
            Ok(engine) => engine,
            Err(error) => {
                tracing::error!(error = %error, "print: cannot register methods");
                return;
            }
        };
        let host = Arc::new(host::PrintHost::new(Arc::new(api), engine));
        methods::install(methods, host);
    }

    fn routes(&self) -> axum::Router<ModuleState> {
        axum::Router::new()
            .route("/print/formats/{doctype}", axum::routing::get(formats))
            .route("/print/preview/{doctype}", axum::routing::post(preview))
            .route(
                "/print/{doctype}/{name}",
                axum::routing::post(print_document),
            )
    }

    fn openapi(&self) -> OpenApiFragment {
        OpenApiFragment {
            paths: vec![
                (
                    "/api/print/formats/{doctype}".to_string(),
                    json!({
                        "get": {
                            "summary": "List the print formats available for a DocType",
                            "tags": ["print"],
                            "parameters": [{
                                "name": "doctype",
                                "in": "path",
                                "required": true,
                                "schema": { "type": "string" }
                            }],
                            "responses": {
                                "200": { "description": "Formats and the builtin layout" }
                            }
                        }
                    }),
                ),
                (
                    "/api/print/preview/{doctype}".to_string(),
                    json!({
                        "post": {
                            "summary": "Preview a print format with sample data",
                            "description": "Renders a stored Print Format or an inline template. Requires the DocType's print right.",
                            "tags": ["print"],
                            "parameters": [{
                                "name": "doctype",
                                "in": "path",
                                "required": true,
                                "schema": { "type": "string" }
                            }],
                            "requestBody": {
                                "required": true,
                                "content": {
                                    "application/json": {
                                        "schema": { "type": "object" }
                                    }
                                }
                            },
                            "responses": {
                                "200": { "description": "Rendered preview" }
                            }
                        }
                    }),
                ),
                (
                    "/api/print/{doctype}/{name}".to_string(),
                    json!({
                        "post": {
                            "summary": "Render a document with a Print Format",
                            "tags": ["print"],
                            "parameters": [
                                {
                                    "name": "doctype",
                                    "in": "path",
                                    "required": true,
                                    "schema": { "type": "string" }
                                },
                                {
                                    "name": "name",
                                    "in": "path",
                                    "required": true,
                                    "schema": { "type": "string" }
                                }
                            ],
                            "requestBody": {
                                "required": false,
                                "content": {
                                    "application/json": {
                                        "schema": { "type": "object" }
                                    }
                                }
                            },
                            "responses": {
                                "200": { "description": "Rendered document" }
                            }
                        }
                    }),
                ),
            ],
        }
    }
}

#[cfg(test)]
mod tests;
