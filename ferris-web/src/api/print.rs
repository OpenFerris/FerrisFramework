//! Print a document through the server's print engine.
//!
//! `/api/print/{doctype}/{name}` renders the document with the configured
//! (or built-in) print format and enforces `Permission::Print`, which is what
//! the form's `allow_print` memo mirrors. The browser asks for a PDF and shows
//! it in a new tab: the viewer's own print button keeps the print dialog, but
//! the page is the document's real print layout instead of a screenshot of the
//! Desk form.
//!
//! Rendering needs the browser's `fetch`/`Blob`; the native build reports that
//! instead of pretending a print happened.

/// Ask the server to render `doctype/name` and open the PDF in a new tab.
///
/// # Errors
///
/// A permission error (the caller may not print this document), a print-engine
/// failure (the format failed to render) or a browser/transport error.
#[cfg(target_arch = "wasm32")]
pub async fn open(doctype: &str, name: &str) -> Result<(), String> {
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let path = format!(
        "/api/print/{}/{}",
        crate::desk::session::path_segment(doctype),
        crate::desk::session::path_segment(name)
    );
    let window = web_sys::window().ok_or_else(|| "sin ventana".to_string())?;
    let headers = web_sys::Headers::new().map_err(|_| "cabeceras".to_string())?;
    let _ = headers.set("content-type", "application/json");
    let _ = headers.set("accept", "application/pdf");
    if let Some(token) = super::client::csrf_token() {
        let _ = headers.set("x-csrf-token", &token);
    }
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_headers(&headers);
    init.set_body(&JsValue::from_str(r#"{"format":"pdf"}"#));
    let request = web_sys::Request::new_with_str_and_init(
        &format!("{}{}", super::client::get_base_url(), path),
        &init,
    )
    .map_err(|_| "solicitud".to_string())?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| "sin conexión".to_string())?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "respuesta".to_string())?;
    let status = response.status();
    if !(200..300).contains(&status) {
        let text = JsFuture::from(response.text().map_err(|_| "cuerpo".to_string())?)
            .await
            .map_err(|_| "cuerpo".to_string())?
            .as_string()
            .unwrap_or_default();
        return Err(super::client::error_message(status, &text));
    }
    let blob = JsFuture::from(response.blob().map_err(|_| "documento".to_string())?)
        .await
        .map_err(|_| "documento".to_string())?;
    let blob = blob
        .dyn_into::<web_sys::Blob>()
        .map_err(|_| "documento".to_string())?;
    let url =
        web_sys::Url::create_object_url_with_blob(&blob).map_err(|_| "documento".to_string())?;
    let _ = window.open_with_url_and_target(&url, "_blank");
    Ok(())
}

/// # Errors
///
/// Always: the native build has no `Blob`/`fetch` to show a PDF with.
#[cfg(not(target_arch = "wasm32"))]
pub async fn open(_doctype: &str, _name: &str) -> Result<(), String> {
    Err("La impresión solo está disponible en el navegador.".to_string())
}
