//! Desk file endpoints: list the store, upload a file, serve a blob.
//!
//! These are the endpoints the file picker uses (attachments, image fields,
//! the sidebar). They are not the public signed-URL surface in `signed_url.rs`:
//! a caller here is an authenticated Desk user, the session cookie authorises
//! the request and the store's `AccessContext` enforces the per-file ACL.
//!
//! `FilesModule::routes` only returns this router when `boot` built a shared
//! store; an install without a signing secret still has the Desk API, and a
//! store that cannot open leaves the routes absent instead of failing the
//! process.

use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use ferris_api::{MethodHandler, ModuleState};
use ferris_auth::middleware::OptionalUser;
use ferris_core::{AuthzError, Error, Result, User, Value as JsonValue};
use serde::Deserialize;
use serde_json::json;

use crate::{AccessContext, FileRef, FileUpload, SignedDownloads, UploadOptions};

/// Folder every Desk upload lands in. The store's `list` takes a path prefix.
const UPLOAD_FOLDER: &str = "uploads";

/// Opt-in header that publishes an upload to the shared store. Desk uploads are
/// private by default: `files.list` and `/api/files/{id}` use the caller's
/// `AccessContext`, so a public default would hand every attachment to every
/// signed-in user. Only an administrator may opt out of that rule, and only by
/// asking for it explicitly.
const PUBLIC_UPLOAD_HEADER: &str = "x-file-public";

/// The API method the Desk's file list resolves through.
///
/// `files.list` is a **base** in the method chain: an app may declare the same
/// name with `overrides = true` (and the `overrides` capability) to replace the
/// listing — a Drive, a remote object store — and the Desk's picker uses it with
/// no frontend change, because `GET /api/files` is only an adapter over this
/// method. The override receives `{ "search": string, "limit": number }` and
/// must answer the same array (`id`, `name`, `content_type`, `size`,
/// `created_at`, `url`).
pub(crate) const FILES_LIST_METHOD: &str = "files.list";

/// The base implementation of `files.list`: the framework's shared store.
struct FilesListBase {
    files: Arc<SignedDownloads>,
}

#[async_trait::async_trait]
impl MethodHandler for FilesListBase {
    async fn call(&self, params: JsonValue) -> Result<JsonValue> {
        // The API router's `bind_ambient_user` scopes the caller; without it the
        // base would have to trust a user id from the params, which an override
        // could then spoof on the way in.
        let user = ferris_document::current_user().ok_or_else(|| {
            Error::Authz(AuthzError::PermissionDenied {
                action: "files.list".to_string(),
                resource: "file store".to_string(),
            })
        })?;
        // The caller's page size is honoured: the query has its own `LIMIT`,
        // so a bigger page is a bigger query, not a host hazard.
        let limit = params
            .get("limit")
            .and_then(JsonValue::as_u64)
            .unwrap_or(100)
            .max(1) as usize;
        let search = params
            .get("search")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let context = AccessContext::for_user(&user);
        let files = self
            .files
            .store()
            .list(&format!("{UPLOAD_FOLDER}/"), limit, &context)
            .await?;
        Ok(JsonValue::Array(
            files
                .into_iter()
                .filter(|file| search.is_empty() || file.filename.to_lowercase().contains(&search))
                .map(|file| file_json(self.files.store(), &file))
                .collect(),
        ))
    }
}

/// The handler the router registers as the base of [`FILES_LIST_METHOD`].
pub(crate) fn list_base(files: Arc<SignedDownloads>) -> Arc<dyn MethodHandler> {
    Arc::new(FilesListBase { files })
}

/// Query string of `GET /api/files`.
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Case-insensitive substring of the file name.
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// The Desk's file routes.
/// The Desk's file routes, with the shared store attached per request.
pub(crate) fn router(signed: Arc<SignedDownloads>) -> Router<ModuleState> {
    Router::new()
        .route("/files", get(list).post(upload))
        .route("/files/{id}", get(download).delete(remove))
        .layer(Extension(signed))
}

/// `GET /api/files?search=&limit=` — the files the caller may read.
///
/// A thin adapter over [`FILES_LIST_METHOD`]: the chain decides whether the
/// framework store or an app's override answers, and the wire shape the picker
/// consumes is the same either way.
async fn list(
    State(state): State<ModuleState>,
    user: OptionalUser,
    Query(query): Query<ListQuery>,
) -> Response {
    let Some(_user) = user.0 else {
        return unauthorized();
    };
    let params = JsonValue::Object(std::collections::HashMap::from([
        (
            "search".to_string(),
            JsonValue::String(query.search.unwrap_or_default()),
        ),
        (
            "limit".to_string(),
            JsonValue::Number((query.limit.unwrap_or(100) as u64).into()),
        ),
    ]));
    match state.api.methods.call(FILES_LIST_METHOD, params).await {
        Ok(rows) => Json(rows).into_response(),
        Err(error) => failure(&error),
    }
}

/// `POST /api/files` — raw bytes plus `x-file-name` and `content-type`.
///
/// The body is the file itself (no multipart parser: the browser can send a
/// Blob/ArrayBuffer directly and the server has one less parser to trust). The
/// store re-validates the real size and, for declared images, the magic bytes.
///
/// Uploads are **private** to the caller. An administrator may publish one to
/// the shared store with `x-file-public: 1`; every other value, and every other
/// role, is ignored rather than rejected, because the file still uploaded
/// correctly — it is only the visibility that stays restricted.
async fn upload(
    Extension(files): Extension<Arc<SignedDownloads>>,
    user: OptionalUser,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(user) = user.0 else {
        return unauthorized();
    };
    let filename = header_text(&headers, "x-file-name")
        .map(percent_decode)
        .unwrap_or_else(|| "archivo".to_string());
    let content_type = header_text(&headers, header::CONTENT_TYPE.as_str())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let size = body.len() as u64;
    let upload = FileUpload {
        filename,
        content_type,
        content: body.to_vec(),
        size,
        checksum: String::new(),
    };
    let options = desk_upload_options(&user, &headers);
    match files.store().upload(upload, options).await {
        Ok(file) => (StatusCode::CREATED, Json(file_json(files.store(), &file))).into_response(),
        Err(error) => failure(&error),
    }
}

/// The options every Desk upload uses: private unless an administrator asks for
/// the public header. Kept separate from the handler so the default is testable
/// without an HTTP request.
fn desk_upload_options(user: &User, headers: &HeaderMap) -> UploadOptions {
    let publish = header_text(headers, PUBLIC_UPLOAD_HEADER).as_deref() == Some("1")
        && can_publish_public(user);
    UploadOptions {
        folder: UPLOAD_FOLDER.to_string(),
        private: !publish,
        optimize: false,
        user_id: user.id.clone(),
        attached_to: Vec::new(),
    }
}

/// Whether `user` may publish an upload to the shared store.
///
/// The store's ACL treats `Administrator` and `System Manager` as the same
/// bypass ([`crate::AccessContext::for_user`]), so publishing follows the same
/// two names instead of inventing a third policy.
fn can_publish_public(user: &User) -> bool {
    user.has_any_role(&["Administrator", "System Manager"])
}

/// `GET /api/files/{id}` — the bytes, when the caller may read them.
///
/// The headers are built by [`download_response`], which is where the
/// same-origin policy lives: only four inert image types render inline.
async fn download(
    Extension(files): Extension<Arc<SignedDownloads>>,
    user: OptionalUser,
    Path(id): Path<String>,
) -> Response {
    let Some(user) = user.0 else {
        return unauthorized();
    };
    let context = AccessContext::for_user(&user);
    let file = match files.manager().get_file_ref(&id).await {
        Ok(Some(file)) => file,
        Ok(None) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
        Err(error) => return failure(&error),
    };
    match files.store().download(&file, &context).await {
        Ok(bytes) => download_response(&file, bytes),
        Err(error) => failure(&error),
    }
}

/// Content types whose bytes are inert when a browser renders them inline.
///
/// `image/svg+xml` is deliberately absent: an SVG is a document that can carry
/// script, so it is downloaded like HTML and JavaScript.
const INLINE_IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Types the response may echo to the browser. Anything else is an opaque
/// stream: a stored `text/html`, `application/javascript` or `image/svg+xml`
/// must never be interpreted in the Desk's origin.
const SERVABLE_TYPES: &[&str] = &[
    "application/json",
    "application/octet-stream",
    "application/pdf",
    "application/zip",
    "image/bmp",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
    "text/plain",
];

/// The base media type of a stored value, lowercased and without parameters.
///
/// The bytes are trusted, the declared type is not, so the decision is made on
/// the **stored** value and only after normalising it.
fn base_content_type(stored: &str) -> String {
    stored
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// The stored type if it is on the allowlist, otherwise an opaque blob.
fn response_content_type(stored: &str) -> &'static str {
    let base = base_content_type(stored);
    SERVABLE_TYPES
        .iter()
        .copied()
        .find(|servable| *servable == base.as_str())
        .unwrap_or("application/octet-stream")
}

/// `inline` only for the safe images; everything else is an attachment.
fn content_disposition(stored: &str, filename: &str) -> String {
    let base = base_content_type(stored);
    let kind = if INLINE_IMAGE_TYPES.contains(&base.as_str()) {
        "inline"
    } else {
        "attachment"
    };
    format!("{kind}; filename=\"{}\"", safe_header_text(filename))
}

/// Build the response for an authorized Desk download.
///
/// `nosniff` stops a browser from reinterpreting the bytes, `no-store` keeps a
/// private file out of shared caches, and the disposition keeps everything but
/// the four safe image types out of the Desk's origin.
fn download_response(file: &FileRef, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(response_content_type(&file.content_type)),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    if let Ok(value) =
        HeaderValue::from_str(&content_disposition(&file.content_type, &file.filename))
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

/// `DELETE /api/files/{id}` — remove a blob.
///
/// Only the uploader (or an administrator) may delete: the picker lists the
/// shared store, so a file another user attached is visible but not theirs to
/// erase. A file still referenced by a document is a conflict: deleting the
/// bytes would leave the document pointing at a hole, so the caller detaches
/// first. Returns `204` on success, so a caller can rely on an empty body.
async fn remove(
    Extension(files): Extension<Arc<SignedDownloads>>,
    user: OptionalUser,
    Path(id): Path<String>,
) -> Response {
    let Some(user) = user.0 else {
        return unauthorized();
    };
    let context = AccessContext::for_user(&user);
    let file = match files.manager().get_file_ref(&id).await {
        Ok(Some(file)) => file,
        Ok(None) => return (StatusCode::NOT_FOUND, "file not found").into_response(),
        Err(error) => return failure(&error),
    };
    if !may_delete(&file, &context) {
        return (
            StatusCode::FORBIDDEN,
            "only the uploader can delete this file",
        )
            .into_response();
    }
    // The manager repeats this guard before deleting (so a concurrent attach
    // cannot slip through), but answering here keeps the reason explicit.
    if !file.attached_to.is_empty() {
        return attached_conflict();
    }
    delete_response(files.manager().delete_file_ref(&id).await)
}

/// Only the uploader, or an administrator, may delete a file.
fn may_delete(file: &FileRef, ctx: &AccessContext) -> bool {
    ctx.is_admin || file.created_by == ctx.user_id
}

/// The `409` shared by the pre-check and the manager's own attachment guard.
fn attached_conflict() -> Response {
    (
        StatusCode::CONFLICT,
        "file is attached to a document; detach it first",
    )
        .into_response()
}

/// Map [`crate::FileReferenceManager::delete_file_ref`] to a response.
///
/// A still-attached file is the manager's `Validation` failure and becomes a
/// `409`; turning it into a `500` would tell the caller to retry a permanent
/// conflict.
fn delete_response(result: Result<bool>) -> Response {
    match result {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, "file not found").into_response(),
        Err(Error::Validation(_)) => attached_conflict(),
        // The row can disappear between the lookup above and the manager's
        // own read; that race is a 404, not an internal failure.
        Err(Error::File(ferris_core::FileError::NotFound(_))) => {
            (StatusCode::NOT_FOUND, "file not found").into_response()
        }
        Err(error) => failure(&error),
    }
}

/// The JSON the picker consumes: id, label, type, size, date and a URL the
/// browser can use directly (the session cookie authorises it).
///
/// The URL origin comes from the store ([`crate::FileStore::public_url`]), so
/// `[files].cdn_url` is applied without changing the route or its ACL.
fn file_json(store: &crate::FileStore, file: &FileRef) -> JsonValue {
    serde_json::from_value(json!({
        "id": file.id,
        "name": file.filename,
        "content_type": file.content_type,
        "size": file.size,
        "created_at": file.created_at.to_rfc3339(),
        "url": store.public_url(&file.id),
    }))
    .expect("a file row is JSON")
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Percent-decode a header value (`fetch` cannot put raw non-ASCII in headers).
fn percent_decode(value: String) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (hex(bytes.get(index + 1)), hex(bytes.get(index + 2)))
        {
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: Option<&u8>) -> Option<u8> {
    match byte.copied()? {
        b @ b'0'..=b'9' => Some(b - b'0'),
        b @ b'a'..=b'f' => Some(b - b'a' + 10),
        b @ b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Keep a filename usable inside `Content-Disposition` (no quotes, no CR/LF).
fn safe_header_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !matches!(ch, '"' | '\\' | '\r' | '\n'))
        .collect()
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "authentication required").into_response()
}

fn failure(error: &ferris_core::Error) -> Response {
    let status = match error {
        ferris_core::Error::Authz(_) | ferris_core::Error::Auth(_) => StatusCode::FORBIDDEN,
        ferris_core::Error::Validation(_) => StatusCode::BAD_REQUEST,
        ferris_core::Error::File(ferris_core::FileError::TooLarge { .. }) => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        ferris_core::Error::File(ferris_core::FileError::InvalidType(_)) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, error.to_string()).into_response()
}

/// Query string of `GET /files/{*path}`: the deadline and the HMAC the link
/// carried. Both absent/invalid → the request is treated as unauthenticated.
#[derive(Debug, Deserialize)]
struct SignedDownloadQuery {
    expires: Option<i64>,
    signature: Option<String>,
}

/// Serve a blob named by a signed, expiring URL.
///
/// For a **public** file the signature is the authorization: no session or ACL
/// is required, which is what lets an external system download an artifact.
/// A **private** file additionally needs the caller's `AccessContext`, which
/// the optional session supplies; a bare signature never grants a private read
/// (SEC-24). A missing or bad signature is answered `401`, a well-formed but
/// stale/forged one (or a private grant without ACL) `403`, and an unknown id
/// `404`.
async fn download_signed_file(
    Extension(files): Extension<Arc<SignedDownloads>>,
    user: OptionalUser,
    Path(path): Path<String>,
    Query(query): Query<SignedDownloadQuery>,
) -> Response {
    let (Some(expires), Some(signature)) = (query.expires, query.signature) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let context = user.0.as_ref().map(AccessContext::for_user);
    signed_download_response(
        files
            .fetch(&path, expires, &signature, context.as_ref())
            .await,
    )
}

/// Map a [`SignedDownloads`] outcome to an HTTP response.
///
/// Split out from the handler so the status mapping (403 for a forged/expired
/// signature, 404 for an unknown id, 200 with the bytes otherwise) is testable
/// without standing up a server.
fn signed_download_response(result: Result<crate::DownloadedFile>) -> Response {
    match result {
        Ok(file) => {
            // Strip anything that could inject a header; the name is metadata,
            // not a path, and the download body is the blob itself.
            let filename = file.filename.replace(['"', '\\', '\r', '\n'], "_");
            axum::response::Response::builder()
                .header(header::CONTENT_TYPE, file.content_type)
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("inline; filename=\"{filename}\""),
                )
                .body(axum::body::Body::from(file.bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(ferris_core::Error::File(ferris_core::FileError::NotFound(_))) => {
            StatusCode::NOT_FOUND.into_response()
        }
        Err(_) => StatusCode::FORBIDDEN.into_response(),
    }
}

#[async_trait::async_trait]
impl ferris_api::Module for crate::module::FilesModule {
    fn name(&self) -> &'static str {
        "files"
    }

    fn register_doctypes(&self) {
        crate::doctypes::register_doctypes();
    }

    async fn boot(&self, ctx: &ferris_api::ServiceContext) -> ferris_core::Result<()> {
        if !ctx.features.files {
            return Ok(());
        }
        let signed = build_signed_downloads(&ctx.db, &ctx.config.files).await;
        if self.signed.set(signed).is_err() {
            tracing::warn!("files: boot ran twice; keeping the first signed-downloads service");
        }
        Ok(())
    }

    fn register_methods(&self, methods: &ferris_api::MethodRegistry) {
        let Some(signed) = self.signed.get().and_then(Clone::clone) else {
            return;
        };
        if methods
            .register_base(FILES_LIST_METHOD, list_base(signed))
            .is_err()
        {
            tracing::warn!("files: `{FILES_LIST_METHOD}` is already registered");
        }
    }

    fn routes(&self) -> Router<ModuleState> {
        match self.signed.get().and_then(Clone::clone) {
            Some(signed) => router(signed),
            None => Router::new(),
        }
    }

    fn public_mount(&self) -> &'static str {
        "/files"
    }

    fn public_routes(&self) -> Router<ModuleState> {
        match self.signed.get().and_then(Clone::clone) {
            // The Desk API works without a signing secret; the public route
            // does not. Leaving it unmounted is the old `/files` behaviour: a
            // request without HMAC support is a 404, not a forged signature.
            Some(signed) if signed.signer().is_usable() => Router::new()
                .route("/{*path}", get(download_signed_file))
                .layer(Extension(signed)),
            _ => Router::new(),
        }
    }

    fn openapi(&self) -> ferris_api::OpenApiFragment {
        if self.signed.get().and_then(Clone::clone).is_none() {
            return ferris_api::OpenApiFragment::default();
        }
        ferris_api::OpenApiFragment {
            paths: vec![
                (
                    "/api/files".to_string(),
                    json!({
                        "get": {
                            "summary": "List the files the caller may read",
                            "tags": ["files"],
                            "responses": { "200": { "description": "File rows" } }
                        },
                        "post": {
                            "summary": "Upload raw bytes as a private file",
                            "description": "The file is private to the caller. `x-file-public: 1` publishes it only when the caller is an administrator.",
                            "tags": ["files"],
                            "parameters": [{"name":"x-file-public","in":"header","required":false,"schema":{"type":"string","enum":["1"]}}],
                            "responses": { "201": { "description": "Created file" } }
                        }
                    }),
                ),
                (
                    "/api/files/{id}".to_string(),
                    json!({
                        "get": {
                            "summary": "Download a file",
                            "tags": ["files"],
                            "parameters": [{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],
                            "responses": { "200": { "description": "File bytes" } }
                        },
                        "delete": {
                            "summary": "Delete a file uploaded by the caller",
                            "description": "Only the uploader or an administrator may delete. A file attached to a document answers 409.",
                            "tags": ["files"],
                            "parameters": [{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],
                            "responses": {
                                "204": { "description": "Deleted" },
                                "403": { "description": "Not the uploader" },
                                "409": { "description": "File is attached to a document" }
                            }
                        }
                    }),
                ),
            ],
        }
    }
}

/// Build the process-wide signed-downloads service from `[files]` and the
/// environment.
///
/// The backend, root, size/extension limits, optimization quality and CDN
/// origin come from the operator's `[files]` section (FILE-04); only the signing
/// secret stays environment-provided. A missing secret is not fatal: the Desk
/// API uses the store without HMACs and the public `/files` route stays
/// unmounted. A store that cannot open or initialise also leaves the routes
/// absent, with one warning.
async fn build_signed_downloads(
    pool: &ferris_db::Db,
    files: &ferris_core::FilesConfig,
) -> Option<Arc<SignedDownloads>> {
    let signer = crate::FileUrlSigner::from_env().unwrap_or_else(|| {
        tracing::warn!(
            "no FERRIS_FILES_SIGNING_SECRET: signed URLs disabled (the Desk file API still works)"
        );
        crate::FileUrlSigner::new(Vec::new(), std::time::Duration::from_secs(3600))
    });
    let store = match crate::FileStore::open_configured(files).await {
        Ok(store) => Arc::new(store.with_db(pool.clone())),
        Err(error) => {
            tracing::warn!(%error, "files: file store unavailable; routes are not mounted");
            return None;
        }
    };
    if let Err(error) = store.init().await {
        tracing::warn!(%error, "files: file schema init failed; routes are not mounted");
        return None;
    }
    let manager =
        Arc::new(crate::FileReferenceManager::new(pool.clone()).with_store(store.clone()));
    if let Err(error) = manager.init().await {
        tracing::warn!(%error, "files: file reference schema init failed; routes are not mounted");
        return None;
    }
    Some(Arc::new(SignedDownloads::new(store, manager, signer)))
}

#[cfg(test)]
mod tests;

/// Regression tests for the Desk file hardening (P0-2/FILE-01/02/03, SEC-02).
///
/// They live inline rather than in `http/tests.rs` because they cover the
/// policy helpers the handlers delegate to, not the signed-URL surface.
#[cfg(test)]
mod a2_tests {
    use super::*;
    use crate::store::{FileStore, StorageBackend, StoreConfig};
    use axum::http::HeaderName;
    use ferris_core::{UserType, ValidationError};

    fn user_with_roles(roles: &[&str]) -> User {
        let mut user = User::new(
            "user@example.com".to_string(),
            "User".to_string(),
            UserType::SystemUser,
        );
        for role in roles {
            user.add_role((*role).to_string());
        }
        user
    }

    fn file_ref(content_type: &str, filename: &str) -> FileRef {
        let now = chrono::Utc::now();
        FileRef {
            id: "f1".to_string(),
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            size: 2,
            checksum: String::new(),
            backend: StorageBackend::Memory,
            path: "uploads/f1.txt".to_string(),
            is_private: true,
            attached_to: Vec::new(),
            created_by: "owner@example.com".to_string(),
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    fn header(response: &Response, name: HeaderName) -> String {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    /// A stored HTML, JavaScript or SVG blob must never be rendered inline in
    /// the Desk's origin; only the four inert image types may be.
    #[test]
    fn desk_downloads_download_active_content_and_inline_only_safe_images() {
        for stored in [
            "text/html",
            "text/html; charset=utf-8",
            "application/javascript",
            "application/xhtml+xml",
            "image/svg+xml",
            "application/pdf",
            "text/plain",
        ] {
            let response = download_response(&file_ref(stored, "a.bin"), Vec::new());
            assert_eq!(
                header(&response, header::CONTENT_TYPE),
                if stored == "application/pdf" || stored == "text/plain" {
                    stored
                } else {
                    "application/octet-stream"
                },
                "stored type {stored} must not be echoed when it is not servable"
            );
            assert!(
                header(&response, header::CONTENT_DISPOSITION).starts_with("attachment;"),
                "stored type {stored} must be an attachment"
            );
            assert_eq!(header(&response, header::X_CONTENT_TYPE_OPTIONS), "nosniff");
            assert_eq!(
                header(&response, header::CACHE_CONTROL),
                "private, no-store"
            );
        }

        for stored in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            let response = download_response(&file_ref(stored, "photo"), Vec::new());
            assert_eq!(header(&response, header::CONTENT_TYPE), stored);
            assert!(
                header(&response, header::CONTENT_DISPOSITION).starts_with("inline;"),
                "safe image {stored} should render inline"
            );
        }
    }

    #[test]
    fn an_unknown_or_parameterised_type_is_an_opaque_stream() {
        assert_eq!(response_content_type(""), "application/octet-stream");
        assert_eq!(
            response_content_type("APPLICATION/XHTML+XML"),
            "application/octet-stream"
        );
        // The base type is normalised, so a parameter cannot smuggle a renderer.
        assert_eq!(
            response_content_type("image/png; charset=binary"),
            "image/png"
        );
        assert_eq!(
            response_content_type("text/html; charset=utf-8"),
            "application/octet-stream"
        );
    }

    #[test]
    fn desk_uploads_are_private_unless_an_admin_asks_for_public() {
        let plain = user_with_roles(&[]);
        let manager = user_with_roles(&["System Manager"]);

        let mut headers = HeaderMap::new();
        assert!(desk_upload_options(&plain, &headers).private);
        assert!(desk_upload_options(&manager, &headers).private);

        headers.insert(PUBLIC_UPLOAD_HEADER, HeaderValue::from_static("1"));
        assert!(
            desk_upload_options(&plain, &headers).private,
            "a normal user cannot publish a file by sending the header"
        );
        assert!(!desk_upload_options(&manager, &headers).private);

        headers.insert(PUBLIC_UPLOAD_HEADER, HeaderValue::from_static("true"));
        assert!(
            desk_upload_options(&manager, &headers).private,
            "only the literal 1 is an explicit request"
        );
    }

    /// A private Desk upload is invisible — metadata and bytes — to another
    /// signed-in user, while an administrator keeps the store's bypass.
    #[tokio::test]
    async fn a_second_user_neither_lists_nor_downloads_a_private_upload() {
        let db = ferris_db::memory_db().await.unwrap();
        let store = FileStore::new(StoreConfig {
            backend: StorageBackend::Memory,
            local_path: None,
            ..Default::default()
        })
        .await
        .unwrap()
        .with_db(db);
        store.init().await.unwrap();

        let owner = user_with_roles(&[]);
        let other = user_with_roles(&[]);
        let admin = user_with_roles(&["System Manager"]);

        let content = b"secret".to_vec();
        let uploaded = store
            .upload(
                FileUpload {
                    filename: "secret.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    size: content.len() as u64,
                    checksum: String::new(),
                    content,
                },
                desk_upload_options(&owner, &HeaderMap::new()),
            )
            .await
            .unwrap();
        assert!(uploaded.is_private, "a Desk upload must default to private");

        let other_ctx = AccessContext::for_user(&other);
        assert!(
            store
                .list("uploads/", 10, &other_ctx)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(store.download(&uploaded, &other_ctx).await.is_err());

        let owner_ctx = AccessContext::for_user(&owner);
        assert_eq!(
            store.list("uploads/", 10, &owner_ctx).await.unwrap().len(),
            1
        );
        assert_eq!(
            store.download(&uploaded, &owner_ctx).await.unwrap(),
            b"secret"
        );

        let admin_ctx = AccessContext::for_user(&admin);
        assert_eq!(
            store.list("uploads/", 10, &admin_ctx).await.unwrap().len(),
            1
        );
        assert_eq!(
            store.download(&uploaded, &admin_ctx).await.unwrap(),
            b"secret"
        );
    }

    #[test]
    fn a_reference_conflict_is_409_and_only_the_owner_or_admin_may_delete() {
        let owner = AccessContext::new("owner@example.com");
        let other = AccessContext::new("other@example.com");
        let admin = AccessContext::admin("admin@example.com");
        let file = file_ref("text/plain", "a.txt");

        assert!(may_delete(&file, &owner));
        assert!(!may_delete(&file, &other));
        assert!(may_delete(&file, &admin));

        assert_eq!(
            delete_response(Err(Error::Validation(ValidationError::Invalid(
                "attached".to_string()
            ))))
            .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(delete_response(Ok(false)).status(), StatusCode::NOT_FOUND);
        assert_eq!(delete_response(Ok(true)).status(), StatusCode::NO_CONTENT);
        assert_eq!(attached_conflict().status(), StatusCode::CONFLICT);
    }
}
