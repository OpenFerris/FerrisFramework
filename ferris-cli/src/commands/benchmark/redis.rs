//! Guard rails for benchmarks that write to Redis.

use anyhow::{Result, anyhow};

/// Whether the Redis in this URL is this machine.
///
/// `redis://localhost:6379` and `redis://<this-host>:6379` are a developer's
/// instance; anything else may be shared, and publishing a thousand messages
/// into somebody else's realtime handlers is not a thing to do by accident.
pub(super) fn is_local_redis(url: &str) -> bool {
    let Ok(info) = url.parse::<redis::ConnectionInfo>() else {
        return false;
    };
    let redis::ConnectionAddr::Tcp(host, _) = info.addr else {
        return false;
    };
    let host = host.trim_matches(['[', ']']);
    if matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") {
        return true;
    }
    hostname().is_ok_and(|name| name == host)
}

fn hostname() -> Result<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_string())
        .map_err(|e| anyhow!("cannot read the hostname: {e}"))
}

/// Refuse to write to a Redis that is not this machine unless asked.
///
/// `writes` says what this particular benchmark is about to do there, because
/// "the realtime benchmark enqueues durable jobs" is its own kind of confusing.
pub(super) fn check_redis_is_writable(
    url: &str,
    allow_remote: bool,
    what: &str,
    writes: &str,
) -> Result<()> {
    if is_local_redis(url) || allow_remote {
        return Ok(());
    }
    Err(anyhow!(
        "the {what} benchmark {writes} at {url}, which is not this machine. Point \
         redis.url at a local instance, or pass --allow-remote-write if that is really \
         where you meant to measure."
    ))
}
