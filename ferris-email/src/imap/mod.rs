//! IMAP client for inbound mail.
//!
//! This is a deliberately small client: it connects (implicit TLS or
//! STARTTLS), authenticates with `LOGIN`, selects one mailbox, searches the
//! unseen UIDs, fetches each message's RFC822 source, flags the ones the
//! caller confirms it stored, and logs out. There is no IDLE support — the
//! poller reconnects on a timer — because IDLE needs a second connection or a
//! command/mailbox state machine for a benefit (sub-minute latency) nobody has
//! asked for. The limitation is documented on [`crate::inbound::ImapPoller`].
//!
//! # Why not the `imap` crate
//!
//! The protocol surface used here is four commands and one response shape
//! (untagged `FETCH` with a literal). A dependency that models the whole RFC
//! 3501 grammar (and carries its own TLS/connection policy) would add more
//! behaviour than this crate exercises, and every TLS decision would still have
//! to be ours: the workspace pins `rustls` with the `ring` provider and refuses
//! `native-tls`. The parser below is therefore local, literal-aware and tested
//! against a scripted TCP server.
//!
//! # Security
//!
//! * The password is redacted in `Debug`; it is never logged.
//! * A mailbox is required to be encrypted: implicit TLS or STARTTLS. The
//!   `allow_plaintext` escape hatch mirrors SMTP and exists for loopback tests
//!   and an operator who explicitly accepts the risk.
//! * Every response line and every message literal is bounded, so a hostile or
//!   broken server cannot make the poller allocate without limit.

mod client;
mod protocol;

pub use client::{FetchedMessage, ImapClient, ImapSession};
pub use protocol::{Response, Status, Token};

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Default ceiling for one message's RFC822 source (25 MiB).
pub const DEFAULT_MAX_MESSAGE_BYTES: u64 = 25 * 1024 * 1024;

/// IMAP connection settings for one mailbox.
///
/// The shape follows the SMTP config and the `[email.accounts]` keys the server
/// already reads, so enabling inbound mail does not introduce a second
/// vocabulary.
#[derive(Clone, Serialize, Deserialize)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// Implicit TLS from the first byte (conventionally port 993).
    pub use_tls: bool,
    /// Upgrade a clear-text connection with STARTTLS (conventionally port 143).
    /// Mutually exclusive with `use_tls`.
    pub use_starttls: bool,
    pub timeout_secs: u64,
    /// Mailbox selected for polling. Empty means `INBOX`.
    #[serde(default)]
    pub mailbox: String,
    /// Reject (without storing) a message whose RFC822 source exceeds this
    /// many bytes. The message stays unseen instead of being dropped silently.
    #[serde(default = "default_max_message_bytes")]
    pub max_message_bytes: u64,
    /// Explicitly accept an unencrypted connection. Only for loopback tests or
    /// an operator who has decided the network is trusted; every real mailbox
    /// should use `use_tls` or `use_starttls`.
    #[serde(default)]
    pub allow_plaintext: bool,
}

const fn default_max_message_bytes() -> u64 {
    DEFAULT_MAX_MESSAGE_BYTES
}

impl Default for ImapConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 993,
            username: String::new(),
            password: String::new(),
            use_tls: true,
            use_starttls: false,
            timeout_secs: 30,
            mailbox: String::new(),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            allow_plaintext: false,
        }
    }
}

/// The credential is redacted: this struct is carried by `EmailAccountConfig`
/// and the server's boot log, and its derived `Debug` used to print the
/// password. `Serialize` still emits it — it is written to config on purpose.
impl fmt::Debug for ImapConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("use_tls", &self.use_tls)
            .field("use_starttls", &self.use_starttls)
            .field("timeout_secs", &self.timeout_secs)
            .field("mailbox", &self.effective_mailbox())
            .field("max_message_bytes", &self.max_message_bytes)
            .field("allow_plaintext", &self.allow_plaintext)
            .finish()
    }
}

impl ImapConfig {
    /// The mailbox to select; `INBOX` when none is configured.
    // `str::trim` is not `const`, so this cannot be a `const fn` even though
    // the lint suggests it.
    #[allow(clippy::missing_const_for_fn)]
    #[must_use]
    pub fn effective_mailbox(&self) -> &str {
        let mailbox = self.mailbox.trim();
        if mailbox.is_empty() { "INBOX" } else { mailbox }
    }

    /// Reject a configuration the client cannot honour.
    ///
    /// # Errors
    ///
    /// [`ImapError::Config`] for an empty host/username/password, a port of
    /// zero, control characters (header-command injection), or a transport
    /// selection that contradicts itself or leaves the session in clear.
    pub fn validate(&self) -> Result<(), ImapError> {
        if self.host.trim().is_empty() {
            return Err(ImapError::Config("host is empty".to_string()));
        }
        if self
            .host
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(ImapError::Config(
                "host contains whitespace or control characters".to_string(),
            ));
        }
        if self.port == 0 {
            return Err(ImapError::Config("port is zero".to_string()));
        }
        if self.username.is_empty() {
            return Err(ImapError::Config("username is empty".to_string()));
        }
        for (field, value) in [("username", &self.username), ("password", &self.password)] {
            if value.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
                return Err(ImapError::Config(format!(
                    "{field} contains a control character"
                )));
            }
        }
        if self.use_tls && self.use_starttls {
            return Err(ImapError::Config(
                "`use_tls` (implicit TLS, usually port 993) and `use_starttls` \
                 (upgrade, usually port 143) are mutually exclusive"
                    .to_string(),
            ));
        }
        if !self.use_tls && !self.use_starttls && !self.allow_plaintext {
            return Err(ImapError::Config(
                "IMAP is not encrypted: enable use_tls/use_starttls or set \
                 allow_plaintext=true"
                    .to_string(),
            ));
        }
        let mailbox = self.effective_mailbox();
        if mailbox.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
            return Err(ImapError::Config(
                "mailbox contains a control character".to_string(),
            ));
        }
        if self.max_message_bytes == 0 {
            return Err(ImapError::Config(
                "max_message_bytes must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }
}

/// Typed IMAP failures.
///
/// A local error type instead of a new `ferris_core::EmailError` variant: this
/// crate is the only producer, the poller matches on the variants to decide
/// whether to retry, and `ferris-core` is not this change's surface. The
/// server logs `Display`; nothing downcasts it.
#[derive(Debug, Error)]
pub enum ImapError {
    #[error("IMAP configuration is invalid: {0}")]
    Config(String),
    #[error("IMAP connection to {host}:{port} failed: {source}")]
    Connect {
        host: String,
        port: u16,
        source: std::io::Error,
    },
    #[error("IMAP TLS failed: {0}")]
    Tls(String),
    #[error("IMAP command timed out after {0} seconds")]
    Timeout(u64),
    #[error("IMAP protocol error: {0}")]
    Protocol(String),
    #[error("IMAP server rejected {command}: {status} {text}")]
    Rejected {
        command: String,
        status: String,
        text: String,
    },
    #[error("IMAP message {uid} exceeds the configured {limit}-byte limit")]
    MessageTooLarge { uid: u32, limit: u64 },
    #[error("IMAP I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests;
