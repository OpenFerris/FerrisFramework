//! Email account configuration and management

use crate::smtp::{SmtpClient, SmtpConfig};
use chrono::{DateTime, Utc};
use ferris_core::{EmailError, Error, Result, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Email account configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailAccountConfig {
    pub name: String,
    pub email: String,
    pub from_email: Option<String>,
    pub display_name: Option<String>,
    pub smtp: SmtpConfig,
    pub imap: Option<ImapConfig>,
    pub default: bool,
    pub enabled: bool,
    pub signature: Option<String>,
    pub auto_bcc: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: HashMap<String, Value>,
}

impl EmailAccountConfig {
    pub fn new(name: String, email: String, smtp: SmtpConfig) -> Self {
        let now = Utc::now();
        Self {
            name,
            email,
            from_email: None,
            display_name: None,
            smtp,
            imap: None,
            default: false,
            enabled: true,
            signature: None,
            auto_bcc: None,
            created_at: now,
            updated_at: now,
            metadata: HashMap::new(),
        }
    }

    pub fn with_display_name(mut self, name: String) -> Self {
        self.display_name = Some(name);
        self
    }

    pub fn with_imap(mut self, imap: ImapConfig) -> Self {
        self.imap = Some(imap);
        self
    }

    pub fn with_signature(mut self, signature: String) -> Self {
        self.signature = Some(signature);
        self
    }

    pub fn with_auto_bcc(mut self, bcc: String) -> Self {
        self.auto_bcc = Some(bcc);
        self
    }

    pub fn with_from_email(mut self, from_email: String) -> Self {
        self.from_email = Some(from_email);
        self
    }

    pub fn as_default(mut self) -> Self {
        self.default = true;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// IMAP configuration lives with the client (`crate::imap`); re-exported here
/// so `EmailAccountConfig` and the server keep one import path.
pub use crate::imap::ImapConfig;

/// Email account with client
#[derive(Debug, Clone)]
pub struct EmailAccount {
    pub config: EmailAccountConfig,
    pub client: Option<SmtpClient>,
}

impl EmailAccount {
    pub fn new(config: EmailAccountConfig) -> Result<Self> {
        let client = if config.enabled {
            Some(SmtpClient::new(config.smtp.clone())?)
        } else {
            None
        };

        Ok(Self { config, client })
    }

    /// Get the from email address (falls back to email if not set)
    pub fn from_email(&self) -> String {
        self.config
            .from_email
            .clone()
            .unwrap_or_else(|| self.config.email.clone())
    }

    /// Resolve the effective sender address, falling back to the account's
    /// `from_email` (or its address) when `requested` is empty.
    ///
    /// Prevents the SMTP layer from failing when a job was queued without an
    /// explicit sender.
    pub fn resolve_from(&self, requested: &str) -> String {
        if requested.is_empty() {
            self.from_email()
        } else {
            requested.to_string()
        }
    }

    /// Create SMTP client
    pub fn create_client(&self) -> Result<SmtpClient> {
        SmtpClient::new(self.config.smtp.clone())
    }

    /// Get sender string (`Display Name <email>`).
    ///
    /// Control characters (CR/LF, header-injection vector) are stripped and the
    /// display name is quoted/escaped when it contains RFC 5322 specials.
    pub fn sender_string(&self) -> String {
        if let Some(name) = &self.config.display_name {
            let sanitized: String = name.chars().filter(|c| !c.is_control()).collect();
            let specials = sanitized.chars().any(|c| {
                matches!(
                    c,
                    '(' | ')' | '<' | '>' | '[' | ']' | ':' | ';' | '@' | '\\' | ',' | '"'
                )
            });
            if specials {
                let escaped = sanitized.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{}\" <{}>", escaped, self.config.email)
            } else {
                format!("{} <{}>", sanitized, self.config.email)
            }
        } else {
            self.config.email.clone()
        }
    }

    /// Check if account is default
    pub fn is_default(&self) -> bool {
        self.config.default
    }

    /// Check if account is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

/// Account manager
pub struct AccountManager {
    accounts: HashMap<String, EmailAccount>,
}

impl AccountManager {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
        }
    }

    /// Add account
    pub fn add_account(&mut self, account: EmailAccount) {
        self.accounts.insert(account.config.name.clone(), account);
    }

    /// Get account by name
    pub fn get_account(&self, name: &str) -> Option<EmailAccount> {
        self.accounts.get(name).cloned()
    }

    /// Get default account
    pub fn get_default(&self) -> Option<EmailAccount> {
        self.accounts
            .values()
            .find(|a| a.config.default && a.config.enabled)
            .cloned()
    }

    /// Get account for email address
    pub fn get_by_email(&self, email: &str) -> Option<EmailAccount> {
        self.accounts
            .values()
            .find(|a| a.config.email == email)
            .cloned()
    }

    /// List all accounts
    pub fn list_accounts(&self) -> Vec<EmailAccount> {
        self.accounts.values().cloned().collect()
    }

    /// Remove account
    pub fn remove_account(&mut self, name: &str) -> bool {
        self.accounts.remove(name).is_some()
    }

    /// Set default account
    pub fn set_default(&mut self, name: &str) -> Result<()> {
        // Clear current default
        for account in self.accounts.values_mut() {
            account.config.default = false;
        }

        // Set new default
        if let Some(account) = self.accounts.get_mut(name) {
            account.config.default = true;
            Ok(())
        } else {
            Err(Error::Email(EmailError::NotFound(format!(
                "Account {} not found",
                name
            ))))
        }
    }
}

impl Default for AccountManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_config() {
        let smtp = SmtpConfig::default();
        let config =
            EmailAccountConfig::new("test".to_string(), "test@example.com".to_string(), smtp)
                .with_display_name("Test User".to_string());

        assert_eq!(config.name, "test");
        assert_eq!(config.email, "test@example.com");
        assert_eq!(config.display_name, Some("Test User".to_string()));
    }

    #[test]
    fn test_sender_string() {
        let smtp = SmtpConfig::default();
        let config =
            EmailAccountConfig::new("test".to_string(), "test@example.com".to_string(), smtp)
                .with_display_name("Test User".to_string());

        // Build the account without an SMTP client: constructing a lettre pool
        // requires a Tokio runtime, which this sync test does not have.
        let account = EmailAccount {
            config,
            client: None,
        };
        assert_eq!(account.sender_string(), "Test User <test@example.com>");
    }

    #[test]
    fn test_resolve_from() {
        let smtp = SmtpConfig::default();
        let config =
            EmailAccountConfig::new("test".to_string(), "test@example.com".to_string(), smtp)
                .with_from_email("sender@example.com".to_string());
        let account = EmailAccount {
            config,
            client: None,
        };

        assert_eq!(account.resolve_from(""), "sender@example.com");
        assert_eq!(
            account.resolve_from("override@example.com"),
            "override@example.com"
        );
    }

    #[test]
    fn test_resolve_from_falls_back_to_email() {
        let smtp = SmtpConfig::default();
        let config =
            EmailAccountConfig::new("test".to_string(), "test@example.com".to_string(), smtp);
        let account = EmailAccount {
            config,
            client: None,
        };

        assert_eq!(account.resolve_from(""), "test@example.com");
    }
}
