//! SMTP client for sending emails

use ferris_core::{EmailError, Error, Result, ValidationError};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    transport::smtp::authentication::Credentials,
};
use std::time::Duration;
use tracing::info;

/// SMTP configuration
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub use_tls: bool,
    pub use_starttls: bool,
    /// Explicit opt-in for unencrypted SMTP. Must be `true` to send in clear.
    #[serde(default)]
    pub allow_plaintext: bool,
    pub timeout_secs: u64,
    pub pool_size: usize,
}

/// The credential is redacted: this struct is embedded in `SmtpClient` and
/// `EmailAccountConfig`, and their derived `Debug` used to print the password
/// (H3). `Serialize` still emits it — the value is written to config on purpose.
impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("use_tls", &self.use_tls)
            .field("use_starttls", &self.use_starttls)
            .field("allow_plaintext", &self.allow_plaintext)
            .field("timeout_secs", &self.timeout_secs)
            .field("pool_size", &self.pool_size)
            .finish()
    }
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            // 587 is the submission port, i.e. STARTTLS: `use_tls` (implicit
            // TLS, conventionally 465) is off so the two flags agree with the
            // port. The previous default enabled both and silently took the
            // implicit branch, which cannot connect on 587.
            port: 587,
            username: String::new(),
            password: String::new(),
            use_tls: false,
            use_starttls: true,
            allow_plaintext: false,
            timeout_secs: 30,
            pool_size: 10,
        }
    }
}

/// SMTP client wrapper
#[derive(Debug, Clone)]
pub struct SmtpClient {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    config: SmtpConfig,
}

impl SmtpClient {
    /// Create new SMTP client.
    ///
    /// Refuses to build a plaintext transport unless `allow_plaintext` is set:
    /// silently sending credentials/email in clear is treated as a bug.
    ///
    /// # Errors
    ///
    /// - `use_tls` and `use_starttls` set together: they select different
    ///   transports (implicit TLS vs. an upgrade), and the old code silently
    ///   preferred one of them;
    /// - no transport at all without `allow_plaintext`;
    /// - the TLS backend or the host cannot be turned into a transport.
    pub fn new(config: SmtpConfig) -> Result<Self> {
        Self::validate_security(&config)?;

        let mut builder = if config.use_tls {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host).map_err(|e| {
                Error::Email(EmailError::Smtp(format!(
                    "Failed to create SMTP transport: {}",
                    e
                )))
            })?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host)
        };

        builder = builder.port(config.port);

        if !config.username.is_empty() {
            builder = builder.credentials(Credentials::new(
                config.username.clone(),
                config.password.clone(),
            ));
        }

        if config.use_starttls && !config.use_tls {
            let tls_params = lettre::transport::smtp::client::TlsParameters::new(
                config.host.clone(),
            )
            .map_err(|e| Error::Email(EmailError::Smtp(format!("Invalid TLS parameters: {e}"))))?;
            builder = builder.tls(lettre::transport::smtp::client::Tls::Required(tls_params));
        }

        builder = builder.timeout(Some(Duration::from_secs(config.timeout_secs)));

        let transport = builder.build();

        Ok(Self { transport, config })
    }

    /// Reject a configuration whose transport flags contradict each other or
    /// leave the connection in clear.
    fn validate_security(config: &SmtpConfig) -> Result<()> {
        if config.use_tls && config.use_starttls {
            return Err(Error::Validation(ValidationError::Invalid(
                "SMTP `use_tls` (implicit TLS, usually port 465) and `use_starttls` \
                 (upgrade, usually port 587) are mutually exclusive"
                    .to_string(),
            )));
        }
        if !config.use_tls && !config.use_starttls && !config.allow_plaintext {
            return Err(Error::Validation(ValidationError::Invalid(
                "SMTP is not encrypted: enable use_tls/use_starttls or set allow_plaintext=true"
                    .to_string(),
            )));
        }
        Ok(())
    }

    /// Send an email.
    ///
    /// # Errors
    ///
    /// An invalid address or header, a message that cannot be built, or a
    /// transport failure.
    pub async fn send(&self, email: Email) -> Result<()> {
        self.send_tracked(email).await.map(|_| ())
    }

    /// Send an email and return the `Message-ID` header it went out with.
    ///
    /// The header is minted while the message is built (by `lettre` when the
    /// caller did not pin one), so it only exists after `build_message`. A
    /// caller that keeps a delivery record uses this instead of [`Self::send`]
    /// and stores the returned value, which is what a reply or a bounce will
    /// quote back.
    ///
    /// # Errors
    ///
    /// The same failures as [`Self::send`]; the message is only reported sent
    /// when the transport accepted it.
    pub async fn send_tracked(&self, email: Email) -> Result<String> {
        let message = self.build_message(email)?;
        let message_id = message
            .headers()
            .get_raw("Message-ID")
            .unwrap_or_default()
            .to_string();

        self.transport
            .send(message)
            .await
            .map_err(|e| Error::Email(EmailError::Smtp(format!("Failed to send email: {}", e))))?;

        info!(%message_id, "email sent");
        Ok(message_id)
    }

    /// Send multiple emails
    pub async fn send_batch(&self, emails: Vec<Email>) -> Result<Vec<Result<()>>> {
        let mut results = Vec::new();

        for email in emails {
            let result = self.send(email).await;
            results.push(result);
        }

        Ok(results)
    }

    /// Build message from email
    fn build_message(&self, email: Email) -> Result<Message> {
        let mut builder = Message::builder().from(email.from.parse().map_err(|e| {
            Error::Validation(ValidationError::InvalidValue {
                field: "from".to_string(),
                message: format!("Invalid from address: {}", e),
            })
        })?);

        // Add recipients
        for to in email.to {
            builder = builder.to(to.parse().map_err(|e| {
                Error::Validation(ValidationError::InvalidValue {
                    field: "to".to_string(),
                    message: format!("Invalid to address: {}", e),
                })
            })?);
        }

        for cc in email.cc {
            builder = builder.cc(cc.parse().map_err(|e| {
                Error::Validation(ValidationError::InvalidValue {
                    field: "cc".to_string(),
                    message: format!("Invalid cc address: {}", e),
                })
            })?);
        }

        for bcc in email.bcc {
            builder = builder.bcc(bcc.parse().map_err(|e| {
                Error::Validation(ValidationError::InvalidValue {
                    field: "bcc".to_string(),
                    message: format!("Invalid bcc address: {}", e),
                })
            })?);
        }

        if let Some(reply_to) = email.reply_to {
            builder = builder.reply_to(reply_to.parse().map_err(|e| {
                Error::Validation(ValidationError::InvalidValue {
                    field: "reply_to".to_string(),
                    message: format!("Invalid reply-to address: {}", e),
                })
            })?);
        }

        if let Some(in_reply_to) = email.in_reply_to {
            builder = builder.in_reply_to(in_reply_to.parse().map_err(|e| {
                Error::Validation(ValidationError::InvalidValue {
                    field: "in_reply_to".to_string(),
                    message: format!("Invalid in-reply-to: {}", e),
                })
            })?);
        }

        for ref_header in email.references {
            builder = builder.references(ref_header.parse().map_err(|e| {
                Error::Validation(ValidationError::InvalidValue {
                    field: "references".to_string(),
                    message: format!("Invalid reference: {}", e),
                })
            })?);
        }

        // Strip CR/LF from the subject: header injection defense in depth.
        let subject = email.subject.replace('\r', " ").replace('\n', " ");
        builder = builder
            .subject(subject)
            // Date and Message-ID are required for correct threading/spam scoring.
            .date_now()
            .message_id(None);

        // Build body and attach any attachments
        let message = if email.attachments.is_empty() {
            match (email.text, email.html) {
                (Some(text), Some(html)) => builder.multipart(
                    lettre::message::MultiPart::alternative()
                        .singlepart(
                            lettre::message::SinglePart::builder()
                                .header(lettre::message::header::ContentType::TEXT_PLAIN)
                                .body(text),
                        )
                        .singlepart(
                            lettre::message::SinglePart::builder()
                                .header(lettre::message::header::ContentType::TEXT_HTML)
                                .body(html),
                        ),
                ),
                (None, Some(html)) => builder.singlepart(
                    lettre::message::SinglePart::builder()
                        .header(lettre::message::header::ContentType::TEXT_HTML)
                        .body(html),
                ),
                (Some(text), None) => builder.singlepart(
                    lettre::message::SinglePart::builder()
                        .header(lettre::message::header::ContentType::TEXT_PLAIN)
                        .body(text),
                ),
                (None, None) => builder.body(String::new()),
            }
        } else {
            let mut mixed = lettre::message::MultiPart::mixed().build();

            match (email.text, email.html) {
                (Some(text), Some(html)) => {
                    mixed = mixed.multipart(
                        lettre::message::MultiPart::alternative()
                            .singlepart(
                                lettre::message::SinglePart::builder()
                                    .header(lettre::message::header::ContentType::TEXT_PLAIN)
                                    .body(text),
                            )
                            .singlepart(
                                lettre::message::SinglePart::builder()
                                    .header(lettre::message::header::ContentType::TEXT_HTML)
                                    .body(html),
                            ),
                    );
                }
                (None, Some(html)) => {
                    mixed = mixed.singlepart(
                        lettre::message::SinglePart::builder()
                            .header(lettre::message::header::ContentType::TEXT_HTML)
                            .body(html),
                    );
                }
                (Some(text), None) => {
                    mixed = mixed.singlepart(
                        lettre::message::SinglePart::builder()
                            .header(lettre::message::header::ContentType::TEXT_PLAIN)
                            .body(text),
                    );
                }
                (None, None) => {}
            }

            for attachment in email.attachments {
                mixed = mixed.singlepart(to_lettre_attachment(attachment));
            }

            builder.multipart(mixed)
        };

        message
            .map_err(|e| Error::Email(EmailError::Smtp(format!("Failed to build message: {}", e))))
    }

    /// Test connection
    pub async fn test_connection(&self) -> Result<()> {
        // Try to send a test message
        let test_email = Email {
            from: self.config.username.clone(),
            to: vec![self.config.username.clone()],
            subject: "Ferris SMTP Test".to_string(),
            text: Some("This is a test email from Ferris framework.".to_string()),
            html: None,
            cc: vec![],
            bcc: vec![],
            reply_to: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        };

        self.send(test_email).await
    }
}

/// Convert a Ferris attachment into a lettre MIME part
fn to_lettre_attachment(attachment: Attachment) -> lettre::message::SinglePart {
    use lettre::message::Attachment as LettreAttachment;
    use lettre::message::header::ContentType;

    let content_type =
        ContentType::parse(&attachment.content_type).unwrap_or(ContentType::TEXT_PLAIN);

    let builder = if attachment.inline {
        match attachment.content_id.clone() {
            Some(content_id) => {
                LettreAttachment::new_inline_with_name(content_id, attachment.filename)
            }
            None => LettreAttachment::new_inline(attachment.filename),
        }
    } else {
        LettreAttachment::new(attachment.filename)
    };

    builder.body(attachment.content, content_type)
}

/// Email structure
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Email {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub reply_to: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<Attachment>,
}

impl Email {
    pub fn new(from: String, to: Vec<String>, subject: String) -> Self {
        Self {
            from,
            to,
            subject,
            ..Default::default()
        }
    }

    pub fn with_text(mut self, text: String) -> Self {
        self.text = Some(text);
        self
    }

    pub fn with_html(mut self, html: String) -> Self {
        self.html = Some(html);
        self
    }

    pub fn with_cc(mut self, cc: Vec<String>) -> Self {
        self.cc = cc;
        self
    }

    pub fn with_bcc(mut self, bcc: Vec<String>) -> Self {
        self.bcc = bcc;
        self
    }

    pub fn with_reply_to(mut self, reply_to: String) -> Self {
        self.reply_to = Some(reply_to);
        self
    }

    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }
}

/// Email attachment
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub content: Vec<u8>,
    pub inline: bool,
    pub content_id: Option<String>,
}

impl Attachment {
    pub fn new(filename: String, content_type: String, content: Vec<u8>) -> Self {
        Self {
            filename,
            content_type,
            content,
            inline: false,
            content_id: None,
        }
    }

    pub fn inline(mut self, content_id: String) -> Self {
        self.inline = true;
        self.content_id = Some(content_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implicit_tls_and_starttls_are_mutually_exclusive() {
        let config = SmtpConfig {
            use_tls: true,
            use_starttls: true,
            ..Default::default()
        };
        let err = SmtpClient::validate_security(&config)
            .expect_err("both transports selected must be rejected");
        assert!(
            err.to_string().contains("mutually exclusive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unencrypted_without_opt_in_is_rejected() {
        let config = SmtpConfig {
            use_tls: false,
            use_starttls: false,
            allow_plaintext: false,
            ..Default::default()
        };
        assert!(SmtpClient::validate_security(&config).is_err());
        let config = SmtpConfig {
            allow_plaintext: true,
            ..config
        };
        assert!(SmtpClient::validate_security(&config).is_ok());
    }

    #[test]
    fn default_config_matches_the_submission_port() {
        let config = SmtpConfig::default();
        assert_eq!(config.port, 587);
        assert!(config.use_starttls && !config.use_tls);
        assert!(SmtpClient::validate_security(&config).is_ok());
    }

    #[test]
    fn test_email_creation() {
        let email = Email::new(
            "sender@example.com".to_string(),
            vec!["recipient@example.com".to_string()],
            "Test Subject".to_string(),
        )
        .with_text("Test body".to_string());

        assert_eq!(email.from, "sender@example.com");
        assert_eq!(email.to, vec!["recipient@example.com"]);
        assert_eq!(email.text, Some("Test body".to_string()));
    }

    #[test]
    fn test_attachment() {
        let attachment = Attachment::new(
            "test.txt".to_string(),
            "text/plain".to_string(),
            b"Hello World".to_vec(),
        );

        assert_eq!(attachment.filename, "test.txt");
        assert_eq!(attachment.content_type, "text/plain");
    }

    /// The delivery record needs the id the message actually carried, so it has
    /// to be readable from the built message rather than guessed beforehand.
    #[tokio::test]
    async fn the_built_message_carries_a_usable_message_id() {
        let client = SmtpClient::new(SmtpConfig {
            host: "127.0.0.1".to_string(),
            allow_plaintext: true,
            use_starttls: false,
            ..SmtpConfig::default()
        })
        .expect("plaintext test transport");
        let message = client
            .build_message(Email::new(
                "sender@example.com".to_string(),
                vec!["recipient@example.com".to_string()],
                "Test".to_string(),
            ))
            .expect("valid message");
        let message_id = message
            .headers()
            .get_raw("Message-ID")
            .expect("a Message-ID header");
        assert!(
            message_id.starts_with('<') && message_id.ends_with('>'),
            "{message_id}"
        );
        assert!(message_id.contains('@'), "{message_id}");
    }

    /// H3: the SMTP credential is embedded in `SmtpClient` and in
    /// `EmailAccountConfig`, both of which are `Debug`-printed on errors. A
    /// derived `Debug` leaked the password to any `{:?}`.
    #[test]
    fn the_smtp_password_is_redacted_in_debug() {
        let config = SmtpConfig {
            password: "hunter2".to_string(),
            ..SmtpConfig::default()
        };
        let printed = format!("{config:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }
}
