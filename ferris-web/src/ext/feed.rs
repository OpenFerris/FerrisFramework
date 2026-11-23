//! The `/api/ui-extensions` feed: shared state, refetch gate, and the
//! resolution rules slots and widgets share.

use leptos::prelude::*;

use super::slots::SLOT_FORM_VIEW;
use super::{AppUi, UiExtension, UiFeed, WidgetDef};
use crate::i18n::t;

/// Shared OpenUI state: the apps' registered UI sets from `/api/ui-extensions`.
#[derive(Clone, Copy)]
pub struct UiCtx {
    pub apps: RwSignal<Vec<AppUi>>,
    /// The feed's last-seen `revision` (its payload fingerprint): the refetch
    /// gate — same revision, no swap, no widget remounts.
    revision: RwSignal<String>,
    /// The last feed failure. Without it a failed `/api/ui-extensions` left the
    /// home and every plugin page on their loading skeleton forever, which
    /// reads as "still loading" instead of "it did not load".
    pub error: RwSignal<Option<String>>,
}

impl UiCtx {
    pub fn default_ctx() -> Self {
        Self {
            apps: RwSignal::new(Vec::new()),
            revision: RwSignal::new(String::new()),
            error: RwSignal::new(None),
        }
    }

    /// Fetch the registered UI sets and swap them in when the feed's
    /// `revision` moved. Called on boot ([`UiCtx::init`]) and by every Fase 4
    /// refetch trigger: a refetch that changed nothing costs one request and
    /// zero re-renders, because the revision is a fingerprint of the whole
    /// payload.
    pub fn refresh(&self) {
        let ctx = *self;
        crate::api::drive(async move {
            let Ok(value) = crate::api::client::api_json("GET", "/api/ui-extensions", None).await
            else {
                ctx.error.set(Some(t("components.ext.feed_load_error")));
                return;
            };
            let Some((revision, apps)) = parse_feed(value) else {
                ctx.error.set(Some(t("components.ext.feed_format_error")));
                return;
            };
            ctx.error.set(None);
            let previous = ctx.revision.get_untracked();
            if previous == revision {
                return;
            }
            // A reloaded plugin can add, remove or change the DocTypes it
            // declares, so the metadata cached for the previous manifest is no
            // longer trustworthy. The first feed of the session has nothing to
            // invalidate because the cache starts empty.
            if !previous.is_empty() {
                crate::api::client::clear_meta_cache();
            }
            // Load the plugins' assets (scripts/stylesheets) that define the
            // declared custom elements. Browser-only: on SSR there is nothing
            // to inject yet.
            #[cfg(target_arch = "wasm32")]
            crate::ext::loader::ensure_loaded(&apps);
            ctx.revision.set(revision);
            ctx.apps.set(apps);
        });
    }

    /// Fetch the registered UI sets once, on the client, and keep the
    /// manifest live (Fase 4, limitación 7): when the tab returns to the
    /// foreground the feed is refetched, so a plugin reload that landed
    /// meanwhile is picked up without a restart — the server already answers
    /// the new routes through the Fase 3 guard. SSR keeps the empty state so
    /// the anchor stays deterministic for hydration.
    pub fn init(&self) {
        self.refresh();
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            let ctx = *self;
            // One listener for the page's lifetime: the Desk shell never
            // unmounts and `init` runs once per session, so a forgotten
            // closure (below) is exactly one per session.
            let callback = wasm_bindgen::closure::Closure::<dyn FnMut()>::new(move || {
                let visible = web_sys::window()
                    .and_then(|window| window.document())
                    .is_some_and(|document| !document.hidden());
                if visible {
                    ctx.refresh();
                }
            });
            if let Some(document) = web_sys::window().and_then(|window| window.document()) {
                let _ = document.add_event_listener_with_callback(
                    "visibilitychange",
                    callback.as_ref().unchecked_ref(),
                );
            }
            callback.forget();
        }
    }

    /// The extensions bound to `slot`, filtered by `doctype` for the doctype-
    /// aware slots, paired with the app that declared each one (F6 scopes each
    /// owner's CSS to its own mount). A declaration without a doctype applies
    /// to every doctype.
    pub fn extensions_for(&self, slot: &str, doctype: Option<&str>) -> Vec<(String, UiExtension)> {
        self.apps
            .with(|apps| {
                let mut out = Vec::new();
                for app in apps {
                    for ext in &app.extensions {
                        if ext.slot == slot && doctype_matches(&ext.doctype, doctype) {
                            out.push((app.owner.clone(), ext.clone()));
                        }
                    }
                }
                out
            })
            .into_iter()
            .collect()
    }

    /// The `form.view` extension a doctype's desk form should defer to, or
    /// `None` to render the built-in metadata form: the declaring app paired
    /// with its declaration (Fase 5 whole-view slot).
    ///
    /// Only a declaration that names this exact doctype matches — a `form.view`
    /// without a `doctype` is ignored, so an app cannot replace every form by
    /// omission. The first declaration in the feed's owner-sorted order wins,
    /// the same stable order slots and widgets mount in.
    #[must_use]
    pub fn form_view_for(&self, doctype: &str) -> Option<(String, UiExtension)> {
        self.apps.with(|apps| form_view_in(apps, doctype))
    }

    /// The definition a widget reference names — `owner.name`, or a bare
    /// `name` against `default_owner` — in the current feed's catalog: the
    /// client twin of the server-side registry's `find_widget`
    /// (`ferris_core::ui_extensions`).
    ///
    /// `None` until the publishing app's catalog lands in the feed, so
    /// callers render the unresolved placeholder instead of failing.
    #[must_use]
    pub fn resolve_widget_ref(
        &self,
        default_owner: &str,
        reference: &str,
    ) -> Option<(String, WidgetDef)> {
        self.apps
            .with_untracked(|apps| resolve_ref_in(apps, default_owner, reference))
    }

    /// The definition `owner` publishes under `name`, if its catalog carries
    /// it — the whole feed indexed by one lookup.
    #[must_use]
    pub fn widget_def(&self, owner: &str, name: &str) -> Option<WidgetDef> {
        self.apps.with_untracked(|apps| {
            apps.iter()
                .find(|ui| ui.owner == owner)
                .and_then(|ui| ui.widgets.iter().find(|w| w.name == name).cloned())
        })
    }
}

/// Case-insensitive doctype tag equality; a host-side declaration without a
/// doctype applies to every doctype.
pub(super) fn doctype_matches(declared: &Option<String>, current: Option<&str>) -> bool {
    match (declared, current) {
        (None, _) => true,
        (Some(have), Some(wanted)) => have.eq_ignore_ascii_case(wanted),
        (Some(_), None) => false,
    }
}

/// The `form.view` extension `doctype` should defer to, resolved against a feed
/// snapshot: the declaring app plus its declaration, in the feed's owner order.
///
/// Stricter than [`doctype_matches`] on purpose: a whole-view override must
/// name its doctype, otherwise every form a plugin did not mean to touch would
/// be replaced. Split from [`UiCtx::form_view_for`] so the rule is unit-testable
/// without a context.
#[must_use]
pub fn form_view_in(apps: &[AppUi], doctype: &str) -> Option<(String, UiExtension)> {
    apps.iter().find_map(|app| {
        app.extensions
            .iter()
            .find(|ext| {
                ext.slot == SLOT_FORM_VIEW
                    && ext
                        .doctype
                        .as_deref()
                        .is_some_and(|declared| declared.eq_ignore_ascii_case(doctype))
            })
            .map(|ext| (app.owner.clone(), ext.clone()))
    })
}

/// Resolve a widget reference against a catalog snapshot: `owner.name`
/// addresses the publishing app directly, a bare `name` resolves against
/// `default_owner`. The client twin of the server-side registry's
/// `find_widget` (`ferris_core::ui_extensions`), and the single rule
/// [`UiCtx::resolve_widget_ref`] applies.
///
/// `None` when no app's catalog carries it — the publishing app may register
/// later, so callers render the unresolved placeholder instead of failing.
#[must_use]
pub fn resolve_ref_in(
    apps: &[AppUi],
    default_owner: &str,
    reference: &str,
) -> Option<(String, WidgetDef)> {
    let (owner, name) = match reference.split_once('.') {
        Some((owner, name)) => (owner.to_string(), name.to_string()),
        None => (default_owner.to_string(), reference.to_string()),
    };
    apps.iter()
        .find(|ui| ui.owner == owner)
        .and_then(|ui| ui.widgets.iter().find(|w| w.name == name).cloned())
        .map(|def| (owner, def))
}

/// The `GET /api/ui-extensions` body is the F0 envelope `UiFeed { revision, apps }`
/// — NOT a bare list. Deserializing the whole payload as `Vec<AppUi>` fails with
/// `invalid type: map, expected a sequence`, and the client used to swallow that
/// error, leaving every slot and dashboard empty. Unwrap the envelope here, once,
/// keeping the `revision` the Fase 4 refetch gate compares, so the parse is
/// unit-testable instead of failing silently on the client.
pub(super) fn parse_feed(value: serde_json::Value) -> Option<(String, Vec<AppUi>)> {
    serde_json::from_value::<UiFeed>(value)
        .ok()
        .map(|feed| (feed.revision, feed.apps))
}
