//! Reading the `Google Settings` Single and its decrypted credentials.

use std::collections::HashMap;

use ferris_core::secret::SecretValue;
use ferris_core::{Result, Value};

use crate::runtime::names::GOOGLE_SETTINGS;
use crate::runtime::state::IntegrationsContext;
use crate::runtime::store;

/// The `Google Settings` Single as a stored `Value` (enabled or not), together
/// with its decrypted credentials.
///
/// The document keeps only the read mask for `client_secret`/`refresh_token`;
/// the plaintext comes from [`store::single_secrets`] and stays a
/// [`SecretValue`]. A missing Single yields an empty document and an empty map.
///
/// # Errors
///
/// Fails when the Single cannot be read or when a stored credential cannot be
/// decrypted (see `SecretError`); a missing key is *not* an error while the
/// fields are unset.
pub async fn settings(
    state: &IntegrationsContext,
) -> Result<(Value, HashMap<String, SecretValue>)> {
    let meta = ferris_schema::get_doctype(GOOGLE_SETTINGS);
    let ctx = state.ctx(GOOGLE_SETTINGS, meta.as_deref());
    let engine = state.engine();
    if engine.get_single(&ctx).await.is_err() {
        return Ok((Value::Object(HashMap::new()), HashMap::new()));
    }
    let document = engine.get_single(&ctx).await?;
    let secrets =
        store::single_secrets(state, GOOGLE_SETTINGS, &["client_secret", "refresh_token"]).await?;
    Ok((document, secrets))
}
