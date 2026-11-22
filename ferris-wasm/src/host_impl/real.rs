//! The real host type and its construction: backends are opened once here and
//! degrade gracefully when a service is unreachable.

use super::config::CONFIG_TABLE;
use super::http::HttpPolicy;
use crate::host::StreamInfo;
use ferris_auth::user::User;
use ferris_db::Db;
use ferris_db::sql::Column;
use ferris_document::engine::DocumentEngine;
use ferris_jobs::JobQueue;
use parking_lot::Mutex;
use redis::Client as RedisClient;
use std::collections::HashMap;
use std::sync::Arc;

/// Real implementation of `HostApi` connecting to framework services.
///
/// Optional infra degrades gracefully: the job queue, cron scheduler and
/// notification backends exist only when their services are reachable (Redis,
/// an SMTP/push/webhook installation). When a service is absent the call is
/// **accepted and logged**, never fabricated — the plugin keeps working, and
/// the host is honest that delivery was not possible.
pub struct RealHostApi {
    pub(super) engine: DocumentEngine,
    pub(super) pool: Db,
    pub(super) job_queue: Option<Arc<JobQueue>>,
    /// Lazy cron scheduler, built the first time `schedule` is asked and only
    /// when a job queue is reachable.
    pub(super) scheduler: Mutex<Option<Arc<ferris_scheduler::Scheduler>>>,
    /// Live streams the host is forwarding into `handle-stream` (transport
    /// registry behind the `streams::info` import).
    pub(super) streams: Mutex<HashMap<String, StreamInfo>>,
    /// Per-stream carry-over buffers behind the `streams::state` /
    /// `streams::set-state` imports: the bytes a guest left unconsumed between
    /// two chunks, keyed by `stream-id` and dropped when the stream closes.
    pub(super) stream_buffers: Mutex<HashMap<String, Vec<u8>>>,
    pub(super) current_user: Mutex<Option<User>>,
    /// Whether the current plugin operation arrived over a verified client
    /// certificate with no session (mTLS service mode). When set, `ctx()` runs
    /// the engine in the framework's anonymous-but-allowed service context so a
    /// device-authenticated request's plugin docstore calls are not default-
    /// denied (H7 / plan Fase 5). Scoped to one operation by
    /// [`Plugin::handle_request_as`](crate::plugin::Plugin::handle_request_as).
    pub(super) certificate_service: Mutex<bool>,
    /// Outbound-HTTP policy behind the `http::request` import (allow-list,
    /// size caps, timeout, credential-header hygiene).
    pub(super) http: HttpPolicy,
}

impl RealHostApi {
    #[must_use]
    pub fn new(pool: Db) -> Self {
        // Try to create JobQueue with Redis if available, otherwise just use DB
        let job_queue = Self::create_job_queue(&pool);

        // The per-app config table is host-owned storage; create it on first
        // use (idempotent, dialect-aware). A failure here only disables the
        // `config` imports — the rest of the host keeps serving.
        if let Err(e) = crate::setup::block_on(
            pool.ensure_table(
                CONFIG_TABLE,
                &[
                    Column::new("plugin", "VARCHAR(255)")
                        .primary_key()
                        .not_null(),
                    Column::new("key", "VARCHAR(255)").primary_key().not_null(),
                    Column::new("value", "TEXT").not_null(),
                ],
            ),
        ) {
            tracing::warn!(error = %e, "app config table not created; `config` imports are disabled");
        }

        Self {
            engine: DocumentEngine::new(pool.clone()),
            pool,
            job_queue,
            scheduler: Mutex::new(None),
            streams: Mutex::new(HashMap::new()),
            stream_buffers: Mutex::new(HashMap::new()),
            current_user: Mutex::new(None),
            certificate_service: Mutex::new(false),
            http: HttpPolicy::from_env(),
        }
    }

    fn create_job_queue(pool: &Db) -> Option<Arc<JobQueue>> {
        let config = ferris_jobs::QueueConfig::default();
        // Prefer Redis when `REDIS_URL` points at a reachable server; otherwise
        // fall back to the durable `jobs` table so the `scheduler` imports work
        // on SQLite-only installs (R-FW-1a).
        let queue = std::env::var("REDIS_URL")
            .ok()
            .and_then(|url| RedisClient::open(url).ok())
            .and_then(|client| crate::setup::block_on(JobQueue::new(config.clone(), client)).ok())
            .map_or_else(
                || JobQueue::new_db(config, pool.clone()),
                |queue| queue.with_db(pool.clone()),
            );

        // `scheduler::schedule`/`scheduler::enqueue` write to the `jobs` table,
        // so create it before the imports are reachable.
        if let Err(e) = crate::setup::block_on(queue.init()) {
            tracing::warn!(error = %e, "job queue schema init failed; `scheduler` imports disabled");
            return None;
        }
        Some(Arc::new(queue))
    }
}
