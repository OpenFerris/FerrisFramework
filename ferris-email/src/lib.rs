//! Ferris Email - outbound SMTP, inbound IMAP, queuing and templates
//!
//! Two directions, one vertical:
//!
//! * outbound — [`smtp`] builds and sends, [`queue`] persists the queue and
//!   [`handlers`] runs it on the job worker;
//! * inbound — [`imap`] is the transport client and [`inbound`] the pipeline
//!   that parses, stores and notifies (see [`ImapPoller`] for the per-account
//!   loop the server starts).
//!
//! Both use `rustls` (no `native-tls`), and both credentials are redacted in
//! `Debug`.

// The shared `ferris_core::Error` enum exceeds clippy's 256-byte
// `result_large_err` threshold. Boxing its variants is a `ferris-core` change
// outside this crate's surface, and every `Result` here is that same type —
// the lint adds no information at the call sites.
#![allow(clippy::result_large_err)]

pub mod account;
pub mod doctypes;
pub mod handlers;
pub mod imap;
pub mod inbound;
#[cfg(feature = "module")]
pub mod module;
pub mod queue;
pub mod smtp;
pub mod templates;
pub mod workspaces;

pub use account::{AccountManager, EmailAccount, EmailAccountConfig, ImapConfig};
pub use handlers::{EmailJobHandler, SendEmailHandler, SendTemplatedEmailHandler};
pub use imap::{ImapClient, ImapError, ImapSession};
pub use inbound::{
    EngineInboundStore, ImapPoller, InboundAttachment, InboundEmail, InboundError,
    InboundProcessor, InboundStore, PollReport, PollerConfig, ProcessedInbound,
};
#[cfg(feature = "module")]
pub use module::EmailModule;
pub use queue::{
    AttachmentRef, EmailAccountManager, EmailJob, EmailQueue, EmailQueueStats, MailLog,
    QueuedEmailJob,
};
pub use smtp::{Attachment, Email, SmtpClient, SmtpConfig};
pub use templates::{EmailTemplate, TemplateEngine};
