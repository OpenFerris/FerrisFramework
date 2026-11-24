//! The OAuth/SSO DocTypes: `Connected App`, `Social Login Key`, `OAuth Client`
//! and `OAuth Provider`.

use ferris_schema::{DoctypeMeta, FieldMeta};

use super::common::{MODULE, system_manager};

/// An OAuth application Ferris uses as a client (Frappe's "Connected App").
#[must_use]
pub fn connected_app_meta() -> DoctypeMeta {
    DoctypeMeta::new("Connected App", MODULE)
        .description("An external OAuth application Ferris authenticates against.")
        .title_field("app_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("app_name")
                .label("App Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("client_id").label("Client ID"))
        .field(FieldMeta::secret("client_secret").label("Client Secret"))
        .field(FieldMeta::url("authorization_uri").label("Authorization URI"))
        .field(FieldMeta::url("token_uri").label("Token URI"))
        .field(FieldMeta::url("userinfo_uri").label("Userinfo URI"))
        .field(FieldMeta::url("redirect_uri").label("Redirect URI"))
        .field(FieldMeta::text("scopes").label("Scopes"))
        .field(
            FieldMeta::select("grant_type", "Authorization Code\nImplicit")
                .label("Grant Type")
                .default_value("Authorization Code"),
        )
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .permission(system_manager())
}

/// A social sign-in provider (Google, GitHub, …).
#[must_use]
pub fn social_login_key_meta() -> DoctypeMeta {
    DoctypeMeta::new("Social Login Key", MODULE)
        .description("A social sign-in provider's OAuth credentials.")
        .title_field("provider_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("provider_name")
                .label("Provider Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("client_id").label("Client ID"))
        .field(FieldMeta::secret("client_secret").label("Client Secret"))
        .field(FieldMeta::url("authorization_uri").label("Authorization URI"))
        .field(FieldMeta::url("token_uri").label("Token URI"))
        .field(FieldMeta::url("userinfo_uri").label("Userinfo URI"))
        .field(FieldMeta::url("redirect_uri").label("Redirect URI"))
        .field(FieldMeta::text("scopes").label("Scopes"))
        .field(
            FieldMeta::select("sign_ups", "Allow\nDeny")
                .label("Sign Ups")
                .default_value("Allow"),
        )
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .permission(system_manager())
}

/// An OAuth2 client registered against this install.
#[must_use]
pub fn oauth_client_meta() -> DoctypeMeta {
    DoctypeMeta::new("OAuth Client", MODULE)
        .description("An OAuth2 client allowed to authenticate against this install.")
        .title_field("app_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("app_name")
                .label("App Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("client_id").label("Client ID"))
        .field(FieldMeta::secret("client_secret").label("Client Secret"))
        .field(FieldMeta::link("user", "User").label("User"))
        .field(FieldMeta::text("redirect_uris").label("Redirect URIs"))
        .field(FieldMeta::text("scopes").label("Scopes"))
        .field(FieldMeta::text("grant_types").label("Grant Types"))
        .field(FieldMeta::text("response_types").label("Response Types"))
        .field(
            FieldMeta::select(
                "token_endpoint_auth_method",
                "client_secret_basic\nclient_secret_post\nnone",
            )
            .label("Token Endpoint Auth Method")
            .default_value("client_secret_basic"),
        )
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .permission(system_manager())
}

/// This install acting as an OAuth2 provider to an external one.
#[must_use]
pub fn oauth_provider_meta() -> DoctypeMeta {
    DoctypeMeta::new("OAuth Provider", MODULE)
        .description("An external OAuth2 provider Ferris delegates to.")
        .title_field("provider_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("provider_name")
                .label("Provider Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("client_id").label("Client ID"))
        .field(FieldMeta::secret("client_secret").label("Client Secret"))
        .field(FieldMeta::url("authorization_uri").label("Authorization URI"))
        .field(FieldMeta::url("token_uri").label("Token URI"))
        .field(FieldMeta::url("userinfo_uri").label("Userinfo URI"))
        .field(FieldMeta::url("redirect_uri").label("Redirect URI"))
        .field(FieldMeta::text("scopes").label("Scopes"))
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .permission(system_manager())
}
