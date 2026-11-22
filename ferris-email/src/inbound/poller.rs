//! The per-account IMAP poller.
//!
//! # Why polling, not IDLE
//!
//! IDLE keeps one connection open per mailbox and gets new mail pushed by the
//! server. It would halve the latency (seconds instead of the poll interval)
//! at the cost of a permanent connection, an interruption/re-issue state
//! machine and a second command channel to flag messages while the IDLE
//! connection is parked. The server's job model is a periodic sweep, so this
//! poller reconnects, drains the unseen UIDs and disconnects. The interval is
//! [`PollerConfig::interval`] (60 s by default) and is documented as the
//! latency bound in the Email guide.
//!
//! # Delivery contract
//!
//! Per message, in this order: fetch (`BODY.PEEK[]`, which leaves the flags
//! alone) → parse → store → **then** `\Seen`. A failure at any step before the
//! store leaves the message unseen; because [`InboundStore::existing`] makes the
//! store idempotent, the next poll either stores it once or finds it already
//! stored. The flag is never set for a message that was not persisted, and a
//! failure is never silent: it is logged with the account and UID and the cycle
//! backs off. The fetch uses `BODY.PEEK[]` rather than `RFC822` because the
//! latter sets `\Seen` server-side as a side effect of reading, which would take
//! the message out of `SEARCH UNSEEN` before it is durable (MAIL-01).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

use crate::imap::{ImapClient, ImapError};

use super::{InboundEmail, InboundError, InboundProcessor, InboundStore};

/// How often an account is polled, and how aggressively a failure is retried.
#[derive(Debug, Clone)]
pub struct PollerConfig {
    /// Delay between successful polls.
    pub interval: Duration,
    /// First delay after a failure; doubles up to `max_backoff`.
    pub retry_base: Duration,
    /// Ceiling for the failure backoff.
    pub max_backoff: Duration,
    /// Unseen UIDs fetched per cycle. The rest wait for the next poll.
    pub batch: usize,
}

impl Default for PollerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            retry_base: Duration::from_secs(10),
            max_backoff: Duration::from_mins(15),
            batch: 25,
        }
    }
}

/// What one cycle did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PollReport {
    /// Messages stored (or recognised as already stored).
    pub stored: usize,
    /// Messages fetched and parsed but skipped (oversized or malformed). They
    /// stay unseen, so the condition is visible on every cycle.
    pub skipped: usize,
    /// UIDs the server reported unseen but no longer had.
    pub vanished: usize,
}

impl PollReport {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.stored == 0 && self.skipped == 0 && self.vanished == 0
    }
}

/// One account's poller: a validated client plus the cadence.
#[derive(Debug, Clone)]
pub struct ImapPoller {
    /// `Email Account.name`, used in logs and as the document's account.
    pub account: String,
    /// In-app notification recipient: the account's address. The store resolves
    /// it to a `User` by `email` (a `Notification.user` is a UUID). An address
    /// with no user account raises no notification and logs the miss instead of
    /// pretending the notice was delivered.
    pub notify_user: String,
    pub client: ImapClient,
    pub config: PollerConfig,
}

impl ImapPoller {
    #[must_use]
    pub fn new(
        account: impl Into<String>,
        notify_user: impl Into<String>,
        client: ImapClient,
    ) -> Self {
        Self {
            account: account.into(),
            notify_user: notify_user.into(),
            client,
            config: PollerConfig::default(),
        }
    }

    // Assigning a field through `&mut self` is not const; the lint is a false
    // positive on this builder.
    #[allow(clippy::missing_const_for_fn)]
    #[must_use]
    pub fn with_config(mut self, config: PollerConfig) -> Self {
        self.config = config;
        self
    }

    /// Run one cycle against the mailbox.
    ///
    /// # Errors
    ///
    /// A connection/login/select failure, a storage failure, or a fetch failure
    /// that breaks the connection. Per-message parse/oversize errors are
    /// counted in [`PollReport::skipped`] and do not fail the cycle, because
    /// one bad message must not block the rest of the mailbox.
    pub async fn poll_once<S: InboundStore>(
        &self,
        processor: &InboundProcessor<S>,
    ) -> Result<PollReport, InboundError> {
        let mut session = self.client.connect().await?;
        let uids = session.unseen_uids(self.config.batch.max(1)).await?;
        if uids.is_empty() {
            let _ = session.logout().await;
            return Ok(PollReport::default());
        }

        let mut report = PollReport::default();
        // A per-message storage failure does not abort the cycle: the other
        // messages are still attempted and flagged, so one poison message does
        // not starve the mailbox. The first error is returned at the end, which
        // makes the run loop back off instead of hammering a broken database.
        let mut store_error: Option<InboundError> = None;
        for uid in uids {
            let raw = match session.fetch_raw(uid).await {
                Ok(Some(raw)) => raw,
                Ok(None) => {
                    report.vanished += 1;
                    continue;
                }
                Err(ImapError::MessageTooLarge { limit, .. }) => {
                    warn!(
                        account = %self.account,
                        uid,
                        limit,
                        "inbound message exceeds the size limit; left unseen, not stored"
                    );
                    report.skipped += 1;
                    continue;
                }
                Err(error) => {
                    let _ = session.logout().await;
                    return Err(error.into());
                }
            };

            let mut email = match InboundEmail::from_raw(
                self.account.clone(),
                self.client.config().effective_mailbox(),
                uid,
                &raw,
            ) {
                Ok(email) => email,
                Err(error) => {
                    warn!(
                        account = %self.account,
                        uid,
                        error = %error,
                        "inbound message could not be parsed; left unseen, not stored"
                    );
                    report.skipped += 1;
                    continue;
                }
            };
            email.notify_user = Some(self.notify_user.clone()).filter(|user| !user.is_empty());

            // A storage failure leaves this message unseen. Keep going with the
            // rest of the batch; `store_error` turns the cycle into a failure
            // so the loop backs off.
            let processed = match processor.process(&email).await {
                Ok(processed) => processed,
                Err(error) => {
                    warn!(
                        account = %self.account,
                        uid,
                        error = %error,
                        "inbound message could not be stored; left unseen"
                    );
                    report.skipped += 1;
                    if store_error.is_none() {
                        store_error = Some(error);
                    }
                    continue;
                }
            };
            session.mark_seen(uid).await?;
            report.stored += 1;
            info!(
                account = %self.account,
                uid,
                name = %processed.name,
                duplicate = processed.duplicate,
                notified = processed.notified,
                "inbound message stored"
            );
        }

        if let Err(error) = session.logout().await {
            warn!(account = %self.account, error = %error, "IMAP LOGOUT failed");
        }
        store_error.map_or(Ok(report), Err)
    }

    /// Poll until `shutdown` flips to `true`.
    ///
    /// Success sleeps [`PollerConfig::interval`]; a failure sleeps an
    /// exponential backoff starting at [`PollerConfig::retry_base`] and capped
    /// at [`PollerConfig::max_backoff`]. A misconfigured or unreachable mailbox
    /// is therefore retried forever without a hot loop, and never blocks boot.
    pub async fn run<S: InboundStore>(
        self,
        processor: Arc<InboundProcessor<S>>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut backoff = self.config.retry_base;
        loop {
            if *shutdown.borrow() {
                break;
            }
            let outcome = tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
                outcome = self.poll_once(processor.as_ref()) => outcome,
            };

            let delay = match &outcome {
                Ok(report) => {
                    if !report.is_empty() {
                        info!(
                            account = %self.account,
                            stored = report.stored,
                            skipped = report.skipped,
                            vanished = report.vanished,
                            "IMAP poll completed"
                        );
                    }
                    backoff = self.config.retry_base;
                    self.config.interval
                }
                Err(error) => {
                    warn!(
                        account = %self.account,
                        backoff_secs = backoff.as_secs(),
                        error = %error,
                        "IMAP poll failed; will retry"
                    );
                    let delay = backoff;
                    backoff = (backoff * 2).min(self.config.max_backoff);
                    delay
                }
            };

            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                () = tokio::time::sleep(delay) => {}
            }
        }
        info!(account = %self.account, "IMAP poller stopped");
    }
}
