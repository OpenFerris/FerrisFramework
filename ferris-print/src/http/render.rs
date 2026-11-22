//! Document rendering: the permission gate, settings and the blocking call.
//!
//! Typst compilation is synchronous, CPU-bound and **cannot be cancelled**:
//! `spawn_blocking` has no abort, so a `#while true {}` in a template would run
//! until the thread returns. The request paths therefore go through
//! [`render_with_limits`], which owns the only three knobs that make that safe:
//! a semaphore caps how many renders exist at once, a deadline bounds how long
//! a request waits, and dropping the join future lets the request answer
//! `Validation` while the runaway thread keeps its slot instead of multiplying
//! (PRINT-01). The work itself is also bounded: `scale` and the PNG pixel
//! ceiling are validated by the engine's `ResolvedPrint` for every path,
//! preview included.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use ferris_core::{
    AuthzError, Error, Permission, Result, User, ValidationError, Value,
    permissions::{ALL_USER_ROLE, SYSTEM_MANAGER_ROLE},
};
use ferris_document::{DocumentEngine, EngineContext};
use ferris_schema::DoctypeMeta;
use tokio::sync::Semaphore;

use crate::{
    PrintSettings,
    http::{
        formats::{PRINT_SETTINGS, load_format},
        host::PrintHost,
        request::{PrintOutput, PrintRequest, to_value},
    },
};

/// Renders allowed to compile at the same time.
///
/// Two keeps a multi-core host busy and still leaves room for the request
/// handlers; a larger value only multiplies the memory one hostile layout can
/// claim, because a stuck render keeps its permit (see the module docs).
const MAX_CONCURRENT_RENDERS: usize = 2;

/// How long a request waits for a free render slot.
const RENDER_QUEUE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long one render may take.
const RENDER_TIMEOUT: Duration = Duration::from_secs(30);

/// Process-wide slots. One semaphore for every entry point, so a preview and a
/// document render compete for the same budget instead of each having its own.
static RENDER_SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_RENDERS)));

/// Render `format` under the shared slot budget and deadline.
///
/// # Errors
///
/// A validation error when no slot is free within [`RENDER_QUEUE_TIMEOUT`], the
/// deadline expires, the blocking task panics, or the engine rejects the format
/// (including an invalid `scale` or a PNG over the pixel ceiling).
pub async fn render_with_limits(
    host: &PrintHost,
    format: &crate::PrintFormatDef,
    data: &Value,
    options: &crate::PrintOptions,
) -> Result<Vec<u8>> {
    let engine = host.engine.clone();
    let format = format.clone();
    let data = data.clone();
    let options = options.clone();
    run_bounded(RENDER_TIMEOUT, move || {
        engine.render_format(&format, &data, &options)
    })
    .await
}

/// Run `job` on the blocking pool with a slot and a deadline.
async fn run_bounded<T, F>(deadline: Duration, job: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let slots = Arc::clone(&RENDER_SLOTS);
    let permit = tokio::time::timeout(RENDER_QUEUE_TIMEOUT, slots.acquire_owned())
        .await
        .map_err(|_| busy())?
        .map_err(|_| busy())?;

    // The permit is moved into the blocking task on purpose: if the render
    // ignores the deadline, the thread that is still burning CPU keeps its
    // slot, so at most `MAX_CONCURRENT_RENDERS` runaway renders exist and the
    // rest of the server keeps working.
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        job()
    });
    match tokio::time::timeout(deadline, task).await {
        Ok(joined) => joined.map_err(|error| {
            Error::Validation(ValidationError::Invalid(format!(
                "print task failed: {error}"
            )))
        })?,
        Err(_) => Err(Error::Validation(ValidationError::Invalid(format!(
            "print render exceeded the {} s deadline",
            deadline.as_secs()
        )))),
    }
}

fn busy() -> Error {
    Error::Validation(ValidationError::Invalid(
        "too many print renders in progress; retry shortly".to_string(),
    ))
}

/// Render `doctype/name` as `user`.
///
/// # Errors
///
/// A permission error when the caller may not print the document, a validation
/// error for an invalid or unsupported option, or the Typst diagnostic.
pub async fn render_document(
    host: &PrintHost,
    user: &User,
    doctype: &str,
    name: &str,
    request: &PrintRequest,
) -> Result<PrintOutput> {
    let mut options = request.options()?;
    let output_format = options.format;
    let settings = load_print_settings(host).await?;
    let format = load_format(host, doctype, request.print_format.as_deref(), settings).await?;

    let meta = ferris_api::store::effective_meta(&host.api.pool, doctype).await?;
    let mut ctx = EngineContext::new(doctype.to_string(), "");
    ctx.meta = meta.as_ref();
    ctx.user = Some(user.clone());
    let document = DocumentEngine::new(host.api.pool.clone())
        // `Print`, not `Read`: a caller who may see the document is not
        // automatically allowed to produce a printable copy of it.
        .get_for(&ctx, name, Permission::Print)
        .await?;

    options.meta = meta.as_ref().map(to_value).transpose()?;
    options.user = Some(user_context(user));

    let bytes = render_with_limits(host, &format, &document, &options).await?;

    Ok(PrintOutput {
        format: output_format,
        bytes,
    })
}

/// Read the `Print Settings` Single (materialising it with defaults on first
/// access). A malformed field falls back to its default inside
/// [`PrintSettings::from_value`], so printing is never unavailable because of a
/// half-edited settings row.
pub async fn load_print_settings(host: &PrintHost) -> Result<PrintSettings> {
    let row = ferris_api::DocStore::get_single_system(&host.api.pool, PRINT_SETTINGS).await?;
    Ok(PrintSettings::from_value(&row))
}

/// Whether `user` holds the `DocType`-level `print` right.
///
/// A preview has no document to authorize, so it checks the `DocType` policy
/// instead of instantiating a row. The render itself still runs under the same
/// limits as a real print.
pub fn can_print_doctype(user: &User, meta: &DoctypeMeta) -> bool {
    if user.has_role(SYSTEM_MANAGER_ROLE) {
        return true;
    }
    meta.permissions.iter().any(|rule| {
        rule.print && (rule.role == ALL_USER_ROLE || user.roles.iter().any(|r| r == &rule.role))
    })
}

pub fn deny_print(doctype: &str) -> Error {
    Error::Authz(AuthzError::PermissionDenied {
        action: "print".to_string(),
        resource: doctype.to_string(),
    })
}

/// The user shape a template may see.
///
/// Deliberately not the full [`User`]: `password_hash`, `salt`, `mfa_secret`
/// and lockout state have no place in a printable document, and a template is
/// user-authored. Expose only identity and roles.
pub fn user_context(user: &User) -> Value {
    Value::from_json(serde_json::json!({
        "id": user.id.clone(),
        "email": user.email.clone(),
        "username": user.username.clone(),
        "full_name": user.full_name.clone(),
        "roles": user.roles.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A render that never returns must be answered by the deadline, not by
    /// waiting for the thread. The blocking job sleeps past the deadline on
    /// purpose: `spawn_blocking` cannot be aborted, so this is the only way to
    /// prove the request path gives up instead of hanging (PRINT-01).
    #[tokio::test]
    async fn a_runaway_render_hits_the_deadline() {
        let started = Instant::now();
        let error = run_bounded(Duration::from_millis(20), || -> Result<()> {
            std::thread::sleep(Duration::from_secs(1));
            Ok(())
        })
        .await
        .expect_err("the deadline must fire");
        assert!(error.to_string().contains("deadline"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the request was answered while the render thread was still busy"
        );
    }
}
