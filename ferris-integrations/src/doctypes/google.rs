//! The Google connector DocTypes: `Google Contacts`, `Google Calendar` and the
//! `Google Settings` Single.

use ferris_schema::{DoctypeMeta, FieldMeta};

use super::common::{MODULE, system_manager, system_manager_single};

/// A contact imported from Google Contacts.
#[must_use]
pub fn google_contacts_meta() -> DoctypeMeta {
    DoctypeMeta::new("Google Contacts", MODULE)
        .description("A contact synced from Google Contacts.")
        .title_field("contact_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(
            FieldMeta::data("contact_name")
                .label("Contact Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("email").label("Email").in_list_view())
        .field(FieldMeta::data("phone").label("Phone"))
        .field(FieldMeta::data("google_account").label("Google Account"))
        .field(FieldMeta::datetime("synced_at").label("Synced At"))
        .permission(system_manager())
}

/// A calendar synced from Google Calendar.
#[must_use]
pub fn google_calendar_meta() -> DoctypeMeta {
    DoctypeMeta::new("Google Calendar", MODULE)
        .description("A calendar synced from Google Calendar.")
        .title_field("calendar_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(
            FieldMeta::data("calendar_name")
                .label("Calendar Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("calendar_id").label("Calendar ID"))
        .field(FieldMeta::link("user", "User").label("User").in_list_view())
        .field(FieldMeta::check("sync").label("Sync").default_value("1"))
        .field(FieldMeta::datetime("last_synced_on").label("Last Synced On"))
        .permission(system_manager())
}

/// Single: the OAuth credentials Ferris uses to reach Google.
#[must_use]
pub fn google_settings_meta() -> DoctypeMeta {
    DoctypeMeta::new("Google Settings", MODULE)
        .description("OAuth credentials for the Google connectors.")
        .single()
        .sort("modified", "desc")
        .field(
            FieldMeta::check("enable")
                .label("Enable")
                .default_value("1"),
        )
        .field(FieldMeta::data("client_id").label("Client ID"))
        .field(FieldMeta::secret("client_secret").label("Client Secret"))
        .field(FieldMeta::data("redirect_uri").label("Redirect URI"))
        .field(FieldMeta::secret("refresh_token").label("Refresh Token"))
        .field(FieldMeta::link("user", "User").label("User"))
        .permission(system_manager_single())
}
