//! The IMAP commands and the public client/session API.

use super::super::protocol::{self, FetchData, Response, Status, Token};
use super::super::{ImapConfig, ImapError};
use super::transport::{Connection, open, quote, timed};

/// Untagged responses tolerated per command before the parser gives up.
const MAX_RESPONSES: usize = 10_000;
/// UIDs fetched by the convenience helper. The poller asks per UID, so this is
/// only a ceiling for an external caller.
pub const MAX_BATCH_UIDS: usize = 100;

/// One raw message returned by [`ImapClient::fetch_unread`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedMessage {
    pub uid: u32,
    pub mailbox: String,
    pub raw: Vec<u8>,
}

/// The result of one tagged command: the untagged responses it produced.
struct CommandResult {
    untagged: Vec<Vec<Token>>,
}

/// An authenticated, selected IMAP session.
pub struct ImapSession {
    connection: Connection,
    config: ImapConfig,
    next_tag: u32,
}

impl std::fmt::Debug for ImapSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapSession")
            .field("host", &self.config.host)
            .field("port", &self.config.port)
            .field("mailbox", &self.config.effective_mailbox())
            .finish_non_exhaustive()
    }
}

impl ImapSession {
    /// The unseen UIDs in the selected mailbox, oldest first (the server's
    /// order), capped at `limit`.
    ///
    /// # Errors
    ///
    /// Fails when the command is rejected or the connection breaks.
    pub async fn unseen_uids(&mut self, limit: usize) -> Result<Vec<u32>, ImapError> {
        let result = self.command("UID SEARCH UNSEEN").await?;
        let mut uids = Vec::new();
        for tokens in &result.untagged {
            uids.extend(protocol::parse_search(tokens));
            if uids.len() >= limit {
                break;
            }
        }
        uids.truncate(limit);
        Ok(uids)
    }

    /// Fetch the RFC822 source of `uid` without changing its flags.
    ///
    /// `BODY.PEEK[]` is the whole message *and* leaves `\Seen` alone; plain
    /// `RFC822`/`BODY[]` set the flag server-side as a side effect of reading,
    /// which would mark a message read before it is stored (MAIL-01). The
    /// caller is the one that flags it, after the document is durable.
    ///
    /// Returns `None` when the server no longer has the message (it was
    /// expunged between `SEARCH` and `FETCH`), which the poller treats as
    /// "nothing to store" rather than an error.
    ///
    /// # Errors
    ///
    /// [`ImapError::MessageTooLarge`] when the literal exceeds
    /// `max_message_bytes` (the bytes are drained, not parsed), and protocol or
    /// transport failures.
    pub async fn fetch_raw(&mut self, uid: u32) -> Result<Option<Vec<u8>>, ImapError> {
        let result = self
            .command(&format!("UID FETCH {uid} (UID BODY.PEEK[])"))
            .await?;
        let mut found: Option<FetchData> = None;
        for tokens in &result.untagged {
            let Ok(Some(data)) = protocol::parse_fetch(tokens) else {
                continue;
            };
            if data.uid.is_some_and(|value| value != uid) {
                continue;
            }
            found = Some(data);
            if found.as_ref().is_some_and(|data| data.message.is_some()) {
                break;
            }
        }
        let Some(data) = found else {
            return Ok(None);
        };
        if let Some(size) = data.too_large {
            return Err(ImapError::MessageTooLarge {
                uid,
                limit: size.max(self.config.max_message_bytes),
            });
        }
        Ok(data.message)
    }

    /// Mark `uid` as seen (`\Seen`). Only called after the caller stored the
    /// message, so a storage failure leaves it unseen for the next poll.
    ///
    /// # Errors
    ///
    /// Fails when the server rejects the flag update or the connection breaks.
    pub async fn mark_seen(&mut self, uid: u32) -> Result<(), ImapError> {
        self.command(&format!("UID STORE {uid} +FLAGS (\\Seen)"))
            .await?;
        Ok(())
    }

    /// Send `LOGOUT` and close. A failure is reported but the socket is closed
    /// either way; callers treat it as best effort.
    ///
    /// # Errors
    ///
    /// The tagged failure or transport error from `LOGOUT`.
    pub async fn logout(&mut self) -> Result<(), ImapError> {
        let result = self.command("LOGOUT").await;
        self.connection.close();
        result.map(|_| ())
    }

    async fn command(&mut self, command: &str) -> Result<CommandResult, ImapError> {
        let tag = format!("A{}", self.next_tag);
        self.next_tag = self.next_tag.saturating_add(1);
        let seconds = self.config.timeout_secs.max(1);
        let line = format!("{tag} {command}\r\n");
        timed(seconds, self.connection.write_all(line.as_bytes())).await?;

        let mut untagged = Vec::new();
        for _ in 0..MAX_RESPONSES {
            let response = timed(seconds, self.connection.read_response()).await?;
            match response {
                Response::Untagged(tokens) => untagged.push(tokens),
                Response::Continuation(text) => {
                    return Err(ImapError::Protocol(format!(
                        "unexpected continuation for `{command}`: {text}"
                    )));
                }
                Response::Tagged {
                    tag: got,
                    status,
                    tokens,
                } => {
                    if got != tag {
                        return Err(ImapError::Protocol(format!(
                            "response tag `{got}` does not match request `{tag}`"
                        )));
                    }
                    if status != Status::Ok {
                        return Err(ImapError::Rejected {
                            command: command
                                .split_whitespace()
                                .next()
                                .unwrap_or(command)
                                .to_string(),
                            status: match status {
                                Status::No => "NO".to_string(),
                                Status::Bad => "BAD".to_string(),
                                Status::Ok => "OK".to_string(),
                            },
                            text: protocol::atoms(&tokens).join(" "),
                        });
                    }
                    return Ok(CommandResult { untagged });
                }
            }
        }
        Err(ImapError::Protocol(
            "server sent no tagged completion".to_string(),
        ))
    }
}

/// An IMAP mailbox client. Cheap to clone; one connection per [`connect`].
///
/// [`connect`]: ImapClient::connect
#[derive(Debug, Clone)]
pub struct ImapClient {
    config: ImapConfig,
}

impl ImapClient {
    /// Validate `config` and build a client. No I/O happens here.
    ///
    /// # Errors
    ///
    /// [`ImapError::Config`] when the settings cannot be honoured; see
    /// [`ImapConfig::validate`].
    pub fn new(config: ImapConfig) -> Result<Self, ImapError> {
        config.validate()?;
        Ok(Self { config })
    }

    /// The configuration this client was built from.
    #[must_use]
    pub const fn config(&self) -> &ImapConfig {
        &self.config
    }

    /// Connect, authenticate and select the configured mailbox.
    ///
    /// # Errors
    ///
    /// Connection/TLS/timeout failures, a rejected `LOGIN` or `SELECT`, and the
    /// protocol errors a non-IMAP endpoint produces.
    pub async fn connect(&self) -> Result<ImapSession, ImapError> {
        let seconds = self.config.timeout_secs.max(1);
        let mut session = ImapSession {
            connection: open(&self.config).await?,
            config: self.config.clone(),
            next_tag: 1,
        };

        let greeting = timed(seconds, session.connection.read_response()).await?;
        let Response::Untagged(tokens) = greeting else {
            return Err(ImapError::Protocol(
                "server did not send a greeting".to_string(),
            ));
        };
        let greeted_with = protocol::atoms(&tokens);
        let first = greeted_with.first().map_or("", String::as_str);
        if !first.eq_ignore_ascii_case("OK") && !first.eq_ignore_ascii_case("PREAUTH") {
            return Err(ImapError::Protocol(format!(
                "server rejected the session: {}",
                greeted_with.join(" ")
            )));
        }

        if self.config.use_starttls {
            session.command("STARTTLS").await?;
            timed(seconds, session.connection.upgrade(&self.config.host)).await?;
        }

        if !first.eq_ignore_ascii_case("PREAUTH") {
            let username = quote(&self.config.username)?;
            let password = quote(&self.config.password)?;
            session
                .command(&format!("LOGIN {username} {password}"))
                .await?;
        }

        let mailbox = quote(self.config.effective_mailbox())?;
        session.command(&format!("SELECT {mailbox}")).await?;
        Ok(session)
    }

    /// Fetch every unseen message, newest search order, up to 100 UIDs.
    ///
    /// This is a convenience for callers that do not need to control the
    /// per-message flag update; the poller uses [`ImapClient::connect`] and
    /// marks each UID seen only after storing it.
    ///
    /// # Errors
    ///
    /// Connection/login/select failures, plus a fetch failure for any UID;
    /// messages already fetched are dropped, not returned partially.
    pub async fn fetch_unread(&self) -> Result<Vec<FetchedMessage>, ImapError> {
        let mailbox = self.config.effective_mailbox().to_string();
        let mut session = self.connect().await?;
        let result = async {
            let uids = session.unseen_uids(MAX_BATCH_UIDS).await?;
            let mut messages = Vec::new();
            for uid in uids {
                if let Some(raw) = session.fetch_raw(uid).await? {
                    messages.push(FetchedMessage {
                        uid,
                        mailbox: mailbox.clone(),
                        raw,
                    });
                }
            }
            Ok(messages)
        }
        .await;
        let _ = session.logout().await;
        result
    }
}
