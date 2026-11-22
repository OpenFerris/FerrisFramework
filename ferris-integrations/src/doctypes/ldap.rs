//! The `LDAP Settings` Single.

use ferris_schema::{DoctypeMeta, FieldMeta};

use super::common::{MODULE, system_manager_single};

/// Single: how Ferris binds to an LDAP directory.
#[must_use]
pub fn ldap_settings_meta() -> DoctypeMeta {
    DoctypeMeta::new("LDAP Settings", MODULE)
        .description("Directory binding and attribute mapping for LDAP login.")
        .single()
        .sort("modified", "desc")
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("0"),
        )
        .field(FieldMeta::data("ldap_server_url").label("LDAP Server URL"))
        .field(FieldMeta::data("bind_dn").label("Bind DN"))
        .field(FieldMeta::secret("bind_password").label("Bind Password"))
        .field(FieldMeta::data("base_dn").label("Base DN"))
        .field(FieldMeta::data("user_search_filter").label("User Search Filter"))
        .field(
            FieldMeta::data("username_attribute")
                .label("Username Attribute")
                .default_value("uid"),
        )
        .field(
            FieldMeta::data("user_email_attribute")
                .label("Email Attribute")
                .default_value("mail"),
        )
        .field(
            FieldMeta::data("first_name_attribute")
                .label("First Name Attribute")
                .default_value("givenName"),
        )
        .field(
            FieldMeta::data("last_name_attribute")
                .label("Last Name Attribute")
                .default_value("sn"),
        )
        .field(FieldMeta::link("default_role", "Role").label("Default Role"))
        .permission(system_manager_single())
}
