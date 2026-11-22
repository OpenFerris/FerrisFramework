//! The Desk's file service: list the store, upload bytes, address a blob.
//!
//! The endpoints are `/api/files` (GET list, POST upload) and
//! `/api/files/{id}` (GET bytes), mounted by `ferris-server` with the session
//! and CSRF gates. `FileItem` is the wire shape the picker consumes.
//!
//! Uploads are browser-only by design: the Desk never proxies a file through
//! the server's SSR process, and the native build (tests, `ssr`) has no
//! `fetch`/`File`. It returns a typed error so a caller cannot mistake "not
//! available here" for "uploaded".

use ferris_web_shared::route::{path_segment, percent_decode};
use serde::{Deserialize, Serialize};

/// One stored file, as the picker lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileItem {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub url: String,
}

/// `GET /api/files`, newest first, filtered by name.
///
/// # Errors
///
/// Returns the API's message when the list fails (the file service disabled, a
/// session-less call, a transport error).
pub async fn list(search: &str, limit: usize) -> Result<Vec<FileItem>, String> {
    let path = format!(
        "/api/files?limit={}&search={}",
        limit.clamp(1, 200),
        path_segment(search)
    );
    let value = super::client::api_json("GET", &path, None).await?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// The bare URL of a stored file (`/api/files/{id}`).
#[must_use]
pub fn url_of(item: &FileItem) -> String {
    if item.url.is_empty() {
        format!("/api/files/{}", item.id)
    } else {
        item.url.clone()
    }
}

/// A URL that keeps the file's name for display.
///
/// A text field stores one string, and `/api/files/{uuid}` alone would show the
/// UUID in the attachment card. The name travels in the fragment: the browser
/// ignores it when resolving the href, and [`split_url`] recovers it for the
/// label. Existing plain URLs keep working.
#[must_use]
pub fn display_url(item: &FileItem) -> String {
    let name = if item.name.is_empty() {
        "archivo"
    } else {
        item.name.as_str()
    };
    format!("{}#{}", url_of(item), path_segment(name))
}

/// `(href, file name)` of a value produced by [`display_url`] (or a plain URL).
#[must_use]
pub fn split_url(value: &str) -> (String, String) {
    match value.split_once('#') {
        Some((href, name)) => (href.to_string(), percent_decode(name)),
        None => {
            let name = value
                .rsplit('/')
                .next()
                .filter(|segment| !segment.is_empty())
                .unwrap_or(value)
                .to_string();
            (value.to_string(), name)
        }
    }
}

/// Upload `bytes` with `name` and `content_type`.
///
/// # Errors
///
/// Returns the API's message (size, type, ACL) or a browser error.
pub async fn upload(name: &str, content_type: &str, bytes: Vec<u8>) -> Result<FileItem, String> {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;

        let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
        let headers = web_sys::Headers::new().map_err(|_| "headers".to_string())?;
        let _ = headers.set("content-type", content_type);
        let _ = headers.set("x-file-name", &path_segment(name));
        if let Some(token) = super::client::csrf_token() {
            let _ = headers.set("x-csrf-token", &token);
        }
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        init.set_headers(&headers);
        let array = js_sys::Uint8Array::from(bytes.as_slice());
        init.set_body(&array);
        let request = web_sys::Request::new_with_str_and_init(
            &format!("{}/api/files", super::client::get_base_url()),
            &init,
        )
        .map_err(|_| "request".to_string())?;
        let response = JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(|_| "fetch failed".to_string())?
            .dyn_into::<web_sys::Response>()
            .map_err(|_| "response".to_string())?;
        let status = response.status();
        let text = JsFuture::from(response.text().map_err(|_| "body".to_string())?)
            .await
            .map_err(|_| "body".to_string())?
            .as_string()
            .unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(super::client::error_message(status, &text));
        }
        return serde_json::from_str(&text).map_err(|error| error.to_string());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The native build has no `File`/`fetch`: refuse loudly instead of
        // pretending the upload happened.
        let _ = (name, content_type, bytes);
        Err("La subida de archivos solo está disponible en el navegador.".to_string())
    }
}

/// Human size (`1.2 MB`) for the picker rows.
#[must_use]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::{FileItem, display_url, human_size, split_url, url_of};

    fn item() -> FileItem {
        FileItem {
            id: "abc".into(),
            name: "informe final.pdf".into(),
            content_type: "application/pdf".into(),
            size: 1536,
            created_at: None,
            url: "/api/files/abc".into(),
        }
    }

    #[test]
    fn a_display_url_keeps_the_name_and_round_trips() {
        let value = display_url(&item());
        assert!(value.starts_with("/api/files/abc#"));
        let (href, name) = split_url(&value);
        assert_eq!(href, url_of(&item()));
        assert_eq!(name, "informe final.pdf");
        let (href, name) = split_url("/files/viejo.pdf");
        assert_eq!(href, "/files/viejo.pdf");
        assert_eq!(name, "viejo.pdf");
    }

    #[test]
    fn sizes_read_in_human_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
