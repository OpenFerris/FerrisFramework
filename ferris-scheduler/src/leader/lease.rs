//! The `scheduler_lease` row and its compare-and-set operations.
//!
//! The row is `(name, owner, expires_at)`. Acquisition is a single guarded
//! `UPDATE` (take the row if it is ours or expired), falling back to an
//! `INSERT` when no row exists yet; the loser of that insert gets the
//! `DatabaseError::Constraint` variant `ferris-db` classifies, which is the
//! normal "somebody else is leader" outcome, not a failure.
//!
//! Every statement is portable SQL with bound parameters, so it behaves the
//! same on SQLite, PostgreSQL and MySQL. Expiry is a unix timestamp in a
//! `BIGINT`, compared in SQL against the contender's clock — no in-process
//! timer and nothing to keep in sync between hosts.

use chrono::Utc;
use ferris_core::{Error, Result, Value};
use ferris_db::{Db, Stmt, sql::Column};

pub(super) const TAB_SCHEDULER_LEASE: &str = "scheduler_lease";

fn lease_table() -> Vec<Column> {
    vec![
        Column::new("name", "VARCHAR(255)").not_null().primary_key(),
        Column::new("owner", "VARCHAR(255)").not_null(),
        Column::new("expires_at", "BIGINT").not_null(),
    ]
}

/// A process's claim on a named scheduler lease.
pub struct LeaderLease {
    db: Db,
    name: String,
    owner: String,
    ttl_secs: i64,
}

impl LeaderLease {
    /// A lease for `name`, claimed on behalf of the process-level `owner`.
    #[must_use]
    pub fn new(db: Db, name: impl Into<String>, owner: impl Into<String>, ttl_secs: i64) -> Self {
        Self {
            db,
            name: name.into(),
            owner: owner.into(),
            ttl_secs,
        }
    }

    /// Create the backing table if it does not exist.
    ///
    /// # Errors
    ///
    /// Fails when the table cannot be created.
    pub async fn init(&self) -> Result<()> {
        self.db
            .ensure_table(TAB_SCHEDULER_LEASE, &lease_table())
            .await
    }

    /// The identity stored in the row, for logging.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Try to become — or stay — the leader.
    ///
    /// Returns `true` when this owner holds the lease afterwards. Taking over
    /// is verification-free on purpose: the previous holder's expiry is the
    /// only fact that matters, and a holder that is alive renews well before
    /// it lapses.
    ///
    /// # Errors
    ///
    /// Fails when neither the update nor the insert can run (a lost insert
    /// race is reported as `false`, not an error).
    pub async fn try_acquire(&self) -> Result<bool> {
        let now = Utc::now().timestamp();
        let expires_at = now.saturating_add(self.ttl_secs);

        // One statement decides the common cases: an expired lease, or one
        // already owned by this process (a restart inside the TTL). A live
        // lease held by someone else matches no row, so it cannot be stolen.
        let affected = self
            .db
            .execute(&Stmt::new(
                format!(
                    "UPDATE {TAB_SCHEDULER_LEASE} SET owner = ?, expires_at = ? \
                     WHERE name = ? AND (owner = ? OR expires_at <= ?)"
                ),
                vec![
                    Value::from(self.owner.as_str()),
                    Value::from(expires_at),
                    Value::from(self.name.as_str()),
                    Value::from(self.owner.as_str()),
                    Value::from(now),
                ],
            ))
            .await?;

        if !affected.is_empty() {
            return Ok(true);
        }

        // No row was taken: either no lease exists yet or a live one belongs
        // to another process. The insert settles it; its unique violation is
        // the answer "you lost the race", not an error.
        match self
            .db
            .execute(&Stmt::new(
                format!(
                    "INSERT INTO {TAB_SCHEDULER_LEASE} (name, owner, expires_at) VALUES (?, ?, ?)"
                ),
                vec![
                    Value::from(self.name.as_str()),
                    Value::from(self.owner.as_str()),
                    Value::from(expires_at),
                ],
            ))
            .await
        {
            Ok(_) => Ok(true),
            Err(Error::Database(ferris_core::error::DatabaseError::Constraint(_))) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Extend the lease this owner already holds.
    ///
    /// Returns `false` when the row is gone or belongs to another owner: this
    /// process has lost leadership and must stop enqueueing.
    ///
    /// # Errors
    ///
    /// Fails when the update cannot run. A transient database error is not the
    /// same as losing the lease; the caller keeps leadership until the
    /// persisted deadline actually lapses.
    pub async fn renew(&self) -> Result<bool> {
        let expires_at = Utc::now().timestamp().saturating_add(self.ttl_secs);
        let affected = self
            .db
            .execute(&Stmt::new(
                format!(
                    "UPDATE {TAB_SCHEDULER_LEASE} SET expires_at = ? WHERE name = ? AND owner = ?"
                ),
                vec![
                    Value::from(expires_at),
                    Value::from(self.name.as_str()),
                    Value::from(self.owner.as_str()),
                ],
            ))
            .await?;
        Ok(!affected.is_empty())
    }

    /// Give the lease up so another instance can take over immediately.
    ///
    /// # Errors
    ///
    /// Fails when the delete cannot run.
    pub async fn release(&self) -> Result<()> {
        self.db
            .execute(&Stmt::new(
                format!("DELETE FROM {TAB_SCHEDULER_LEASE} WHERE name = ? AND owner = ?"),
                vec![
                    Value::from(self.name.as_str()),
                    Value::from(self.owner.as_str()),
                ],
            ))
            .await
            .map(|_| ())
    }

    /// Make the persisted lease look expired, as if its holder had died.
    ///
    /// Tests cannot wait out a real TTL deterministically, so they move the
    /// deadline instead of the clock.
    #[cfg(test)]
    pub(crate) async fn expire_for_test(&self) -> Result<()> {
        self.db
            .execute(&Stmt::new(
                format!("UPDATE {TAB_SCHEDULER_LEASE} SET expires_at = 0 WHERE name = ?"),
                vec![Value::from(self.name.as_str())],
            ))
            .await
            .map(|_| ())
    }
}
