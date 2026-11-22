//! The host slot anchors: their contract constants and the two components that
//! fill them (`UiSlot`, `UiHeaderSlot`).

use leptos::prelude::*;

use crate::app::Workspace;

use super::UiCtx;
#[cfg(target_arch = "wasm32")]
use super::mount::{mount_header_slot, mount_slot, stamp_header_props};

/// Form doctype-aware slot: the right corner of the form header (next to the
/// status badge). Declared by the host so plugins can target it without
/// knowing the desk's internals.
pub const SLOT_FORM_TOOLBAR_RIGHT: &str = "form.toolbar.right";
/// Form doctype-aware slot: the left corner of the form header, beside the
/// title — for plugins that put their actions in front of the document.
pub const SLOT_FORM_TOOLBAR_LEFT: &str = "form.toolbar.left";
/// Form doctype-aware slot: top of the plugin sidebar column shown beside
/// the form card (Fase 5). The column only renders while some
/// `form.sidebar.*` extension is declared, so the form keeps its current
/// full-width layout until a plugin asks for the sidebar.
pub const SLOT_FORM_SIDEBAR_TOP: &str = "form.sidebar.top";
/// Form doctype-aware slot: bottom of the plugin sidebar column (Fase 5).
pub const SLOT_FORM_SIDEBAR_BOTTOM: &str = "form.sidebar.bottom";
/// Form doctype-aware slot: between the form header and the field tabs.
pub const SLOT_FORM_BEFORE_FIELDS: &str = "form.before_fields";
/// Form doctype-aware slot: after the field tabs, before the form actions.
pub const SLOT_FORM_AFTER_FIELDS: &str = "form.after_fields";
/// Form doctype-aware **whole-view** slot: when an app declares an extension
/// for this slot on a doctype, the Desk mounts the app's custom element
/// *instead of* rendering the built-in metadata form at
/// `/desk/:module/:doctype/:name` (and for a Single at
/// `/desk/:module/:doctype`).
///
/// Unlike the additive `form.*` slots, this one replaces the view: the anchor
/// is the whole form body, so an app owns the doctype's editing experience.
/// The mount point is doctype-aware and **requires** the declaration to name
/// its `doctype` — a `form.view` without one never matches, so a plugin cannot
/// replace every form by accident.
///
/// The mounted element receives the selected document through `data-doc`
/// (JSON; `null` for the `new` route), `data-docstatus`, the routing params
/// (`data-doctype` / `data-name` / `data-module`), the owning app
/// (`data-owner` / `data-component`) and the declaration's `props` resolved
/// against that document (`data-props`, with `"$field"` projected from it).
pub const SLOT_FORM_VIEW: &str = "form.view";
/// List doctype-aware slot: the bulk-actions area of the list toolbar.
pub const SLOT_LIST_TOOLBAR_BULK: &str = "list.toolbar.bulk";
/// List doctype-aware slot: above the table (below the toolbar), for
/// banners and toolbars that belong to the result set.
pub const SLOT_LIST_BEFORE_TABLE: &str = "list.before_table";
/// List doctype-aware slot: mounted once per visible row, in the row's
/// trailing action cell. Every mounted element carries the record's name in
/// `data-row`.
pub const SLOT_LIST_ROW_ACTIONS: &str = "list.row.actions";
/// Shell slot: the bottom of the sidebar, above the footer — where a plugin
/// parks status indicators and other always-visible one-liners.
pub const SLOT_SHELL_SIDEBAR_BOTTOM: &str = "shell.sidebar.bottom";
/// Shell slot: the end of the top bar's right-hand actions row.
pub const SLOT_SHELL_TOPBAR_RIGHT: &str = "shell.topbar.right";
/// Shell slot: the sidebar's module header. An app that declares into it
/// replaces the desk's default trigger *visual*: the element the desk mounts
/// lives inside the dropdown trigger, so clicking it still opens the module
/// menu. The element receives the active workspace and the sidebar state as
/// live props (see [`UiHeaderSlot`]).
pub const SLOT_SHELL_HEADER: &str = "shell.header";

/// Renders nothing but an anchor; on the client, mounts the extensions bound
/// to `slot`.
///
/// On SSR this is an empty `<div data-slot>` — the browser mount happens in an
/// effect after hydration, so the server and client trees never diverge at the
/// point Leptos reconciles. On the client the anchor carries an `open` shadow
/// root and the plugin elements live inside it (F6): plugin CSS is adopted into
/// that root instead of leaking into the Desk.
#[component]
pub fn UiSlot(
    /// The host slot constant (e.g. [`SLOT_FORM_TOOLBAR_RIGHT`]).
    ///
    /// Named `slot_name` (not `slot`) because Leptos' `view!` macro reserves
    /// `slot`/`slot:` attributes as named-slot syntax on the *parent* builder.
    slot_name: &'static str,
    /// The current doctype for doctype-aware slots; omitted for global ones.
    /// Taken as a signal because the router reuses the view across param
    /// changes, so the slot must re-filter when the doctype moves.
    #[prop(optional)]
    doctype: Option<Signal<String>>,
    /// Row identity for the per-row slots ([`SLOT_LIST_ROW_ACTIONS`]):
    /// forwarded to the mounted elements as `data-row`, so the plugin's
    /// element knows which record it acts on. The anchor carries it too.
    #[prop(optional)]
    row: Option<String>,
    /// Row document for the per-row slots: a prop value written as `"$field"`
    /// is projected from here (R-7), so a plugin element gets the row's data
    /// without an extra fetch per row. `None` for the form/global slots.
    #[prop(optional)]
    row_data: Option<serde_json::Value>,
) -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let doctype_signal: Signal<String> = doctype.unwrap_or_default();
    // An empty anchor still draws a box, and a zero-width flex item still adds
    // its parent's gap (twice, between siblings). The slot is hidden until the
    // feed declares an extension, so layouts keep their intended rhythm; the
    // anchor stays in the DOM either way, so the mount effect never loses it.
    let has_extensions = Memo::new(move |_| {
        let dt = doctype_signal.get();
        let current = (!dt.is_empty()).then_some(dt.as_str());
        !ctx.extensions_for(slot_name, current).is_empty()
    });
    let anchor: NodeRef<leptos::html::Div> = NodeRef::new();
    // The mounted elements and the anchor both carry `data-row`; the effect
    // below (wasm-only) keeps the original, the view takes a clone.
    let row_attr = row.clone();

    // The close enough slot: fill the anchor every time the manifest or the
    // current doctype changes.
    Effect::new(move |_| {
        let current = doctype_signal.get();
        let current = (!current.is_empty()).then_some(current.as_str());
        let exts = ctx.extensions_for(slot_name, current);
        #[cfg(target_arch = "wasm32")]
        {
            // Mounting needs the plugin's custom elements defined (or at least
            // scheduled): injecting the manifest is deduplicated, so repeating
            // it on every slot mount is a no-op after the first time.
            let apps = ctx.apps.get();
            crate::ext::loader::ensure_loaded(&apps);
            mount_slot(
                anchor.get(),
                slot_name,
                current,
                row.as_deref(),
                row_data.as_ref(),
                &exts,
                &apps,
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (&anchor, &exts, &row_data);
    });

    view! {
        <div
            node_ref=anchor
            class="ferris-ui-slot"
            class:hidden=move || !has_extensions.get()
            data-slot=slot_name
            data-row=row_attr
        ></div>
    }
}

/// The sidebar's module header mounted as slots are: an app that declares
/// `shell.header` replaces the desk's default trigger *visual* with its own
/// custom element. Unlike [`UiSlot`] the props are **live** — the component
/// stamps the active workspace and the sidebar state onto the mounted
/// elements whenever they change, without rebuilding them (a plugin's header
/// keeps its internal DOM across a sidebar toggle or a workspace switch).
///
/// The app's element receives, like every slot/widget, `data-slot`,
/// `data-label` and `data-component`; the live half is:
///
/// - `data-sidebar-open` — `"true"` expanded, `"false"` collapsed to the rail;
/// - `data-module` / `data-label` / `data-icon` — the active workspace's
///   fields, for cheap reads;
/// - `data-props` — JSON `{ "open": <bool>, "workspace": <Workspace or null> }`
///   with the whole sidebar entry (items included).
///
/// The element is mounted *inside* the dropdown trigger by the sidebar — the
/// component itself only owns the anchor — so a click on it opens the module
/// menu exactly like the built-in tile.
#[component]
pub fn UiHeaderSlot(
    /// The active workspace, `None` while the desk layout has not loaded.
    #[prop(into)]
    workspace: Signal<Option<Workspace>>,
    /// Whether the sidebar is expanded (`true`) or collapsed to its rail.
    #[prop(into)]
    open: Signal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let anchor: NodeRef<leptos::html::Div> = NodeRef::new();
    // The extension set that is currently mounted. The effect below re-runs on
    // every workspace/sidebar change (to re-stamp the props), and this gate
    // keeps those runs from tearing the plugins' elements down each time.
    let mounted: RwSignal<Option<String>> = RwSignal::new(None);

    Effect::new(move |_| {
        let apps = ctx.apps.get();
        let exts = ctx.extensions_for(SLOT_SHELL_HEADER, None);
        let signature = format!("{exts:?}");
        let need_mount = mounted.get_untracked().as_deref() != Some(signature.as_str());
        #[cfg(target_arch = "wasm32")]
        {
            if need_mount {
                mounted.set(Some(signature));
                crate::ext::loader::ensure_loaded(&apps);
                mount_header_slot(anchor.get(), &exts, &apps);
            }
            stamp_header_props(anchor.get(), workspace.get().as_ref(), open.get());
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (
            &anchor, &apps, &exts, &signature, need_mount, workspace, open,
        );
    });

    view! {
        <div
            node_ref=anchor
            class="ferris-ui-slot"
            data-slot=SLOT_SHELL_HEADER
        ></div>
    }
}
