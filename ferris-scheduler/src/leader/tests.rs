//! `scheduler_lease` behaviour: one holder, expiry takeover, renewal.

use super::*;

const LEASE: &str = "test-scheduler";

async fn db() -> ferris_db::Db {
    ferris_db::memory_db().await.expect("in-memory db")
}

async fn lease(db: &ferris_db::Db, owner: &str) -> LeaderLease {
    let lease = LeaderLease::new(db.clone(), LEASE, owner, 60);
    lease.init().await.expect("lease table");
    lease
}

/// Two schedulers on the same database: exactly one holds the lease at a time.
#[tokio::test]
async fn only_one_owner_holds_a_live_lease() {
    let db = db().await;
    let first = lease(&db, "instance-a").await;
    let second = lease(&db, "instance-b").await;

    assert!(
        first.try_acquire().await.unwrap(),
        "the first contender wins"
    );
    assert!(
        !second.try_acquire().await.unwrap(),
        "a live lease cannot be stolen"
    );
    assert!(first.renew().await.unwrap());
    assert!(
        !second.renew().await.unwrap(),
        "renewing someone else's lease is reported as lost"
    );

    first.release().await.unwrap();
    assert!(
        second.try_acquire().await.unwrap(),
        "a released lease is free for the next contender"
    );
}

/// A holder that stopped heartbeating (crashed) is taken over once the
/// persisted deadline passes, and its own renewal afterwards reports lost
/// leadership.
#[tokio::test]
async fn an_expired_lease_is_taken_over() {
    let db = db().await;
    let first = lease(&db, "instance-a").await;
    let second = lease(&db, "instance-b").await;

    assert!(first.try_acquire().await.unwrap());
    first.expire_for_test().await.unwrap();

    assert!(
        second.try_acquire().await.unwrap(),
        "an expired lease must be recoverable"
    );
    assert!(
        !first.renew().await.unwrap(),
        "the old holder must learn that it lost the lease"
    );
}

/// Renewal pushes the deadline forward, so the holder keeps the lease past the
/// moment its original TTL would have lapsed.
#[tokio::test]
async fn renewing_keeps_the_lease() {
    let db = db().await;
    let first = lease(&db, "instance-a").await;
    let second = lease(&db, "instance-b").await;

    assert!(first.try_acquire().await.unwrap());
    // Force the row into the state it has just before a restart of the same
    // owner, then renew: the deadline must move into the future again.
    first.expire_for_test().await.unwrap();
    assert!(first.renew().await.unwrap());
    assert!(
        !second.try_acquire().await.unwrap(),
        "a renewed lease stays with its owner"
    );
}

/// A different lease name has its own row and its own leader.
#[tokio::test]
async fn leases_are_namespaced() {
    let db = db().await;
    let global = lease(&db, "instance-a").await;
    let other = LeaderLease::new(db.clone(), "other", "instance-b", 60);
    other.init().await.unwrap();

    assert!(global.try_acquire().await.unwrap());
    assert!(other.try_acquire().await.unwrap());
}

/// The TTL is a deadline, not a sleep: an expired row is taken over without
/// waiting for wall-clock time to pass.
#[tokio::test]
async fn a_zero_ttl_lease_is_immediately_contested() {
    let db = db().await;
    let short = LeaderLease::new(db.clone(), "short", "instance-a", 0);
    short.init().await.unwrap();
    let other = LeaderLease::new(db.clone(), "short", "instance-b", 60);
    other.init().await.unwrap();

    assert!(short.try_acquire().await.unwrap());
    // The expiry check is `expires_at <= now`, and B's clock reading is never
    // earlier than A's, so a zero TTL is stale by the time B asks.
    assert!(other.try_acquire().await.unwrap());
}
