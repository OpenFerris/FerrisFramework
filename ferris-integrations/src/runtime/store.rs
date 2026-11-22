//! Thin, authorized document access for the integrations runtime.
//!
//! Everything reads and writes through [`ferris_document::DocumentEngine`] as
//! the context's service user (see [`super::state`]), so the DocTypes' declared
//! `System Manager` permissions gate every call exactly as they gate the API.

use ferris_core::secret::SecretValue;
use ferris_core::{Result, Value};
use ferris_document::ListQuery;
use ferris_schema::get_doctype;
use std::collections::HashMap;

use super::state::{IntegrationsContext, ValueMap};

/// List rows of `doctype` whose fields equal every `(field, value)` pair,
/// clamped to 1000 rows.
///
/// Equality on the declared fields is exactly what the runtime needs for
/// configuration lookups ("the enabled Webhooks for this DocType/event"). For a
/// test or a mis-registered DocType the list simply returns what the engine
/// stores; the caller filters booleans/enabled flags in Rust where a stored Check
/// could be `true` or `"1"` depending on the writer.
///
/// # Errors
///
/// Fails when the DocType is unknown to the engine or the list query fails.
pub async fn list(
    state: &IntegrationsContext,
    doctype: &str,
    filters: ValueMap,
) -> Result<Vec<Value>> {
    let meta = get_doctype(doctype);
    let ctx = state.ctx(doctype, meta.as_deref());
    let query = ListQuery {
        filters,
        limit: Some(1000),
        ..Default::default()
    };
    let (docs, _total) = state.engine().list(&ctx, &query).await?;
    Ok(docs)
}

/// Read `name` as the service user (requires `Permission::Read`).
///
/// # Errors
///
/// Fails when the row does not exist or the service user lacks `Read` on it.
pub async fn get(state: &IntegrationsContext, doctype: &str, name: &str) -> Result<Value> {
    let meta = get_doctype(doctype);
    let ctx = state.ctx(doctype, meta.as_deref());
    state.engine().get(&ctx, name).await
}

/// Create a document owned by `owner` (`create_owned_by`, the only supported
/// path for rows created *for* their subject — notifications, assignments).
///
/// # Errors
///
/// Fails when the DocType rejects the insert (validation, naming, duplicate)
/// or the database write fails.
pub async fn create_owned_by(
    state: &IntegrationsContext,
    doctype: &str,
    owner: &str,
    data: Value,
) -> Result<Value> {
    let meta = get_doctype(doctype);
    let ctx = state.ctx(doctype, meta.as_deref());
    state.engine().create_owned_by(&ctx, owner, data).await
}

/// Create a document owned by the service user.
///
/// # Errors
///
/// Fails when the DocType rejects the insert or the database write fails.
pub async fn create(state: &IntegrationsContext, doctype: &str, data: Value) -> Result<Value> {
    let meta = get_doctype(doctype);
    let ctx = state.ctx(doctype, meta.as_deref());
    state.engine().create(&ctx, data).await
}

/// Update `name` with `data` as the service user.
///
/// # Errors
///
/// Fails when the row does not exist, the service user lacks `Write`, or the
/// update is rejected (a submitted/cancelled document, validation).
pub async fn update(
    state: &IntegrationsContext,
    doctype: &str,
    name: &str,
    data: Value,
) -> Result<Value> {
    let meta = get_doctype(doctype);
    let ctx = state.ctx(doctype, meta.as_deref());
    state.engine().update(&ctx, name, data).await
}

/// Whether `doctype` is registered in the schema registry.
#[must_use]
pub fn is_registered(doctype: &str) -> bool {
    get_doctype(doctype).is_some()
}

/// The plaintext of `fields` on the row `name` — the server-side `SecretStore`.
///
/// Reads go through [`IntegrationsContext::ctx_revealed`] and
/// `DocumentEngine::read_secrets`, so the plaintext is a [`SecretValue`] and
/// never enters a document that could be logged. Absent/empty fields are omitted.
///
/// # Errors
///
/// Fails for the same reasons as [`get`], plus the typed `SecretError` when a
/// stored value cannot be recovered (an Argon2 hash from before the field was
/// migrated, a retired key that is no longer installed, or tampering).
pub async fn secrets(
    state: &IntegrationsContext,
    doctype: &str,
    name: &str,
    fields: &[&str],
) -> Result<HashMap<String, SecretValue>> {
    let meta = get_doctype(doctype);
    let ctx = state.ctx_revealed(doctype, meta.as_deref());
    state.engine().read_secrets(&ctx, name, fields).await
}

/// [`secrets`] for the one document of a Single DocType.
///
/// # Errors
///
/// Fails when `doctype` is not a Single or the values cannot be recovered; see
/// [`secrets`].
pub async fn single_secrets(
    state: &IntegrationsContext,
    doctype: &str,
    fields: &[&str],
) -> Result<HashMap<String, SecretValue>> {
    let meta = get_doctype(doctype);
    let ctx = state.ctx_revealed(doctype, meta.as_deref());
    state.engine().read_single_secrets(&ctx, fields).await
}
