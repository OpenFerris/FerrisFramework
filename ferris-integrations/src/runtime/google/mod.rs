//! Google connector: refresh-token OAuth and the People/Calendar syncs.
//!
//! [`Google Settings`](super::names::GOOGLE_SETTINGS) is the Single holding the
//! credentials; the runtime refreshes an access token from `refresh_token` at
//! the token endpoint, then calls the People or Calendar API with it. The API
//! endpoints are constants here and parameters in the core functions, so tests
//! exercise the full HTTP surface against a local endpoint and production uses
//! Google's.
//!
//! Contact sync maps each People API connection to a
//! [`Google Contacts`](super::names::GOOGLE_CONTACTS) row (upserted on
//! `contact_name` + `google_account`); calendar sync does the same for
//! [`Google Calendar`](super::names::GOOGLE_CALENDAR) rows keyed on
//! `calendar_id`. No row is deleted — a de-activated contact simply stops
//! updating.
//!
//! [`config`] holds the endpoints and the shared `Value` accessors, [`api`] the
//! HTTP calls, [`sync`] the upserts and [`settings`] the Single reader, so each
//! concern can be read without the others.

mod api;
mod config;
mod settings;
mod sync;

#[cfg(test)]
mod tests;

pub use api::{GoogleContact, fetch_contacts, refresh_access_token};
pub use config::{GOOGLE_CALENDAR_LIST_URI, GOOGLE_PEOPLE_CONNECTIONS_URI, GOOGLE_TOKEN_URI};
pub use settings::settings;
pub use sync::{sync_calendars, sync_contacts};
