//! Public locale endpoints.
//!
//! The Desk needs the catalogue before it has a session (the login page is the
//! first screen a visitor sees), so these routes live outside the auth layers,
//! next to `/api/auth/login`. They expose only the static messages the binary
//! already embeds: no user data, no database access, nothing worth guarding.
//!
//! `GET /api/i18n` advertises `{default, supported}`; `GET /api/i18n/{locale}`
//! returns the effective catalogue (English base plus the locale's overrides),
//! so a client never has to merge two payloads or guess a fallback.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferris_core::i18n::Locale;

use super::state::AppState;

/// `GET /api/i18n` — the locale policy.
pub(super) async fn available(State(app): State<AppState>) -> Json<serde_json::Value> {
    Json(app.i18n.available_json())
}

/// `GET /api/i18n/{locale}` — the effective catalogue for one locale.
///
/// A malformed tag is `400`; a well-formed locale the deployment does not
/// advertise is `404`. Neither is an auth failure, so the client can show the
/// right screen instead of a login redirect.
pub(super) async fn catalog(State(app): State<AppState>, Path(tag): Path<String>) -> Response {
    let Some(locale) = Locale::parse(&tag) else {
        return (StatusCode::BAD_REQUEST, "invalid locale tag").into_response();
    };
    if !app.i18n.supported().contains(&locale) {
        return (StatusCode::NOT_FOUND, "locale is not advertised").into_response();
    }
    Json(app.i18n.catalog_json(&locale)).into_response()
}
