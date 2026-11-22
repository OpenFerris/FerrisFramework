//! Tests for the print endpoints.

use super::*;
use crate::http::host::{PrintHost, print_engine};
use crate::{FormatEngine, Orientation, PaperSize, PrintFormat, PrintSettings};
use axum::http::{StatusCode, header};
use ferris_core::{Error, User, Value};
use ferris_document::{DocumentEngine, EngineContext};
use ferris_schema::{DoctypeMeta, FieldMeta, RolePermission};
use serde_json::json;

const FIXTURE: &str = "Print Fixture";

fn fixture_meta() -> DoctypeMeta {
    DoctypeMeta::new(FIXTURE, "test")
        .field(FieldMeta::data("title").label("Title"))
        .permission(RolePermission::new("Reader").read().print())
        .permission(RolePermission::new("No Print").read())
}

fn register_all() {
    // The DocTypes live in a process-wide registry; registering is
    // idempotent, so every test can call this.
    crate::doctypes::register_doctypes();
    ferris_schema::register_doctype(fixture_meta());
}

fn user(roles: &[&str]) -> User {
    User {
        id: "user-1".to_string(),
        roles: roles.iter().map(|role| (*role).to_string()).collect(),
        ..Default::default()
    }
}

fn system() -> User {
    user(&["System Manager"])
}

fn test_host(state: &ApiState) -> PrintHost {
    PrintHost::new(
        std::sync::Arc::new(state.clone()),
        print_engine().expect("print engine"),
    )
}

async fn fixture_state() -> (ApiState, String) {
    register_all();
    let state = ApiState::new(ferris_db::memory_db().await.unwrap());
    let meta = ferris_api::store::effective_meta(&state.pool, FIXTURE)
        .await
        .unwrap()
        .expect("fixture is registered");
    let mut ctx = EngineContext::new(FIXTURE.to_string(), "");
    ctx.meta = Some(&meta);
    ctx.user = Some(system());
    let doc = DocumentEngine::new(state.pool.clone())
        .create(&ctx, Value::from_json(json!({ "title": "Hello" })))
        .await
        .expect("fixture document");
    let name = doc
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (state, name)
}

async fn create_format(state: &ApiState, body: serde_json::Value) -> String {
    let meta = ferris_api::store::effective_meta(&state.pool, formats::PRINT_FORMAT)
        .await
        .unwrap()
        .expect("Print Format is registered");
    let mut ctx = EngineContext::new(formats::PRINT_FORMAT.to_string(), "");
    ctx.meta = Some(&meta);
    ctx.user = Some(system());
    let doc = DocumentEngine::new(state.pool.clone())
        .create(&ctx, Value::from_json(body))
        .await
        .expect("Print Format row");
    doc.get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_document_renders_to_a_non_empty_pdf() {
    let (state, name) = fixture_state().await;
    let host = test_host(&state);
    let output = render_document(&host, &system(), FIXTURE, &name, &PrintRequest::default())
        .await
        .expect("PDF renders");
    assert_eq!(output.format, PrintFormat::Pdf);
    assert!(output.bytes.starts_with(b"%PDF"));
    assert!(output.bytes.len() > 100, "the PDF must carry content");
}

#[tokio::test]
async fn printing_requires_the_print_permission() {
    let (state, name) = fixture_state().await;
    let host = test_host(&state);
    let error = render_document(
        &host,
        &user(&["No Print"]),
        FIXTURE,
        &name,
        &PrintRequest::default(),
    )
    .await
    .expect_err("a caller without Print must be refused");
    assert!(matches!(&error, Error::Authz(_)), "{error}");
    assert_eq!(
        ferris_api::router::error_response(error).status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn an_invalid_output_format_is_a_bad_request() {
    let (state, name) = fixture_state().await;
    let host = test_host(&state);
    let request = PrintRequest {
        format: Some("gif".to_string()),
        ..Default::default()
    };
    let error = render_document(&host, &system(), FIXTURE, &name, &request)
        .await
        .expect_err("gif is not an output format");
    assert_eq!(
        ferris_api::router::error_response(error).status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn an_invalid_page_option_is_a_bad_request() {
    let (state, name) = fixture_state().await;
    let host = test_host(&state);
    let request = PrintRequest {
        scale: Some(-1.0),
        ..Default::default()
    };
    let error = render_document(&host, &system(), FIXTURE, &name, &request)
        .await
        .expect_err("a negative scale is not renderable");
    assert!(error.to_string().contains("scale"), "{error}");
    assert_eq!(
        ferris_api::router::error_response(error).status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_configured_print_format_is_listed_and_loaded() {
    let (state, _) = fixture_state().await;
    let host = test_host(&state);
    let format_name = create_format(
        &state,
        json!({
            "format_name": "Legal layout",
            "doc_type": FIXTURE,
            "print_format_type": "Server",
            "template": "CUSTOM #title",
            "paper_size": "Legal",
            "orientation": "Landscape",
            "enabled": true,
        }),
    )
    .await;

    let listed = available_formats(&host, FIXTURE).await.unwrap();
    let entry = listed
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(format_name.as_str()))
        .expect("the configured format is listed");
    assert_eq!(
        entry.get("format_name").and_then(Value::as_str),
        Some("Legal layout")
    );
    assert_eq!(entry.get("engine").and_then(Value::as_str), Some("typst"));
    assert!(
        listed
            .iter()
            .any(|entry| entry.get("name").and_then(Value::as_str) == Some(BUILTIN_FORMAT)),
        "the builtin fallback is always offered: {listed:?}"
    );

    let format = load_format(&host, FIXTURE, Some(&format_name), PrintSettings::default())
        .await
        .unwrap();
    assert_eq!(format.template, "CUSTOM #title");
    assert_eq!(format.engine, FormatEngine::Typst);
    assert_eq!(format.paper_size, PaperSize::Legal);
    assert_eq!(format.orientation, Orientation::Landscape);
}

#[tokio::test]
async fn a_jinja_format_renders_with_document_and_parameters() {
    let (state, name) = fixture_state().await;
    let host = test_host(&state);
    let format_name = create_format(
        &state,
        json!({
            "format_name": "Jinja card",
            "doc_type": FIXTURE,
            "engine": "Jinja",
            "template": "#text[{{ doc.title | typst }} {{ params.suffix }}]",
            "parameters": { "suffix": "!" },
            "enabled": true,
        }),
    )
    .await;

    let request = PrintRequest {
        format: Some("html".to_string()),
        print_format: Some(format_name),
        params: std::collections::HashMap::from([(
            "suffix".to_string(),
            Value::string("from request"),
        )]),
        ..Default::default()
    };
    let output = render_document(&host, &system(), FIXTURE, &name, &request)
        .await
        .expect("Jinja format renders");
    let html = String::from_utf8(output.bytes).expect("utf-8");
    assert!(html.contains("Hello from request"), "{html}");
}

#[tokio::test]
async fn preview_renders_an_inline_template_with_sample_data() {
    let (state, _) = fixture_state().await;
    let host = test_host(&state);
    let request = PreviewRequest {
        print: PrintRequest {
            format: Some("html".to_string()),
            ..Default::default()
        },
        template: Some("#text[{{ doc.title | typst }}]".to_string()),
        engine: Some("jinja".to_string()),
        template_dir: None,
        data: Some(Value::from_json(json!({ "title": "Preview card" }))),
    };
    let output = preview_document(&host, &system(), FIXTURE, &request)
        .await
        .expect("the preview renders");
    let html = String::from_utf8(output.bytes).expect("utf-8");
    assert!(html.contains("Preview card"), "{html}");
}

#[tokio::test]
async fn preview_uses_the_stored_sample_data() {
    let (state, _) = fixture_state().await;
    let host = test_host(&state);
    let format_name = create_format(
        &state,
        json!({
            "format_name": "Sample layout",
            "doc_type": FIXTURE,
            "engine": "Jinja",
            "template": "#text[{{ doc.title | typst }}]",
            "sample_data": { "title": "Stored sample" },
            "enabled": true,
        }),
    )
    .await;
    let request = PreviewRequest {
        print: PrintRequest {
            format: Some("html".to_string()),
            print_format: Some(format_name),
            ..Default::default()
        },
        template: None,
        engine: None,
        template_dir: None,
        data: None,
    };
    let output = preview_document(&host, &system(), FIXTURE, &request)
        .await
        .expect("the preview renders");
    let html = String::from_utf8(output.bytes).expect("utf-8");
    assert!(html.contains("Stored sample"), "{html}");
}

#[tokio::test]
async fn preview_requires_the_doctype_print_right() {
    let (state, _) = fixture_state().await;
    let host = test_host(&state);
    let request = PreviewRequest {
        print: PrintRequest::default(),
        template: Some("#text[hi]".to_string()),
        engine: Some("typst".to_string()),
        template_dir: None,
        data: None,
    };
    let error = preview_document(&host, &user(&["No Print"]), FIXTURE, &request)
        .await
        .expect_err("No Print cannot preview");
    assert!(matches!(&error, Error::Authz(_)), "{error}");
}

#[test]
fn the_binary_response_carries_the_content_type_and_a_safe_filename() {
    let output = PrintOutput {
        format: PrintFormat::Pdf,
        bytes: b"%PDF".to_vec(),
    };
    let response = output.into_response("Print Fixture", "A/B");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "inline; filename=\"Print_Fixture-A_B.pdf\""
    );
}

#[test]
fn a_template_never_sees_user_credentials() {
    let user = User {
        id: "user-1".to_string(),
        password_hash: Some("hash".to_string()),
        salt: Some("salt".to_string()),
        mfa_secret: Some("secret".to_string()),
        ..Default::default()
    };
    let value = render::user_context(&user);
    assert!(value.get("password_hash").is_none(), "{value:?}");
    assert!(value.get("salt").is_none(), "{value:?}");
    assert!(value.get("mfa_secret").is_none(), "{value:?}");
    assert_eq!(value.get("id").and_then(Value::as_str), Some("user-1"));
}

#[tokio::test]
async fn the_print_base_methods_render_and_preview_through_the_registry() {
    use base64::Engine as _;
    use ferris_api::MethodRegistry;
    use ferris_document::with_user;

    let (state, name) = fixture_state().await;
    let host = std::sync::Arc::new(test_host(&state));
    let methods = MethodRegistry::new();
    methods::install(&methods, host);

    let render = with_user(
        Some(system()),
        methods.call(
            methods::RENDER_METHOD,
            Value::from_json(json!({
                "doctype": FIXTURE,
                "name": name,
                "format": "pdf",
            })),
        ),
    )
    .await
    .expect("print.render");
    let encoded = render
        .get("bytes_base64")
        .and_then(Value::as_str)
        .expect("base64 payload");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("base64 decodes");
    assert!(bytes.starts_with(b"%PDF"), "print.render must return a PDF");

    let formats = with_user(
        Some(system()),
        methods.call(
            methods::FORMATS_METHOD,
            Value::from_json(json!({ "doctype": FIXTURE })),
        ),
    )
    .await
    .expect("print.formats.list");
    assert!(formats.as_array().is_some_and(|items| !items.is_empty()));

    let preview = with_user(
        Some(system()),
        methods.call(
            methods::PREVIEW_METHOD,
            Value::from_json(json!({
                "doctype": FIXTURE,
                "format": "html",
                "template": "#text[{{ doc.title | typst }}]",
                "engine": "jinja",
                "data": { "title": "Method preview" },
            })),
        ),
    )
    .await
    .expect("print.preview");
    let encoded = preview
        .get("bytes_base64")
        .and_then(Value::as_str)
        .expect("preview payload");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("base64 decodes");
    let html = String::from_utf8(bytes).expect("utf8");
    assert!(html.contains("Method preview"), "{html}");

    let saved = with_user(
        Some(system()),
        methods.call(
            methods::SAVE_FORMAT_METHOD,
            Value::from_json(json!({
                "doctype": FIXTURE,
                "format_name": "Saved by method",
                "engine": "Jinja",
                "template": "#text[{{ doc.title | typst }}]",
                "parameters": {},
                "sample_data": { "title": "Saved sample" },
            })),
        ),
    )
    .await
    .expect("print.format.save");
    assert_eq!(
        saved.get("format_name").and_then(Value::as_str),
        Some("Saved by method")
    );
}
