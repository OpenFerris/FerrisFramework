//! HTTP client for the Ferris REST API.
//!
//! Used by the hydrate (wasm) path of [`super::store`]; the SSR path talks to
//! the engine in-process via [`super::server`].

use crate::api::meta_types::DynMeta;
use ferris_web_shared::route::path_segment;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::types::ApiResponse;

/// Metadata does not change while a Desk session is open unless the user edits
/// the runtime customizations. Every list/form/dashboard for the same doctype
/// used to fetch the same document again; keep the first answer for the rest of
/// the session and drop it only when a customization write proves it stale.
static META_CACHE: OnceLock<Mutex<HashMap<String, Option<DynMeta>>>> = OnceLock::new();

/// The session's CSRF token, captured from the `X-CSRF-Token`
/// response header that login/session/mutating responses expose and echoed back
/// on the next mutating request. `None` until a login happens. Mirrors the
/// server's `ferris_auth::CSRF_HEADER` name; kept as a literal so the hydrate
/// (wasm) client does not drag the whole auth crate into the browser build.
const CSRF_HEADER_NAME: &str = "x-csrf-token";

static CSRF_TOKEN: Mutex<Option<String>> = Mutex::new(None);

/// Whether a request mutates server state and therefore must carry the CSRF
/// token. This is the mirror of the server's `require_csrf` gate:
/// `GET`/`HEAD`/`OPTIONS` are exempt, everything else is checked.
fn is_mutating(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

/// The captured CSRF token, if any.
pub(crate) fn csrf_token() -> Option<String> {
    CSRF_TOKEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Remember the token the server handed back with a response. Non-mutating
/// responses (`GET /api/auth/session`, login, …) refresh the stored value so a
/// session renewal keeps the desk in sync.
fn capture_csrf_token(token: Option<&str>) {
    if let Some(token) = token
        && let Ok(mut guard) = CSRF_TOKEN.lock()
    {
        *guard = Some(token.to_string());
    }
}

/// Origin of the Ferris API.
///
/// In the browser this is read once from the live location -- never hardcoded,
/// so the desk works on any host or port. A native build has no window to ask,
/// so it reads `FERRIS_API_URL` and otherwise points at the dev server.
pub fn get_base_url() -> &'static str {
    static BASE: OnceLock<String> = OnceLock::new();
    BASE.get_or_init(|| {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .map(|window| {
                    let location = window.location();
                    location.origin().unwrap_or_else(|_| {
                        format!(
                            "{}://{}",
                            location.protocol().unwrap_or_default(),
                            location.host().unwrap_or_default()
                        )
                    })
                })
                .unwrap_or_default()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::env::var("FERRIS_API_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".to_string())
        }
    })
    .as_str()
}

/// Join a constant prefix with already-known path segments, percent-encoding
/// the dynamic ones.
///
/// Doctypes carry spaces ("Sales Invoice") and docnames can carry anything, so
/// interpolating them raw produces a URL the server cannot route.
fn path(prefix: &str, segments: &[&str]) -> String {
    let mut out = String::with_capacity(prefix.len() + segments.len() * 16);
    out.push_str(prefix);
    for segment in segments {
        out.push('/');
        // One encoder for the whole crate graph: the metadata layer builds the
        // same routes, and two implementations could disagree on `%`.
        out.push_str(&path_segment(segment));
    }
    out
}

/// Body of a request whose response must be 2xx.
async fn request_text(method: &str, path: &str, body: Option<&str>) -> Result<String, String> {
    let (status, text) = api_request_raw(method, path, body).await?;
    if (200..300).contains(&status) {
        Ok(text)
    } else {
        Err(transport::error_message(status, &text))
    }
}

/// POST to an authenticated session; returns the JSON body on success.
pub async fn api_post_raw(path: &str, body: &str) -> Result<String, String> {
    request_text("POST", path, Some(body)).await
}

/// GET to the API; returns the JSON body on success.
pub async fn api_get_raw(path: &str) -> Result<String, String> {
    request_text("GET", path, None).await
}

/// PUT to the API; returns the JSON body on success.
pub async fn api_put_raw(path: &str, body: &str) -> Result<String, String> {
    request_text("PUT", path, Some(body)).await
}

/// DELETE to the API; returns the JSON body on success.
pub async fn api_delete_raw(path: &str) -> Result<String, String> {
    request_text("DELETE", path, None).await
}

/// Fetch doctype metadata from the API, memoised for the session.
pub async fn api_meta(doctype: &str) -> Result<Option<DynMeta>, String> {
    if let Some(cached) = META_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(doctype)
    {
        return Ok(cached.clone());
    }

    let text = api_get_raw(&path("/api/doctype/meta", &[doctype])).await?;
    let meta: Option<DynMeta> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    META_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(doctype.to_string(), meta.clone());
    Ok(meta)
}

/// Drop the cached metadata of `doctype` after a runtime customization write.
fn invalidate_meta(doctype: &str) {
    if let Ok(mut cache) = META_CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        cache.remove(doctype);
    }
}

/// Drop every cached doctype.
///
/// The identity and the app manifest are part of what `/api/doctype/meta`
/// answers with, so a sign-in, sign-out or a plugin hot reload invalidates the
/// whole map rather than one doctype.
pub fn clear_meta_cache() {
    if let Ok(mut cache) = META_CACHE.get_or_init(|| Mutex::new(HashMap::new())).lock() {
        cache.clear();
    }
}

/// Hit the desktop list endpoint (`POST /api/resource/{doctype}/list`).
///
/// Returns the decoded envelope rather than its JSON text: round-tripping
/// through a `String` made every row be encoded and parsed twice per fetch.
pub async fn api_list(
    params: &super::types::ListParams,
    doctype: &str,
) -> Result<ApiResponse<Value>, String> {
    let body = serde_json::to_string(params).map_err(|e| e.to_string())?;
    let value = api_json(
        "POST",
        &path("/api/resource", &[doctype, "list"]),
        Some(&body),
    )
    .await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Create a document.
pub async fn api_create(doctype: &str, body: impl Serialize) -> Result<Value, String> {
    let payload = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let text = api_post_raw(&path("/api/resource", &[doctype]), &payload).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Update a document.
pub async fn api_update(doctype: &str, name: &str, body: impl Serialize) -> Result<Value, String> {
    let payload = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let text = api_put_raw(&path("/api/resource", &[doctype, name]), &payload).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Delete a document.
pub async fn api_delete(doctype: &str, name: &str) -> Result<(), String> {
    api_delete_raw(&path("/api/resource", &[doctype, name])).await?;
    Ok(())
}

/// Read the one document of a Single DocType (materialsing it if needed).
pub async fn api_get_single(doctype: &str) -> Result<Value, String> {
    let text = api_get_raw(&path("/api/single", &[doctype])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Save the one document of a Single DocType.
pub async fn api_put_single(doctype: &str, body: impl Serialize) -> Result<Value, String> {
    let payload = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let text = api_put_raw(&path("/api/single", &[doctype]), &payload).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Submit a document (docstatus 0 -> 1).
pub async fn api_submit(doctype: &str, name: &str) -> Result<Value, String> {
    let text = api_post_raw(&path("/api/resource", &[doctype, name, "submit"]), "{}").await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Rename a document (move its primary key).
pub async fn api_rename(doctype: &str, name: &str, new_name: &str) -> Result<Value, String> {
    let body = serde_json::json!({ "new_name": new_name }).to_string();
    let text = api_post_raw(&path("/api/resource", &[doctype, name, "rename"]), &body).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Cancel a document (docstatus 1 -> 2 or 2 -> 0).
pub async fn api_cancel(doctype: &str, name: &str) -> Result<Value, String> {
    let text = api_post_raw(&path("/api/resource", &[doctype, name, "cancel"]), "{}").await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Amend a document: clone it into a fresh draft with a new name.
pub async fn api_amend(doctype: &str, name: &str) -> Result<Value, String> {
    let text = api_post_raw(&path("/api/resource", &[doctype, name, "amend"]), "{}").await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Like and comment state of the documents a list just loaded.
///
/// The names go in a JSON body, like the list request does: a docname may
/// contain a comma, so a joined query value would be ambiguous.
pub async fn api_engagement(
    doctype: &str,
    names: &[String],
) -> Result<super::types::EngagementPage, String> {
    let body =
        serde_json::to_string(&serde_json::json!({ "names": names })).map_err(|e| e.to_string())?;
    let text = api_post_raw(&path("/api/engagement", &[doctype]), &body).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Like a document, or unlike it when already liked.
pub async fn api_toggle_like(doctype: &str, name: &str) -> Result<super::types::LikeState, String> {
    let text = api_post_raw(&path("/api/engagement", &[doctype, name, "like"]), "{}").await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// The comments of a document, oldest first.
pub async fn api_list_comments(
    doctype: &str,
    name: &str,
) -> Result<Vec<super::types::DocComment>, String> {
    let text = api_get_raw(&path("/api/engagement", &[doctype, name, "comments"])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Post a comment on a document, or a reply when `parent` names another
/// comment's id.
pub async fn api_add_comment(
    doctype: &str,
    name: &str,
    body: &str,
    parent: Option<&str>,
) -> Result<super::types::DocComment, String> {
    // `parent` is sent explicitly (null for a top-level comment) so the server
    // body shape is the same either way.
    let payload = serde_json::to_string(&serde_json::json!({ "body": body, "parent": parent }))
        .map_err(|e| e.to_string())?;
    let text = api_post_raw(
        &path("/api/engagement", &[doctype, name, "comments"]),
        &payload,
    )
    .await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// A document's activity timeline, oldest first.
pub async fn api_activity(
    doctype: &str,
    name: &str,
) -> Result<Vec<super::types::ActivityEntry>, String> {
    let text = api_get_raw(&path("/api/activity", &[doctype, name])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Call a whitelisted server method (`POST /api/method/{method}`).
///
/// This is what a `DocType Action` of type `Server Action` executes; the body is
/// the `kwargs` object the method receives, exactly like Frappe's whitelisted
/// functions.
pub async fn api_call_method(
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = serde_json::to_string(&params).map_err(|e| e.to_string())?;
    let text = api_post_raw(&path("/api/method", &[method]), &body).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// The role × custom-permission-type matrix of one DocType.
pub async fn api_permission_matrix(
    doctype: &str,
) -> Result<super::types::PermissionMatrix, String> {
    let text = api_get_raw(&path("/api/doctype/permissions", &[doctype, "manage"])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Grant or revoke one custom permission type for one role.
pub async fn api_set_custom_permission(
    doctype: &str,
    role: &str,
    perm_type: &str,
    granted: bool,
) -> Result<(), String> {
    let body = serde_json::json!({
        "role": role,
        "perm_type": perm_type,
        "granted": granted,
    })
    .to_string();
    api_put_raw(
        &path("/api/doctype/permissions", &[doctype, "manage"]),
        &body,
    )
    .await
    .map(|_| ())
}

/// The Connections panel of one document.
pub async fn api_connections(
    doctype: &str,
    name: &str,
) -> Result<super::types::Connections, String> {
    let text = api_get_raw(&path("/api/doctype/connections", &[doctype, name])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// A document's assignments, oldest first.
pub async fn api_list_assignments(
    doctype: &str,
    name: &str,
) -> Result<Vec<super::types::Assignment>, String> {
    let text = api_get_raw(&path("/api/engagement", &[doctype, name, "assignments"])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Assign `user` to a document.
pub async fn api_add_assignment(
    doctype: &str,
    name: &str,
    user: &str,
) -> Result<super::types::Assignment, String> {
    let payload =
        serde_json::to_string(&serde_json::json!({ "user": user })).map_err(|e| e.to_string())?;
    let text = api_post_raw(
        &path("/api/engagement", &[doctype, name, "assignments"]),
        &payload,
    )
    .await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Remove `user` from a document's assignments.
pub async fn api_remove_assignment(doctype: &str, name: &str, user: &str) -> Result<(), String> {
    api_delete_raw(&path(
        "/api/engagement",
        &[doctype, name, "assignments", user],
    ))
    .await
    .map(|_| ())
}

/// A document's attachments, oldest first.
pub async fn api_list_attachments(
    doctype: &str,
    name: &str,
) -> Result<Vec<super::types::Attachment>, String> {
    let text = api_get_raw(&path("/api/engagement", &[doctype, name, "attachments"])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Register an already-stored file as an attachment of a document.
pub async fn api_add_attachment(
    doctype: &str,
    name: &str,
    file_name: &str,
    file_url: &str,
    file_size: u64,
) -> Result<super::types::Attachment, String> {
    let payload = serde_json::to_string(&serde_json::json!({
        "file_name": file_name,
        "file_url": file_url,
        "file_size": file_size,
    }))
    .map_err(|e| e.to_string())?;
    let text = api_post_raw(
        &path("/api/engagement", &[doctype, name, "attachments"]),
        &payload,
    )
    .await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Remove the attachment `id` from a document.
pub async fn api_remove_attachment(doctype: &str, name: &str, id: &str) -> Result<(), String> {
    api_delete_raw(&path(
        "/api/engagement",
        &[doctype, name, "attachments", id],
    ))
    .await
    .map(|_| ())
}

/// A document's share grants.
pub async fn api_list_shares(
    doctype: &str,
    name: &str,
) -> Result<Vec<super::types::Share>, String> {
    let text = api_get_raw(&path("/api/engagement", &[doctype, name, "shares"])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Share a document with `user`.
pub async fn api_add_share(
    doctype: &str,
    name: &str,
    user: &str,
    read: bool,
    write: bool,
    share: bool,
) -> Result<super::types::Share, String> {
    let payload = serde_json::to_string(&serde_json::json!({
        "user": user,
        "read": read,
        "write": write,
        "share": share,
    }))
    .map_err(|e| e.to_string())?;
    let text = api_post_raw(
        &path("/api/engagement", &[doctype, name, "shares"]),
        &payload,
    )
    .await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Remove `user`'s share grant on a document.
pub async fn api_remove_share(doctype: &str, name: &str, user: &str) -> Result<(), String> {
    api_delete_raw(&path("/api/engagement", &[doctype, name, "shares", user]))
        .await
        .map(|_| ())
}

/// The previous/next docname of a document in list order.
pub async fn api_neighbors(doctype: &str, name: &str) -> Result<super::types::Neighbors, String> {
    let text = api_get_raw(&path("/api/resource", &[doctype, name, "neighbors"])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// The current runtime customizations of a DocType.
pub async fn api_get_customization(doctype: &str) -> Result<super::types::Customization, String> {
    let text = api_get_raw(&path("/api/customization", &[doctype])).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Add or replace a custom field of a DocType.
pub async fn api_put_custom_field(
    doctype: &str,
    spec: &super::types::CustomFieldSpec,
) -> Result<super::types::CustomFieldSpec, String> {
    let payload = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let text = api_post_raw(&path("/api/customization", &[doctype, "fields"]), &payload).await?;
    let saved = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    invalidate_meta(doctype);
    Ok(saved)
}

/// Undo a custom field of a DocType.
pub async fn api_delete_custom_field(doctype: &str, fieldname: &str) -> Result<(), String> {
    api_delete_raw(&path("/api/customization", &[doctype, "fields", fieldname])).await?;
    invalidate_meta(doctype);
    Ok(())
}

/// Add or replace a property setter of a DocType.
pub async fn api_put_property_setter(
    doctype: &str,
    spec: &super::types::PropertySetterSpec,
) -> Result<super::types::PropertySetterSpec, String> {
    let payload = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let text = api_post_raw(
        &path("/api/customization", &[doctype, "properties"]),
        &payload,
    )
    .await?;
    let saved = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    invalidate_meta(doctype);
    Ok(saved)
}

/// Write this DocType's customization into a plugin world.
///
/// Returns the path relative to the project (`plugins/<app>/customizations/…`).
/// The server validates the app name; a new app gets its world created.
///
/// `scope` narrows the file to the changes the caller selected (the form UI
/// exports what the user just edited); `None` exports the whole customization.
/// The scope is the shared [`ExportScope`](super::types::ExportScope) from the
/// wire contract, so the spelling the browser sends is the one the server
/// deserializes.
pub async fn api_export_customization(
    doctype: &str,
    app: &str,
    scope: Option<super::types::ExportScope>,
) -> Result<String, String> {
    let payload = serde_json::to_string(&serde_json::json!({ "app": app, "scope": scope }))
        .map_err(|e| e.to_string())?;
    let text = api_post_raw(&path("/api/customization", &[doctype, "export"]), &payload).await?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "unexpected export response".to_string())
}

/// Undo a property setter of a DocType, optionally scoped to a field.
pub async fn api_delete_property_setter(
    doctype: &str,
    fieldname: Option<&str>,
    property: &str,
) -> Result<(), String> {
    let mut target = path("/api/customization", &[doctype, "properties", property]);
    if let Some(fieldname) = fieldname {
        target.push_str("?fieldname=");
        target.push_str(&path_segment(fieldname));
    }
    api_delete_raw(&target).await?;
    invalidate_meta(doctype);
    Ok(())
}

mod transport;

pub use transport::{api_json, api_request_raw, error_message};

#[cfg(test)]
mod tests;

/// Report one client-side warning or error to `POST /api/log/client`.
///
/// The endpoint writes a server log line (no database row) and echoes nothing,
/// so this is the only durable channel a browser error has. Failures are
/// returned for a caller that wants to surface them; a log sink must never be
/// the reason a save fails, so callers normally ignore the result.
///
/// # Errors
///
/// A transport failure or a non-2xx answer from the endpoint.
pub async fn log_client(level: &str, message: &str, context: Option<&str>) -> Result<(), String> {
    let body = serde_json::json!({
        "level": level,
        "message": message,
        "context": context,
    });
    let body = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    api_post_raw("/api/log/client", &body).await.map(|_| ())
}

/// The guided tours declared for `doctype` (`GET /api/doctype/form-tours/{doctype}`).
///
/// A missing metadata table answers an empty list, never an error.
///
/// # Errors
///
/// A transport failure or a malformed payload.
pub async fn api_form_tours(doctype: &str) -> Result<Vec<Value>, String> {
    let path = path("/api/doctype/form-tours", &[doctype]);
    let body = api_get_raw(&path).await?;
    let payload: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    Ok(payload
        .get("tours")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}
