//! The email hub, its worker and the per-account IMAP pollers.
//!
//! Kept out of `background/mod.rs` (which owns the queues and the generic app
//! worker) because this is the vertical's runtime: SMTP accounts, the `email`
//! queue consumer, the mailer bridge and inbound polling. `ferris-server`'s
//! composition root still calls these functions through the module re-exports.

// `EmailAccount::new` returns `ferris_core::Result`; the shared error enum
// exceeds clippy's 256-byte threshold and boxing it is a `ferris-core` change.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use anyhow::Result;
use ferris_core::config::EmailAccountConfig as CoreEmailAccountConfig;
use ferris_db::Db;
use ferris_email::{
    EmailAccount, EmailAccountConfig, EmailAccountManager, EmailJobHandler, EngineInboundStore,
    ImapClient, ImapConfig, ImapPoller, InboundProcessor, SmtpConfig, TemplateEngine,
};
use ferris_jobs::{JobQueue, QueueConfig, Worker, WorkerConfig, WorkerPool};
use tracing::{info, warn};

use crate::ServerSettings;

use super::build_queue;

#[cfg(feature = "email")]
mod log;

#[cfg(feature = "email")]
pub use log::{DbMailLog, LoggedEmailHandler, settings_default_account};

const EMAIL_QUEUE: &str = "email";

/// The email transport bits the server wires together once and shares.
///
/// The `email` queue, the account manager and the default outbound account,
/// shared so the [`EmailJobHandler`] and the integrations runtime's `Mailer`
/// bridge drain the same queue. `log` is the durable record every path writes
/// through (see [`DbMailLog`]).
#[cfg(feature = "email")]
pub struct EmailHub {
    queue: Arc<JobQueue>,
    accounts: Arc<EmailAccountManager>,
    /// Account used as the `from` of queued mail and the sender of a group
    /// send: `Email Settings.default_outgoing_account` when it names a
    /// configured account, else the first one.
    default_account: String,
    log: Arc<DbMailLog>,
    /// Per-account inbound pollers. Drop aborts them; each also stops on the
    /// process shutdown signal.
    _pollers: ImapPollers,
}

/// The IMAP pollers the server started, one task per configured mailbox.
///
/// The handle is owned by [`EmailHub`], so the pollers live exactly as long as
/// the background roles that built the hub. `Drop` aborts them as a backstop;
/// the cooperative path is the process-wide shutdown signal they subscribe to.
#[cfg(feature = "email")]
#[derive(Default)]
pub struct ImapPollers {
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

#[cfg(feature = "email")]
impl ImapPollers {
    /// Number of pollers running.
    #[must_use]
    pub const fn active(&self) -> usize {
        self.tasks.len()
    }
}

#[cfg(feature = "email")]
impl Drop for ImapPollers {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

/// Build the email hub: the `email` queue (initialised) and the configured
/// accounts, named after the first configured account by default.
///
/// Zero accounts is not an error — the email worker and the integrations
/// mailer downgrade to log-and-drop until an account is added.
///
/// # Errors
///
/// Fails only when the queue's schema cannot be initialised (`init`). The queue
/// itself falls back to the database when Redis is absent (R-FW-1a).
#[cfg(feature = "email")]
pub async fn build_email_hub(settings: &ServerSettings, pool: &Db) -> Result<EmailHub> {
    if !settings.config.features.email {
        // The server checks the flag before calling this; the guard keeps a
        // future caller from starting an email subsystem the operator turned
        // off (and makes the misuse loud instead of silent).
        anyhow::bail!(
            "the `email` feature is disabled in [features]; refusing to build the email hub"
        );
    }
    let email_cfg = &settings.config.email;
    let jobs_cfg = &settings.config.jobs;

    let queue = build_queue(
        settings,
        pool,
        QueueConfig {
            name: EMAIL_QUEUE.to_string(),
            retry_backoff_base_secs: email_cfg.retry_delay_secs,
            dead_letter_queue: jobs_cfg.dead_letter_queue.clone(),
        },
    )
    .await;
    queue.init().await?;

    let mut accounts = EmailAccountManager::new();
    for acct in &email_cfg.accounts {
        match email_account(acct) {
            Ok(account) => accounts.add_account(account),
            Err(e) => warn!("skipping email account `{}`: {}", acct.name, e),
        }
    }
    let first_account = email_cfg
        .accounts
        .first()
        .map(|acct| acct.name.clone())
        .unwrap_or_default();
    let default_account = match settings_default_account(pool).await {
        Some(name) if email_cfg.accounts.iter().any(|acct| acct.name == name) => name,
        Some(name) => {
            warn!(
                "Email Settings.default_outgoing_account = `{name}` is not a configured \
                 [email.accounts] entry; using `{first_account}`"
            );
            first_account
        }
        None => first_account,
    };
    if accounts.list_accounts().is_empty() {
        warn!("no email accounts configured; queued send jobs will fail until one is added");
    }
    let pollers = start_imap_pollers(settings, pool);
    Ok(EmailHub {
        queue: Arc::new(queue),
        accounts: Arc::new(accounts),
        default_account,
        log: Arc::new(DbMailLog::new(pool.clone())),
        _pollers: pollers,
    })
}

#[cfg(feature = "email")]
impl EmailHub {
    /// The one outbound queue of the process, with its delivery log.
    #[must_use]
    pub fn email_queue(&self) -> ferris_email::EmailQueue {
        ferris_email::EmailQueue::new(
            self.queue.clone(),
            self.accounts.clone(),
            self.default_account.clone(),
        )
        .with_log(self.log.clone())
    }

    /// Queue `email` to every member of `group`, skipping the opt-outs, and
    /// return how many messages were queued.
    ///
    /// One message per member: a shared `Bcc` batch would show the list to every
    /// recipient and make an opt-out impossible to honour.
    ///
    /// # Errors
    ///
    /// No configured account, a group that does not exist, or a queue failure.
    pub async fn send_to_group(&self, group: &str, email: ferris_email::Email) -> Result<usize> {
        if self.default_account.is_empty() {
            anyhow::bail!("no email account configured: cannot send to `{group}`");
        }
        let recipients = self.log.group_recipients(group).await?;
        let queue = self.email_queue();
        let mut queued = 0;
        for recipient in recipients {
            let mut message = email.clone();
            message.to = vec![recipient];
            message.text = Some(with_unsubscribe_footer(message.text.as_deref(), group));
            queue
                .queue_email_with_account(message, &self.default_account)
                .await?;
            queued += 1;
        }
        Ok(queued)
    }

    /// Opt `address` out of `group`, so the next send to it skips them.
    ///
    /// # Errors
    ///
    /// An empty address, a group that does not exist, or a write failure.
    pub async fn unsubscribe(&self, group: &str, address: &str) -> Result<bool> {
        Ok(self.log.unsubscribe(group, address).await?)
    }
}

/// Append the opt-out instructions to a group message.
fn with_unsubscribe_footer(text: Option<&str>, group: &str) -> String {
    format!(
        "{}\n\n--\nYou receive this because you are in the {group} email group. \
         Reply to the sender if you do not want further messages.",
        text.unwrap_or_default()
    )
}

/// Start the email worker on the `email` queue. Owns the worker tasks until the
/// returned pool is shut down.
///
/// Infallible by construction: the queue was built and initialised by
/// [`build_email_hub`], so there is no failure branch here. The signature says
/// so instead of returning a `Result` the caller can never observe (SRV8).
#[cfg(feature = "email")]
pub fn start_email_worker(settings: &ServerSettings, hub: &EmailHub) -> WorkerPool {
    if !settings.config.features.email {
        warn!("feature `email` is disabled: not starting the email worker");
        return WorkerPool::new();
    }
    let email_cfg = &settings.config.email;
    let jobs_cfg = &settings.config.jobs;

    let handler = LoggedEmailHandler::new(
        EmailJobHandler::new(hub.accounts.clone(), Arc::new(TemplateEngine::new())),
        hub.log.clone(),
    );

    let worker = Worker::new(
        WorkerConfig {
            worker_id: format!("email-{}", uuid::Uuid::new_v4()),
            queues: vec![EMAIL_QUEUE.to_string()],
            concurrency: email_cfg.queue_concurrency.max(1),
            poll_interval_ms: jobs_cfg.poll_interval_ms,
            shutdown_timeout_secs: 30,
        },
        handler,
        hub.queue.clone(),
    );

    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(worker);
    info!("email worker started on queue `{EMAIL_QUEUE}`");
    worker_pool
}

/// The host's port-based outbound mailer.
///
/// Modules consume `ferris_core::ports::MailerPort`; this adapter enqueues on
/// the same `email` queue the worker drains, so there is one delivery path and
/// no module owns SMTP credentials.
#[cfg(feature = "email")]
#[derive(Clone)]
pub struct EmailHubMailer {
    queue: Arc<ferris_email::EmailQueue>,
    handle: tokio::runtime::Handle,
}

#[cfg(feature = "email")]
impl ferris_core::ports::MailerPort for EmailHubMailer {
    fn send(&self, message: &ferris_core::ports::EmailMessage) -> ferris_core::Result<()> {
        let email = ferris_email::Email::new(
            message.from.clone().unwrap_or_default(),
            message.to.clone(),
            message.subject.clone(),
        )
        .with_text(message.text.clone());
        let queue = self.queue.clone();
        self.handle.spawn(async move {
            if let Err(error) = queue.queue_email(email).await {
                tracing::error!(%error, "email port: enqueue failed");
            }
        });
        Ok(())
    }
}

/// Build the port adapter for a configured email hub.
#[cfg(feature = "email")]
#[must_use]
pub fn email_hub_mailer(hub: &EmailHub) -> Option<EmailHubMailer> {
    if hub.default_account.is_empty() || hub.accounts.list_accounts().is_empty() {
        return None;
    }
    Some(EmailHubMailer {
        queue: Arc::new(hub.email_queue()),
        handle: tokio::runtime::Handle::current(),
    })
}

/// The [`Mailer`] the server installs into the integrations runtime.
///
/// Each `send` enqueues an email job on the shared `email` queue, so an auto
/// email report or email reminder the sweeps produce is delivered by the same
/// worker that sends the rest of the server's mail.
///
/// `Mailer::send` is sync by contract, so the async enqueue is handed to the
/// runtime handle; the message counts as accepted once it reaches the queue.
#[cfg(all(feature = "email", feature = "integrations"))]
#[derive(Clone)]
pub struct QueueMailer {
    queue: Arc<ferris_email::EmailQueue>,
    handle: tokio::runtime::Handle,
}

#[cfg(all(feature = "email", feature = "integrations"))]
impl ferris_integrations::runtime::Mailer for QueueMailer {
    fn send(
        &self,
        message: &ferris_integrations::runtime::EmailMessage,
    ) -> ferris_core::Result<()> {
        let email = ferris_email::Email::new(
            message.from.clone().unwrap_or_default(),
            message.to.clone(),
            message.subject.clone(),
        )
        .with_text(message.text.clone());
        let queue = self.queue.clone();
        self.handle.spawn(async move {
            if let Err(e) = queue.queue_email(email).await {
                tracing::error!(error = %e, "integrations email enqueue failed");
            }
        });
        Ok(())
    }
}

/// The mailer the integrations runtime should use given the email hub.
///
/// The [`QueueMailer`] when there is an account to send from, otherwise the
/// crate's `NullMailer` (which logs the drop instead of failing the run).
#[cfg(all(feature = "email", feature = "integrations"))]
#[must_use]
pub fn integrations_mailer(
    hub: Option<&EmailHub>,
) -> Arc<dyn ferris_integrations::runtime::Mailer> {
    match hub.filter(|h| !h.default_account.is_empty() && !h.accounts.list_accounts().is_empty()) {
        Some(hub) => Arc::new(QueueMailer {
            queue: Arc::new(hub.email_queue()),
            handle: tokio::runtime::Handle::current(),
        }),
        None => Arc::new(ferris_integrations::runtime::NullMailer),
    }
}

/// Map one account's `[email.accounts]` IMAP keys onto the client's config.
///
/// `None` means "inbound is disabled for this account": no `imap_host`. An
/// account with a host but no usable credentials still maps to `Some`, so
/// [`start_imap_pollers`] fails validation loudly instead of silently dropping
/// a mailbox the operator asked to poll.
#[cfg(feature = "email")]
#[must_use]
pub fn imap_config(account: &CoreEmailAccountConfig) -> Option<ImapConfig> {
    let host = account.imap_host.as_deref()?.trim();
    if host.is_empty() {
        return None;
    }
    let use_starttls = account.imap_starttls;
    Some(ImapConfig {
        host: host.to_string(),
        // 993 is implicit TLS, 143 is STARTTLS. `imap_port` wins when set.
        port: account
            .imap_port
            .unwrap_or(if use_starttls { 143 } else { 993 }),
        username: account
            .imap_username
            .clone()
            .filter(|username| !username.trim().is_empty())
            .unwrap_or_else(|| account.email.clone()),
        password: account.imap_password.clone().unwrap_or_default(),
        use_tls: account.imap_tls,
        use_starttls,
        timeout_secs: 60,
        ..ImapConfig::default()
    })
}

/// Start one IMAP poller per account with `imap_host` set.
///
/// Runs only when the `email` feature is enabled in `[features]`; the caller
/// (the email hub) is already gated on a background role. A misconfigured
/// account is logged and skipped: SMTP keeps working and the mail stays on the
/// server, which is the non-destructive failure. Each poller subscribes to the
/// process shutdown signal and stops with the server; [`ImapPollers`] aborts
/// any task that outlives it.
#[cfg(feature = "email")]
#[must_use]
pub fn start_imap_pollers(settings: &ServerSettings, pool: &Db) -> ImapPollers {
    let mut pollers = ImapPollers::default();
    if !settings.config.features.email {
        return pollers;
    }
    let accounts: Vec<(String, String, ImapConfig)> = settings
        .config
        .email
        .accounts
        .iter()
        .filter_map(|account| {
            let imap = imap_config(account)?;
            Some((account.name.clone(), account.email.clone(), imap))
        })
        .collect();
    if accounts.is_empty() {
        return pollers;
    }

    let store = Arc::new(EngineInboundStore::new(pool.clone()));
    let processor = Arc::new(InboundProcessor::new(store));
    for (name, email, imap) in accounts {
        match ImapClient::new(imap) {
            Ok(client) => {
                let poller = ImapPoller::new(name.clone(), email, client);
                let shutdown = ferris_api::shutdown::subscribe();
                pollers
                    .tasks
                    .push(tokio::spawn(poller.run(processor.clone(), shutdown)));
                info!("inbound IMAP poller started for account `{name}`");
            }
            Err(error) => warn!(
                account = %name,
                error = %error,
                "inbound IMAP disabled for this account: the settings are invalid"
            ),
        }
    }
    pollers
}

/// Map core email config onto the mailer's account config. SMTP setup is lazy
/// (the transport only connects at send time), so an unreachable server does
/// not prevent startup.
#[cfg(feature = "email")]
fn email_account(cfg: &CoreEmailAccountConfig) -> ferris_core::Result<EmailAccount> {
    let mut account = EmailAccountConfig::new(
        cfg.name.clone(),
        cfg.email.clone(),
        SmtpConfig {
            host: cfg.smtp_host.clone(),
            port: cfg.smtp_port,
            username: cfg.smtp_username.clone(),
            password: cfg.smtp_password.clone(),
            use_tls: cfg.smtp_tls,
            use_starttls: cfg.smtp_starttls,
            allow_plaintext: false,
            timeout_secs: 60,
            pool_size: 2,
        },
    );
    // Only attach IMAP when the account asks for it: an always-`Some` config
    // with an empty host made "no inbound" indistinguishable from "misconfigured
    // inbound".
    if let Some(imap) = imap_config(cfg) {
        account = account.with_imap(imap);
    }
    EmailAccount::new(account)
}

#[cfg(test)]
mod tests;
