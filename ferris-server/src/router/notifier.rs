//! The realtime implementation of `ferris_api::notify::Notifier`.
//!
//! `ferris-api` publishes committed document changes through a port instead of
//! depending on `ferris-realtime`. This adapter is the other half: the server
//! knows both sides, so it installs a sink that forwards to the realtime
//! engine's process-wide handler. Without the `realtime` feature the default
//! sink stays in place and every notification is a no-op.

#[cfg(feature = "realtime")]
use std::sync::Arc;

#[cfg(feature = "realtime")]
use ferris_api::notify::{Notifier, install};
#[cfg(feature = "realtime")]
use ferris_core::Value;

/// Install the realtime sink. First installation wins; a second boot (or a hot
/// reload) keeps the live handler, exactly like `ferris_realtime` itself.
#[cfg(feature = "realtime")]
pub fn install_realtime_notifier() {
    struct RealtimeNotifier;

    fn report(result: ferris_core::Result<()>) {
        if let Err(error) = result {
            tracing::warn!("realtime document notification failed: {error}");
        }
    }

    impl Notifier for RealtimeNotifier {
        fn doc_inserted(&self, doctype: &str, name: &str, doc: &Value) {
            report(ferris_realtime::notify_doc_insert(
                doctype,
                name,
                doc.clone(),
            ));
        }

        fn doc_updated(
            &self,
            doctype: &str,
            name: &str,
            doc: &Value,
            changes: &Value,
            previous: &Value,
        ) {
            report(ferris_realtime::notify_doc_update(
                doctype,
                name,
                doc.clone(),
                changes.clone(),
                previous.clone(),
            ));
        }

        fn doc_deleted(&self, doctype: &str, name: &str, previous: Option<&Value>) {
            report(ferris_realtime::notify_doc_delete(
                doctype,
                name,
                previous.cloned(),
            ));
        }

        fn doc_submitted(&self, doctype: &str, name: &str, doc: &Value) {
            report(ferris_realtime::notify_doc_submit(
                doctype,
                name,
                doc.clone(),
            ));
        }

        fn doc_cancelled(&self, doctype: &str, name: &str, doc: &Value) {
            report(ferris_realtime::notify_doc_cancel(
                doctype,
                name,
                doc.clone(),
            ));
        }
    }

    if install(Arc::new(RealtimeNotifier)) {
        tracing::info!("document notifications wired to the realtime engine");
    } else {
        tracing::warn!("a document notifier is already installed; keeping the first one");
    }
}

/// Without the `realtime` feature there is no sink to install; the default
/// no-op stays in place.
#[cfg(not(feature = "realtime"))]
pub fn install_realtime_notifier() {}
