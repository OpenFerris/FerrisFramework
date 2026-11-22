//! Document-event hooks that make the integration DocTypes act on the world.
//!
//! [`install`] registers one tagged hook per (doctype, point) for every DocType
//! in the schema registry — re-registering with the same tag replaces the login
//! previous entry, so calling it again after a plugin registers new DocTypes
//! covers them without stacking duplicates (that is exactly what the dev server
//! does on hot reload).
//!
//! # Event mapping
//!
//! | HookPoint | Webhooks | Automation | Also |
//! |---|---|---|---|
//! | `AfterInsert` | `after_insert` | `after_insert` | assignment |
//! | `OnUpdate` | `on_update` | `on_update` | milestone tracking |
//! | `OnSubmit` | `on_submit` | `on_submit` | |
//! | `OnCancel` | `on_cancel` | `on_cancel` | |
//! | `OnTrash` | `on_trash` | — | |
//!
//! Push delivery is *not* a hook: the engine re-persists the triggering
//! document after every hook batch, so a hook cannot record `Sent`/`Failed` on
//! its own DocType (see [`push`]). Delivery is sweep-driven
//! ([`push::deliver_pending`], also exposed as a job) instead.
//!
//! The engine fires `OnUpdate` for fresh inserts as well (mirroring Frappe,
//! where `on_update` runs for new documents), so a row configured for both
//! events sees exactly the two calls it would in Frappe.
//!
//! # Failure policy
//!
//! The hooks are best-effort by design: a slow webhook or a failed condition
//! must not strand the document write that triggered it. Every step is wrapped
//! so its error is logged (and, where the DocType records it — the automation
//! trigger queue, the push row — persisted as a `Failed` state) while the save
//! proceeds. A per-task depth guard stops pathological recursion (flow A
//! notifies a document whose automation notifies again) from hanging the
//! runtime without treating concurrent writes as recursion.

use std::cell::Cell;
use std::future::Future;
use std::sync::Arc;

use ferris_core::Result;
use ferris_core::hooks::{DocumentHook, HookContext, HookFn, HookPoint, HookRegistry};
use ferris_schema::list_doctypes;

use super::assignment;
use super::automation;
use super::milestone;
use super::names::events;
use super::state::IntegrationsContext;
use super::webhook;

/// Tag under which every hook this crate installs is registered. Re-installing
/// replaces, never stacks.
const TAG: &str = "ferris-integrations";

// Per-task re-entrancy guard: integration work that itself writes documents
// must not feed the same hooks forever.
//
// The depth used to be a process-wide atomic. That conflated concurrency with
// recursion: a burst of unrelated writes (the observability layer records one
// `Path Log`/`API Request Log` row per request) pushed the counter over the
// limit and made the guard skip perfectly valid work. A task-local counter
// follows the actual causal chain, because a nested write is awaited inline by
// the same task.
tokio::task_local! {
    static RUN_DEPTH: Cell<usize>;
}
const MAX_DEPTH: usize = 8;

/// Framework log documents must not fan out into integrations.
///
/// The observability layer writes one row per request. Each write emits both
/// `AfterInsert` and `OnUpdate`, so registering the integration hooks for these
/// doctypes would run the webhook/automation lookups once per log row — and,
/// before the guard was made task-local, emit a warning per row under load.
/// Integrations are for application documents, not for the framework's own
/// request log.
const OBSERVABILITY_DOCTYPES: &[&str] = &["Path Log", "API Request Log", "View Log", "Error Log"];

/// Install the document-event hooks for every registered DocType.
///
/// Call after the full DocType catalog is registered (framework + plugins).
/// Idempotent: the `ferris-integrations` tag makes a second call replace the
/// first, and re-running after new DocTypes are registered covers them.
pub fn install(state: IntegrationsContext) {
    let state = Arc::new(state);
    let doctypes = list_doctypes();
    let total = doctypes.len();
    let doctypes: Vec<String> = doctypes
        .into_iter()
        .filter(|doctype| !OBSERVABILITY_DOCTYPES.contains(&doctype.as_str()))
        .collect();
    tracing::info!(
        count = doctypes.len(),
        skipped = total.saturating_sub(doctypes.len()),
        "installing integration document hooks"
    );
    for doctype in doctypes {
        for point in POINTS {
            let function = document_event_hook(state.clone(), point);
            HookRegistry::replace_global(TAG, doctype.clone(), point, function, 0, None);
        }
    }
}

/// The points the integrations react to (one hook each, dispatched by point).
const POINTS: [HookPoint; 5] = [
    HookPoint::AfterInsert,
    HookPoint::OnUpdate,
    HookPoint::OnSubmit,
    HookPoint::OnCancel,
    HookPoint::OnTrash,
];

fn document_event_hook(state: Arc<IntegrationsContext>, point: HookPoint) -> HookFn {
    Box::new(move |_ctx: &mut HookContext, doc: &mut dyn DocumentHook| {
        let state = state.clone();
        let doctype = doc.doctype().to_string();
        let docname = doc.name().to_string();
        let fields = doc.get_fields().clone();
        Box::pin(async move {
            // The hook future is already boxed at the trait boundary; box this
            // inner chain too so the task-local wrapper does not inline the
            // whole `run_point` state machine into every hook call site.
            Box::pin(with_run_depth(&doctype, &docname, async {
                run_point(&state, &doctype, &docname, &fields, point).await
            }))
            .await
        })
    })
}

/// Run `future` under the task's integration chain depth.
///
/// The first hook of a chain installs the task-local scope; nested hooks,
/// awaited by that same task, reuse it. A spawned task starts a new chain, which
/// is correct: it cannot re-enter the save that spawned it synchronously.
async fn with_run_depth<F>(doctype: &str, docname: &str, future: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    if RUN_DEPTH.try_with(|_| ()).is_ok() {
        return run_guarded(doctype, docname, future).await;
    }
    RUN_DEPTH
        .scope(Cell::new(0), run_guarded(doctype, docname, future))
        .await
}

async fn run_guarded<F>(doctype: &str, docname: &str, future: F) -> Result<()>
where
    F: Future<Output = Result<()>>,
{
    let depth = RUN_DEPTH.with(|cell| {
        let depth = cell.get();
        cell.set(depth.saturating_add(1));
        depth
    });
    if depth >= MAX_DEPTH {
        RUN_DEPTH.with(|cell| cell.set(depth));
        tracing::warn!(
            doctype,
            docname,
            "integration hook depth guard tripped; skipped"
        );
        return Ok(());
    }
    let result = future.await;
    RUN_DEPTH.with(|cell| cell.set(depth));
    result
}

async fn run_point(
    state: &IntegrationsContext,
    doctype: &str,
    docname: &str,
    fields: &std::collections::HashMap<String, ferris_core::Value>,
    point: HookPoint,
) -> Result<()> {
    let event = match point {
        HookPoint::AfterInsert => events::AFTER_INSERT,
        HookPoint::OnUpdate => events::ON_UPDATE,
        HookPoint::OnSubmit => events::ON_SUBMIT,
        HookPoint::OnCancel => events::ON_CANCEL,
        HookPoint::OnTrash => events::ON_TRASH,
        _ => return Ok(()),
    };

    // `webhooks = false` removes only this step: automation, assignment,
    // milestones and push delivery are separate flags/hooks and keep running.
    if state.features.webhooks {
        best_effort(
            "webhook",
            doctype,
            docname,
            webhook::run_for_event(state, doctype, event, docname, fields),
        )
        .await;
    }

    // `Automation Flow` models no trash event; keep the filter honest.
    if point != HookPoint::OnTrash {
        best_effort(
            "automation",
            doctype,
            docname,
            automation::evaluate(state, doctype, event, docname, fields),
        )
        .await;
    }

    match point {
        HookPoint::AfterInsert => {
            best_effort(
                "assignment",
                doctype,
                docname,
                assignment::apply_for_insert(state, doctype, docname, fields),
            )
            .await;
        }
        HookPoint::OnUpdate => {
            best_effort(
                "milestone",
                doctype,
                docname,
                milestone::check(state, doctype, docname, fields),
            )
            .await;
        }
        // OnSubmit/OnCancel/OnTrash stage no integration side effect today.
        _ => {}
    }
    Ok(())
}

/// Run a hook step and log instead of propagating: a hook is never allowed to
/// fail the document operation that triggered it.
async fn best_effort<F, T>(label: &str, doctype: &str, docname: &str, fut: F)
where
    F: Future<Output = Result<T>>,
{
    if let Err(e) = fut.await {
        tracing::warn!(label, doctype, docname, error = %e, "integration hook step failed");
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::{MAX_DEPTH, with_run_depth};

    /// Concurrent writes are not recursion. The old process-wide counter made
    /// a burst of log rows skip valid work once eight of them overlapped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_chains_do_not_share_a_depth_budget() {
        const TASKS: usize = MAX_DEPTH + 4;
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(TASKS);
        for _ in 0..TASKS {
            let running = Arc::clone(&running);
            let peak = Arc::clone(&peak);
            handles.push(tokio::spawn(async move {
                with_run_depth("Probe", "one", async move {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .expect("the chain stays under the guard");
            }));
        }
        for handle in handles {
            handle.await.expect("task joins");
        }
        assert_eq!(
            peak.load(Ordering::SeqCst),
            TASKS,
            "each task must get its own depth"
        );
    }

    fn recurse(
        level: usize,
        seen: Arc<AtomicUsize>,
    ) -> Pin<Box<dyn Future<Output = super::Result<()>> + Send>> {
        Box::pin(async move {
            with_run_depth("Probe", "one", async move {
                seen.fetch_add(1, Ordering::SeqCst);
                if level + 1 < MAX_DEPTH + 4 {
                    recurse(level + 1, Arc::clone(&seen)).await?;
                }
                Ok(())
            })
            .await
        })
    }

    /// The guard still stops a real chain: a nested write is awaited inline by
    /// the same task, so its depth is visible, unlike a concurrent write.
    #[tokio::test]
    async fn nested_chains_still_stop_at_the_limit() {
        let seen = Arc::new(AtomicUsize::new(0));
        recurse(0, Arc::clone(&seen))
            .await
            .expect("the guard swallows the over-depth step");
        assert_eq!(seen.load(Ordering::SeqCst), MAX_DEPTH);
    }

    /// `[features] webhooks = false` removes only the webhook dispatch: the
    /// hook stays installed for automation/assignment/milestone, but a
    /// matching `Webhook` row must not reach the endpoint.
    #[tokio::test]
    async fn webhooks_false_skips_dispatch_and_reinstall_enables_it() {
        use crate::runtime::names::{GOOGLE_CONTACTS, WEBHOOK};
        use crate::runtime::state::IntegrationsContext;
        use crate::runtime::store;
        use crate::runtime::testing::{TestServer, exclusive, test_context};
        use ferris_core::{FeatureFlags, Value};
        use serde_json::json;

        async fn contact(ctx: &IntegrationsContext, email: &str) {
            store::create(
                ctx,
                GOOGLE_CONTACTS,
                Value::from_json(json!({ "contact_name": "Ada", "email": email })),
            )
            .await
            .expect("contact created");
        }

        let _guard = exclusive().await;
        let server = TestServer::start(200, "{}");
        let ctx = test_context().await;

        let disabled = ctx.clone().with_features(FeatureFlags {
            webhooks: false,
            ..Default::default()
        });
        super::install(disabled);

        store::create(
            &ctx,
            WEBHOOK,
            Value::from_json(json!({
                "webhook_doctype": GOOGLE_CONTACTS,
                "webhook_docevent": "after_insert",
                "request_url": format!("{}/hook", server.url),
                "request_method": "POST",
                "request_body": r#"{"email": "{{ doc.email }}"}"#,
                "webhook_secret": "whsec",
                "enabled": true,
            })),
        )
        .await
        .expect("webhook row");

        contact(&ctx, "ada@example.com").await;
        assert!(
            server.received().is_empty(),
            "webhooks=false must not dispatch"
        );

        // Re-installing replaces the tagged hooks; the same row now dispatches.
        super::install(ctx.clone());
        contact(&ctx, "grace@example.com").await;
        assert_eq!(
            server.received().len(),
            1,
            "webhooks=true dispatches the matching row"
        );
    }
}
