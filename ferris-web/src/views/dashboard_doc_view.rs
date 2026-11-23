//! A doctype's documents rendered as a dashboard — the
//! [`DocView::Dashboard`](ferris_schema::contract::DocView::Dashboard) body of
//! `/desk/:module/:doctype/:name`.
//!
//! The Desk has one dashboard engine ([`DashboardGrid`], the very component an
//! app's route and the home page mount) and this module builds no second one.
//! All it answers is the question the engine cannot: *which* dashboard this
//! document opens. Four bindings, all of them declared next to the view kind
//! in the doctype's metadata ([`DashboardView`]):
//!
//! - **native** — the document *is* a `Dashboard` record, so its own name is
//!   the dashboard id and the Desk renders it directly: no request of its own,
//!   and no dependency on the UI feed either. The feed only *enriches* it (the
//!   declared label and seed); a record the feed did not carry — its route was
//!   unusable or already taken — still opens, with the document's own label
//!   and the widgets `GET /api/dashboard-layout/db/{id}` reads from the
//!   database. That is what makes opening any `Dashboard` document a
//!   dashboard.
//! - **document** — the doctype's *own* fields carry the dashboard format
//!   ([`DocumentFormat`]: widgets, layout, label, each bindable to another
//!   field name), so the document is its own panel: the Desk renders the
//!   widgets straight out of `doc`, and the layout endpoint reads and writes
//!   the same fields (`GET|PUT /api/dashboard-layout/doc/{doctype}/{name}`) —
//!   no second record, no per-user override. Fields the binding does not name
//!   are the doctype's own and are never touched.
//! - **wrapper** — a doctype that wraps a dashboard through a `Link` declares
//!   `link_field`, and *this* document's value in that field is the dashboard
//!   to open. A document that has not chosen one yet says so instead of
//!   drawing an empty grid.
//! - **fixed** — `dashboard` names one dashboard every document of the doctype
//!   opens, resolved in the feed; a declaration the feed does not carry is
//!   reported, never drawn as an empty grid.
//!
//! Resolution is a pure function of metadata, document and feed, so the SSR
//! pass and the browser draw the same thing. The loads are not this module's:
//! [`crate::views::services::FormService`] reads the metadata and the document
//! before this renders (its failure banner is this view's failure banner), and
//! the engine loads the layout itself.

use leptos::prelude::*;
use serde_json::Value;

use ferris_schema::contract::{
    AppUi, Dashboard, DashboardBinding, DashboardStyle, DashboardView, DocumentFormat, DynMeta,
    NATIVE_DASHBOARD_OWNER, WidgetInstance,
};

use crate::ext::UiCtx;
use crate::i18n::use_i18n;
use crate::views::DashboardGrid;
use crate::views::plugin_page::placeholder;

/// What a [`DashboardView`] declaration resolves to for one document.
#[derive(Debug, PartialEq, Eq)]
enum Resolved {
    /// Open this dashboard. `native` marks an id that is a *document name*
    /// rather than a declared id, so a miss in the feed still renders (see
    /// [`DashboardBinding::Direct`]).
    Open { id: String, native: bool },
    /// The document itself is the dashboard: `format` names its fields, which
    /// hold the widgets, the layout and the label.
    Document { format: DocumentFormat },
    /// The wrapper's link field holds no dashboard — a new document, or one
    /// whose panel was unlinked. Carries the field so the state can say where
    /// to set it.
    Unlinked { field: String },
}

/// The dashboard `docname` opens under `view`, with the wrapper's field read
/// out of `doc`.
fn resolve(view: &DashboardView, doc: &Value, docname: &str) -> Resolved {
    match view.binding_for(docname) {
        DashboardBinding::Direct { id, native } => Resolved::Open { id, native },
        // The document *is* the dashboard: its own fields are the whole
        // answer, so neither `doc` nor the feed is consulted here — the
        // render reads the bound fields directly.
        DashboardBinding::Document { format } => Resolved::Document { format },
        DashboardBinding::Link { field } => match doc.get(&field).and_then(Value::as_str) {
            Some(value) if !value.is_empty() => Resolved::Open {
                id: value.to_string(),
                // A wrapper points at a dashboard *document*, so the native
                // owner answers for it whether or not the feed carries it.
                native: true,
            },
            _ => Resolved::Unlinked { field },
        },
    }
}

/// The feed's entry for `id`: the app that owns that dashboard, with its
/// declared label and seed.
fn in_feed<'a>(apps: &'a [AppUi], id: &str) -> Option<(&'a str, &'a Dashboard)> {
    apps.iter().find_map(|app| {
        app.dashboards
            .iter()
            .find(|dashboard| dashboard.id == id)
            .map(|dashboard| (app.owner.as_str(), dashboard))
    })
}

/// The engine, mounted with the same props `plugin_page::dashboard_view`
/// gives it: one renderer, one shape, wherever a dashboard comes from.
/// `document` is the `(doctype, name)` target of the document binding, and
/// `None` for every dashboard that is not one. `declared` is the board style
/// the app (or the document) declares; the saved override layers over it.
fn grid(
    owner: &str,
    id: &str,
    label: String,
    seed: Vec<WidgetInstance>,
    document: Option<(String, String)>,
    declared: Option<DashboardStyle>,
) -> AnyView {
    view! {
        <DashboardGrid
            owner=owner.to_string()
            dashboard_id=id.to_string()
            label=label
            seed=seed
            document=document
            declared_style=declared
        />
    }
    .into_any()
}

/// The board style a `Dashboard` document declares in its own `style` field,
/// in either shape the field may hold (a `JSON` object or its serialized
/// text). A malformed value reads as "declares none", exactly like the
/// server's own reader: the Desk's defaults then apply instead of a broken
/// page.
fn declared_style(doc: &Value) -> Option<DashboardStyle> {
    let raw = doc.get("style")?;
    raw.as_str().map_or_else(
        || serde_json::from_value(raw.clone()).ok(),
        |text| serde_json::from_str(text).ok(),
    )
}

/// A document that names no dashboard yet — the wrapper's field is empty.
/// Saying so is the point: an empty grid would read as "this panel has no
/// widgets", which is a different statement.
fn unlinked_state(field: &str, i18n: &(impl crate::i18n::Translate + Sync)) -> AnyView {
    view! {
        <div class="rounded-xl border border-dashed p-6 space-y-2" data-name="DashboardDocUnlinked">
            <div class="text-sm font-medium">{i18n.t("views.dashboard_doc.unlinked_title")}</div>
            <div class="text-xs text-muted-foreground">
                {i18n.t("views.dashboard_doc.unlinked_before")}
                <code>{field.to_string()}</code>
                {i18n.t("views.dashboard_doc.unlinked_after")}
            </div>
        </div>
    }
    .into_any()
}

/// A fixed declaration naming a dashboard no app publishes: a broken
/// declaration, reported rather than drawn as an empty grid.
fn missing_state(id: &str, i18n: &(impl crate::i18n::Translate + Sync)) -> AnyView {
    view! {
        <div class="rounded-xl border border-destructive/40 bg-destructive/10 p-6 space-y-2" data-name="DashboardDocMissing">
            <div class="text-sm font-medium">{i18n.t("views.dashboard_doc.missing_title")}</div>
            <div class="text-xs text-muted-foreground">
                {i18n.t("views.dashboard_doc.missing_before")}
                <code>{id.to_string()}</code>
                {i18n.t("views.dashboard_doc.missing_after")}
            </div>
        </div>
    }
    .into_any()
}

/// The label shown in the header: what the feed declares, else what the
/// document carries, else the id — the three coincide for a record the feed
/// does carry, and the middle one is what a dropped record still has.
fn label_for(feed: &Dashboard, doc: &Value, id: &str) -> String {
    if !feed.label.is_empty() {
        return feed.label.clone();
    }
    document_label(doc).unwrap_or_else(|| id.to_string())
}

/// The document's own label in `field`, when it has a non-empty one.
fn label_in(doc: &Value, field: &str) -> Option<String> {
    doc.get(field)
        .and_then(Value::as_str)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
}

/// The document's own `label`, when it has a non-empty one — the native and
/// wrapper readings, which both read the canonical field.
fn document_label(doc: &Value) -> Option<String> {
    label_in(doc, "label")
}

/// The `DocView::Dashboard` body: metadata + document in, the doctype's
/// dashboard out.
#[component]
pub fn DashboardDocView(
    /// The doctype's metadata, where the view kind and its binding are
    /// declared. `None` only for the frame before the form's load lands.
    #[prop(into)]
    meta: Signal<Option<DynMeta>>,
    /// The document the form's load read: the wrapper's link field lives
    /// here, and so do the document binding's own format fields.
    #[prop(into)]
    doc: Signal<Value>,
    /// The route's document name: the native binding's dashboard id, and the
    /// document binding's row name.
    #[prop(into)]
    docname: Signal<String>,
    /// The route's doctype: half of the document binding's layout target
    /// (`/api/dashboard-layout/doc/{doctype}/{name}`).
    #[prop(into)]
    doctype: Signal<String>,
) -> impl IntoView {
    // `use_context`, not `expect_context`: outside the desk shell there is no
    // feed to resolve against, and the engine itself would panic on the
    // missing context — so this draws the loading box and never reaches it.
    let apps = use_context::<UiCtx>().map(|ctx| ctx.apps);
    let i18n = use_i18n();

    view! {
        <div class="space-y-3" data-name="DashboardDocView">
            {move || {
                let Some(apps) = apps else {
                    return placeholder();
                };
                let Some(meta) = meta.get() else {
                    return placeholder();
                };
                let doc = doc.get();
                let docname = docname.get();
                let doctype = doctype.get();
                let feed = apps.get();
                match resolve(&meta.dashboard, &doc, &docname) {
                    Resolved::Unlinked { field } => unlinked_state(&field, &i18n),
                    // The document is the dashboard: its bound fields supply
                    // the label and the seed, and the grid targets the
                    // document so its own save path applies. The feed is not
                    // consulted — this dashboard has no registered id.
                    Resolved::Document { format } => {
                        let label =
                            label_in(&doc, format.label()).unwrap_or_else(|| docname.clone());
                        let seed = format.widgets_in(&doc);
                        let id = docname.clone();
                        grid(
                            NATIVE_DASHBOARD_OWNER,
                            &id,
                            label,
                            seed,
                            Some((doctype, docname)),
                            None,
                        )
                    }
                    Resolved::Open { id, native } => match in_feed(&feed, &id) {
                        Some((owner, dashboard)) => grid(
                            owner,
                            &id,
                            label_for(dashboard, &doc, &id),
                            dashboard.widgets.clone(),
                            None,
                            dashboard.style.clone(),
                        ),
                        // The document is the dashboard: render it by name
                        // even before the feed answers (or if it never
                        // carries this record), with the document's label.
                        None if native => {
                            let label = document_label(&doc).unwrap_or_else(|| id.clone());
                            grid(
                                NATIVE_DASHBOARD_OWNER,
                                &id,
                                label,
                                Vec::new(),
                                None,
                                declared_style(&doc),
                            )
                        }
                        // A fixed declaration: only the feed can place its
                        // id, so an empty feed is still an unanswered one.
                        None if feed.is_empty() => placeholder(),
                        None => missing_state(&id, &i18n),
                    },
                }
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(json: &str) -> Value {
        serde_json::from_str(json).expect("valid document")
    }

    /// The native binding: a `Dashboard` record opens under its own name, and
    /// nothing but its name is needed to know that.
    #[test]
    fn a_native_dashboard_opens_under_its_own_name() {
        let view = DashboardView::default();
        let resolved = resolve(&view, &Value::Null, "SALES-1");
        assert_eq!(
            resolved,
            Resolved::Open {
                id: "SALES-1".to_string(),
                native: true
            }
        );
    }

    /// The wrapper binding: the dashboard is whatever the document's link
    /// field points at, never the document's own name.
    #[test]
    fn a_wrapper_opens_the_dashboard_its_link_field_names() {
        let view = DashboardView::link_field("panel");
        let doc = doc(r#"{"name":"R-1","panel":"OPS-2"}"#);
        assert_eq!(
            resolve(&view, &doc, "R-1"),
            Resolved::Open {
                id: "OPS-2".to_string(),
                native: true
            }
        );
    }

    /// The document binding: the document itself is the dashboard, and the
    /// resolution carries its bound field names (custom ones included) — no
    /// feed lookup, no linked record, nothing else to open.
    #[test]
    fn a_document_binding_opens_its_own_fields() {
        let bound = DocumentFormat::default()
            .with_widgets("panel_widgets")
            .with_label("titulo");
        assert_eq!(
            resolve(
                &DashboardView::document_fields(bound.clone()),
                &Value::Null,
                "PANEL-1"
            ),
            Resolved::Document { format: bound },
            "the declaration's field names ride the resolution"
        );
        assert_eq!(
            resolve(&DashboardView::document(), &Value::Null, "PANEL-1"),
            Resolved::Document {
                format: DocumentFormat::default()
            },
            "the bare declaration resolves to the canonical format"
        );
    }

    /// A wrapper with nothing linked yet — a new document, or a cleared
    /// field — reports the field instead of naming some other dashboard.
    #[test]
    fn a_wrapper_without_a_value_reports_the_field() {
        let view = DashboardView::link_field("panel");
        let empty = doc(r#"{"name":"R-1","panel":""}"#);
        assert_eq!(
            resolve(&view, &empty, "R-1"),
            Resolved::Unlinked {
                field: "panel".to_string()
            }
        );
        assert_eq!(
            resolve(&view, &Value::Null, "R-1"),
            Resolved::Unlinked {
                field: "panel".to_string()
            }
        );
    }

    /// The fixed binding: every document opens the declared dashboard, and
    /// its id is *not* native — a feed miss for it is a broken declaration,
    /// not something to render by name.
    #[test]
    fn a_fixed_declaration_is_not_native() {
        let view = DashboardView::dashboard("ventas");
        assert_eq!(
            resolve(&view, &Value::Null, "R-1"),
            Resolved::Open {
                id: "ventas".to_string(),
                native: false
            }
        );
    }

    /// The feed places a dashboard the app declared under the app's own owner;
    /// a lookup never guesses one.
    #[test]
    fn the_feed_supplies_the_owner_of_a_declared_dashboard() {
        let apps = vec![AppUi {
            owner: "acme".to_string(),
            dashboards: vec![Dashboard {
                id: "ventas".to_string(),
                label: "Ventas".to_string(),
                route: "/ventas".to_string(),
                ..Dashboard::default()
            }],
            ..AppUi::default()
        }];
        let (owner, dashboard) = in_feed(&apps, "ventas").expect("declared in the feed");
        assert_eq!(owner, "acme");
        assert_eq!(dashboard.label, "Ventas");
        assert!(in_feed(&apps, "otro").is_none());
        assert!(in_feed(&[], "ventas").is_none());
    }

    /// The header prefers the feed's label, falls back to the document's own
    /// (the record the feed dropped still has one) and finally to the id.
    #[test]
    fn the_label_falls_back_from_the_feed_to_the_document() {
        let declared = Dashboard {
            id: "a".to_string(),
            label: "Del feed".to_string(),
            ..Dashboard::default()
        };
        let with_doc = doc(r#"{"label":"Del documento"}"#);

        assert_eq!(label_for(&declared, &with_doc, "a"), "Del feed");

        let dropped = Dashboard {
            id: "a".to_string(),
            label: String::new(),
            ..Dashboard::default()
        };
        assert_eq!(label_for(&dropped, &with_doc, "a"), "Del documento");
        assert_eq!(label_for(&dropped, &Value::Null, "a"), "a");
    }
}
