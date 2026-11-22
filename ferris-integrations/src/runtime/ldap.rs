//! LDAP directory adapters.
//!
//! [`config_from_settings`] reads the `LDAP Settings` Single and derivos the
//! concrete directory operations the runtime needs — the user's bind DN, the
//! search base and the search filter from the stored templates — with RFC 4515
//! filter escaping done in Rust. The real TCP/LDAP transport is a
//! [`LdapConnector`]: the crate ships the mapping and the contract, and a
//! host installs a connector over a directory client (the same seam pattern as
//! the [`Mailer`](super::state::Mailer)). Tests drive a fake connector against
//! the derived bind/search values, which pins the mapping exactly.

use ferris_core::{Error, Result, ValidationError, Value};

use self::ldap_esc::escape_filter_chars;

fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn bool_field(doc: &Value, key: &str, default: bool) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// The settings the directory needs, derived from the `LDAP Settings` Single.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LdapConfig {
    pub server_url: String,
    pub bind_dn: String,
    pub base_dn: String,
    pub user_search_filter: String,
    pub username_attribute: String,
    pub email_attribute: String,
    pub first_name_attribute: String,
    pub last_name_attribute: String,
    pub default_role: Option<String>,
    pub enabled: bool,
}

impl LdapConfig {
    /// Whether the Settings row enables the directory.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled && !self.server_url.is_empty() && !self.base_dn.is_empty()
    }

    /// The DN to bind as for `username`: the configured `bind_dn` template with
    /// `{0}` / `{username}` substituted.
    ///
    /// The `{…}` sequences are template placeholders for `String::replace`,
    /// not `format` arguments (`clippy::literal_string_with_formatting_args`).
    #[must_use]
    #[allow(clippy::literal_string_with_formatting_args)]
    pub fn user_dn(&self, username: &str) -> String {
        self.bind_dn
            .replace("{0}", username)
            .replace("{username}", username)
    }

    /// The subtree to search for `username` (the configured `base_dn`, with the
    /// same template substitutions so a per-user subtree can be configured).
    ///
    /// The `{…}` sequences are template placeholders for `String::replace`,
    /// not `format` arguments (`clippy::literal_string_with_formatting_args`).
    #[must_use]
    #[allow(clippy::literal_string_with_formatting_args)]
    pub fn search_base(&self, username: &str) -> String {
        self.base_dn
            .replace("{0}", username)
            .replace("{username}", username)
    }

    /// The directory filter to find `username`: the configured template (or a
    /// `(username_attribute=username)` default), with the username RFC 4515
    /// escaped and substituted for `{0}` / `{username}`.
    ///
    /// The `{…}` sequences are template placeholders for `String::replace`,
    /// not `format` arguments (`clippy::literal_string_with_formatting_args`).
    #[must_use]
    #[allow(clippy::literal_string_with_formatting_args)]
    pub fn user_filter(&self, username: &str) -> String {
        let escaped = escape_filter_chars(username);
        let attr = if self.username_attribute.is_empty() {
            "uid"
        } else {
            &self.username_attribute
        };
        if self.user_search_filter.is_empty() {
            format!("({attr}={escaped})")
        } else {
            self.user_search_filter
                .replace("{0}", &escaped)
                .replace("{username}", &escaped)
                .replace("{uid}", &escaped)
                .replace("{attribute}", attr)
        }
    }
}

/// The identity a successful bind/search produced, ready to become a Ferris
/// user (the auth plugin that consumes LDAP maps these onto a `User`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LdapIdentity {
    /// The username as the directory knows it (usually the bind DN or `uid`).
    pub user: String,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    /// Roles to grant, from `default_role` plus anything the directory
    /// returned.
    pub roles: Vec<String>,
}

/// The directory transport seam: a host over an `ldap3` client (or any LDAP
/// driver) authenticates `username`/`password` and returns the mapped identity.
#[async_trait::async_trait]
pub trait LdapConnector: Send + Sync {
    /// Bind with the user DN derived from `config` and (when the server asks
    /// for it) search for the user's attributes.
    async fn authenticate(
        &self,
        config: &LdapConfig,
        username: &str,
        password: &str,
    ) -> Result<LdapIdentity>;
}

/// Build the [`LdapConfig`] from an `LDAP Settings` Single row.
///
/// Returns `None` when the row is empty/missing (the runtime just skips LDAP
/// then); the row's own `enabled` flag is carried into the config so
/// [`LdapConfig::is_enabled`] still decides.
#[must_use]
pub fn config_from_settings(settings: &Value) -> Option<LdapConfig> {
    let obj = settings.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let enabled = bool_field(settings, "enabled", false);
    let server_url = str_field(settings, "ldap_server_url");
    if !enabled && server_url.is_empty() {
        return None;
    }
    let default_role = {
        let role = str_field(settings, "default_role");
        (!role.is_empty()).then_some(role)
    };
    Some(LdapConfig {
        server_url,
        bind_dn: str_field(settings, "bind_dn"),
        base_dn: str_field(settings, "base_dn"),
        user_search_filter: str_field(settings, "user_search_filter"),
        username_attribute: str_field(settings, "username_attribute"),
        email_attribute: str_field(settings, "user_email_attribute"),
        first_name_attribute: str_field(settings, "first_name_attribute"),
        last_name_attribute: str_field(settings, "last_name_attribute"),
        default_role,
        enabled,
    })
}

/// Apply a connector to a settings row: the one-call form hosts use.
///
/// # Errors
///
/// Returns `Err` when the Settings do not configure an enabled directory or
/// the credentials are blank, and propagates the connector's own errors.
pub async fn authenticate(
    connector: &dyn LdapConnector,
    settings: &Value,
    username: &str,
    password: &str,
) -> Result<LdapIdentity> {
    let Some(config) = config_from_settings(settings) else {
        return Err(Error::Validation(ValidationError::Invalid(
            "LDAP is not configured".to_string(),
        )));
    };
    if !config.is_enabled() {
        return Err(Error::Validation(ValidationError::Invalid(
            "LDAP is disabled".to_string(),
        )));
    }
    if username.is_empty() || password.is_empty() {
        return Err(Error::Validation(ValidationError::Invalid(
            "LDAP username and password are required".to_string(),
        )));
    }
    connector.authenticate(&config, username, password).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Value {
        Value::from_json(serde_json::json!({
            "enabled": true,
            "ldap_server_url": "ldap://directory.example:389",
            "bind_dn": "cn={0},ou=people,dc=example,dc=com",
            "base_dn": "dc=example,dc=com",
            "user_search_filter": "(&(objectClass=person)({attribute}={username}))",
            "username_attribute": "uid",
            "user_email_attribute": "mail",
            "first_name_attribute": "givenName",
            "last_name_attribute": "sn",
            "default_role": "LDAP User",
        }))
    }

    #[test]
    fn config_derives_dn_base_and_filter() {
        let config = config_from_settings(&settings()).expect("config");
        assert!(config.is_enabled());
        assert_eq!(config.user_dn("ada"), "cn=ada,ou=people,dc=example,dc=com");
        assert_eq!(config.search_base("ada"), "dc=example,dc=com");
        assert_eq!(
            config.user_filter("ada"),
            "(&(objectClass=person)(uid=ada))"
        );
        assert_eq!(config.default_role.as_deref(), Some("LDAP User"));
    }

    #[test]
    fn filter_escapes_rfc4515_specials() {
        let config = config_from_settings(&settings()).expect("config");
        // `*`, `(`, `)`, `\` and NUL must be escaped inside a filter value.
        assert_eq!(
            config.user_filter("a*(b)"),
            "(&(objectClass=person)(uid=a\\2a\\28b\\29))"
        );
    }

    #[test]
    fn missing_settings_row_is_not_configured() {
        assert!(config_from_settings(&Value::from_json(serde_json::json!({}))).is_none());
        // A disabled Settings row is still a config, just unusable.
        let mut row = settings();
        row.set("enabled".into(), Value::from(false));
        let config = config_from_settings(&row);
        assert!(config.is_some());
        assert!(!config.unwrap().is_enabled());
    }

    #[async_trait::async_trait]
    impl LdapConnector for RecordingConnector {
        async fn authenticate(
            &self,
            config: &LdapConfig,
            username: &str,
            password: &str,
        ) -> Result<LdapIdentity> {
            self.calls.lock().unwrap().push((
                config.user_dn(username),
                config.user_filter(username),
                password.to_string(),
            ));
            Ok(LdapIdentity {
                user: username.to_string(),
                email: Some("ada@example.com".to_string()),
                first_name: Some("Ada".to_string()),
                last_name: Some("Lovelace".to_string()),
                roles: config.default_role.clone().into_iter().collect(),
            })
        }
    }

    use std::sync::Mutex;

    struct RecordingConnector {
        calls: Mutex<Vec<(String, String, String)>>,
    }

    #[tokio::test]
    async fn authenticate_binds_with_derived_dn_and_filter() {
        let connector = RecordingConnector {
            calls: Mutex::new(Vec::new()),
        };
        let identity = authenticate(&connector, &settings(), "ada", "hunter2")
            .await
            .expect("authenticated");
        assert_eq!(identity.user, "ada");
        assert_eq!(identity.email.as_deref(), Some("ada@example.com"));
        assert_eq!(identity.roles, vec!["LDAP User".to_string()]);
        let calls = connector.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "cn=ada,ou=people,dc=example,dc=com");
        assert_eq!(calls[0].1, "(&(objectClass=person)(uid=ada))");
        assert_eq!(calls[0].2, "hunter2");
    }
}

/// RFC 4515 filtering helper, kept in its own module so both this file and
/// future directory code can reuse it without pulling in the whole adapter.
mod ldap_esc {
    /// Escape a filter assertion value per RFC 4515: `*`, `(`, `)`, `\` and
    /// NUL become their `\XX` hex forms.
    #[must_use]
    pub fn escape_filter_chars(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for byte in input.bytes() {
            match byte {
                b'*' => out.push_str("\\2a"),
                b'(' => out.push_str("\\28"),
                b')' => out.push_str("\\29"),
                b'\\' => out.push_str("\\5c"),
                0 => out.push_str("\\00"),
                _ => out.push(byte as char),
            }
        }
        out
    }
}
