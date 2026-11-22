//! `pubsub` interface of [`HostApi`](crate::host::HostApi).

use super::RealHostApi;
use crate::host::{RealtimeBroadcaster, realtime_broadcaster};
use anyhow::Result;
use std::sync::Arc;

impl RealHostApi {
    // -- pubsub -------------------------------------------------------------

    pub(super) async fn pubsub_publish(
        &self,
        channel: &str,
        event_type: &str,
        payload: &str,
    ) -> Result<()> {
        publish_via(realtime_broadcaster(), channel, event_type, payload)
    }
}

/// Deliver a pubsub event through `broadcaster`, or fail when none is
/// registered.
///
/// A plugin that publishes with no realtime backend must not be told the event
/// went out: the old `Ok(())` + warning let a guest report success while every
/// subscriber missed it. Split out from [`RealHostApi::pubsub_publish`] so the
/// two branches are testable without touching the process-wide registry.
pub(super) fn publish_via(
    broadcaster: Option<Arc<dyn RealtimeBroadcaster>>,
    channel: &str,
    event_type: &str,
    payload: &str,
) -> Result<()> {
    let broadcaster = broadcaster.ok_or_else(|| {
        anyhow::anyhow!("no realtime broadcaster registered; event not delivered")
    })?;
    broadcaster.publish(channel, event_type, payload)
}
