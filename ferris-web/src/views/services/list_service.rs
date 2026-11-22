//! List view — composed over `ViewCore`.
//!
//! Adds the state that only makes sense for a tabular view: per-column sorting,
//! column visibility, the infinite-scroll page size and the derived signals the
//! table component needs.
//!
//! Nothing here re-implements data fetching, filtering or selection —
//! all of that lives in `ViewCore` and is reached via the `ViewService`
//! trait.

use std::collections::HashSet;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::view_core::{ViewCore, ViewService};
use crate::api::meta::{
    Column, ViewSpec, can_create, filter_fields as all_filter_fields, list_columns,
    standard_filter_fields, view_modes,
};
use crate::api::types::EngagementPage;
use crate::api::{DocStore, Filter};

/// Rows fetched by the first request and by every scroll-triggered append.
///
/// Small enough that the first paint is cheap, large enough to fill a typical
/// viewport before the virtualizer asks for the next page.
pub const LIST_PAGE_SIZE: u32 = 20;

// ────────────────────────────────────────────────────────────────────────────
// Pure helpers
// ────────────────────────────────────────────────────────────────────────────

/// Parse `"fieldname asc|desc"` into `(fieldname, is_desc)`.
/// Missing direction defaults to ascending.
///
/// ```
/// use ferris_web::views::services::list_service::split_order;
///
/// assert_eq!(split_order("creation desc"), ("creation".to_string(), true));
/// assert_eq!(split_order("name"), ("name".to_string(), false));
/// assert_eq!(split_order(""), ("name".to_string(), false));
/// ```
pub fn split_order(order: &str) -> (String, bool) {
    let mut it = order.split_whitespace();
    let field = it.next().unwrap_or("name").to_string();
    let desc = it
        .next()
        .map(|d| d.eq_ignore_ascii_case("desc"))
        .unwrap_or(false);
    (field, desc)
}

/// The list-specific slice of the persisted `SavedView.extra`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ListExtra {
    #[serde(default)]
    order_by: Option<String>,
    #[serde(default)]
    visible_columns: Option<Vec<String>>,
    /// Filters made in the quick rows. The advanced dropdown persists through
    /// `SavedView::filters`; keeping these here is what lets the two coexist.
    #[serde(default)]
    quick_filters: Option<Vec<Filter>>,
}

/// Writes a stored `extra` over the list's own state.
///
/// Unconditional on purpose: a doctype with nothing saved gets the defaults back
/// rather than inheriting the previous doctype's columns and order. Skipping
/// the write when the stored value is absent is what made those leak.
fn apply_extra(
    value: &Value,
    order_by: &RwSignal<String>,
    visible_cols: &RwSignal<HashSet<String>>,
    quick_filters: &RwSignal<Vec<Filter>>,
) {
    let extra: ListExtra = serde_json::from_value(value.clone()).unwrap_or_default();
    order_by.set(match extra.order_by {
        Some(ob) if !ob.is_empty() => ob,
        _ => "creation desc".to_string(),
    });
    visible_cols.set(
        extra
            .visible_columns
            .map(|c| c.into_iter().collect())
            .unwrap_or_default(),
    );
    quick_filters.set(extra.quick_filters.unwrap_or_default());
}

// ────────────────────────────────────────────────────────────────────────────
// ListService
// ────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ListService {
    pub core: ViewCore,

    // ── List-specific state ─────────────────────────────────────────────
    pub order_by: RwSignal<String>,
    pub visible_cols: RwSignal<HashSet<String>>,
    /// Filters edited by the quick rows. Separate from `core.query.filters`
    /// (the advanced dropdown) so one never replaces the other.
    pub quick_filters: RwSignal<Vec<Filter>>,
    /// Show only documents the user liked (the header heart).
    pub only_liked: RwSignal<bool>,
    /// Like and comment state of the loaded rows.
    pub engagement: RwSignal<EngagementPage>,
    /// A like write is in flight.
    pub pending_like: RwSignal<bool>,

    // ── Derived (Memo, no RwSignal + Effect) ────────────────────────────
    pub all_columns: Memo<Vec<Column>>,
    pub visible_columns: Memo<Vec<Column>>,
    /// Seed fields for the external quick rows (`in_standard_filter` plus the
    /// doctype's Select/Check fields).
    pub standard_filters: Memo<Vec<Column>>,
    /// Every field the advanced filter builder may target, independent of
    /// `in_list_view`.
    pub filter_fields: Memo<Vec<Column>>,
    pub sort_state: Memo<(String, bool)>,
}

impl ViewService for ListService {
    fn core(&self) -> &ViewCore {
        &self.core
    }
}

impl ListService {
    pub const VIEW_ID: &'static str = "list";

    pub fn new(doctype: Signal<String>, module: Signal<String>) -> Self {
        // ── 1. Signals specific to the list view ────────────────────────
        let order_by = RwSignal::new("creation desc".to_string());
        let visible_cols = RwSignal::new(HashSet::<String>::new());
        let quick_filters = RwSignal::new(Vec::<Filter>::new());
        let only_liked = RwSignal::new(false);
        let engagement = RwSignal::new(EngagementPage::default());
        let pending_like = RwSignal::new(false);

        // The list's restored extras are applied while the core builds its very
        // first query. Building the core first and writing these signals from a
        // later effect left the first `/list` without `order_by`, so a plain
        // list visit always fetched twice.
        //
        // The infinite mode fetches `LIST_PAGE_SIZE` rows and appends the next
        // pages at the current offset; it never rewrites `limit_start`, so the
        // fetch effect only fires when the query itself changes.
        let core =
            ViewCore::new_infinite_with(doctype, module, Self::VIEW_ID, move |query, saved| {
                apply_extra(&saved.extra, &order_by, &visible_cols, &quick_filters);
                query.limit_start = 0;
                query.limit_page_length = u64::from(LIST_PAGE_SIZE);
                query.order_by = Some(order_by.get_untracked());
                query.quick_filters = quick_filters.get_untracked();
                query.only_liked = false;
                // The list no longer renders a global search box. A term
                // restored from an older save would filter the result set with
                // no visible control, so drop it (the persistence effect then
                // clears it from storage too).
                query.search.clear();
            });

        // ── 2. Restore list extras when the doctype changes ─────────────
        // Advanced filters and search are handled by ViewCore; the quick rows
        // ride in `extra`. The router keeps this component alive when only the
        // params change, so neither may leak from
        // the previous doctype into the next one.
        {
            let key = core.view_key;
            let core_for_restore = core.clone();
            let applied = RwSignal::new(Some(core.view_key.get_untracked()));
            Effect::new(move |_| {
                let key = key.get();
                if applied.get_untracked().as_deref() == Some(key.as_str()) {
                    return;
                }
                applied.set(Some(key));
                apply_extra(
                    &core_for_restore.restored_extra(),
                    &order_by,
                    &visible_cols,
                    &quick_filters,
                );
                // Not part of `extra`, and it belongs to the document set that
                // was on screen, not to the one the URL now names.
                only_liked.set(false);
                // `ViewCore` has already restored the previous key's filters
                // and search at this point. Clear the term for the same reason
                // as in `configure`, and seed the new quick rows immediately
                // instead of waiting for the bridge effect and a second fetch.
                core_for_restore.query.update(|query| {
                    query.search.clear();
                    query.quick_filters = quick_filters.get_untracked();
                });
            });
        }

        // ── 2b. Drop quick filters whose field is no longer a seed ──────
        // A restored filter for a field the doctype no longer exposes would
        // keep narrowing the result set with no row to show or clear it. Wait
        // for the metadata of *this* doctype: on a key change the signal still
        // holds the previous doctype's metadata until its fetch lands.
        {
            let qf = quick_filters;
            Effect::new(move |_| {
                let Some(meta) = core.meta.get() else {
                    return;
                };
                if meta.name != core.doctype.get() {
                    return;
                }
                let valid: HashSet<String> = standard_filter_fields(&meta)
                    .into_iter()
                    .map(|column| column.fieldname)
                    .collect();
                let current = qf.get_untracked();
                let kept: Vec<Filter> = current
                    .iter()
                    .filter(|filter| valid.contains(&filter.field))
                    .cloned()
                    .collect();
                if kept != current {
                    qf.set(kept);
                }
            });
        }

        // ── 3. Push list state into core.query (limit/order) ────────────
        // Advanced filters and search are owned by the core; order_by,
        // only_liked and quick_filters are owned by us. This effect is the only
        // bridge. The offset always stays
        // at zero: appends are driven by `ViewCore::load_more`, which asks for
        // the accumulated length instead of re-fetching the whole prefix.
        {
            let q = core.query;
            Effect::new(move |_| {
                q.update(|qv| {
                    qv.limit_start = 0;
                    qv.limit_page_length = u64::from(LIST_PAGE_SIZE);
                    qv.order_by = Some(order_by.get());
                    qv.only_liked = only_liked.get();
                    qv.quick_filters = quick_filters.get();
                });
            });
        }

        // ── 4. Persist list extras whenever they change ─────────────────
        // The core persists advanced filters + search; the quick filters are
        // list-specific and therefore part of `extra`.
        {
            let core_for_save = core.clone();
            Effect::new(move |_| {
                let extra = ListExtra {
                    order_by: Some(order_by.get()),
                    visible_columns: Some(visible_cols.get().into_iter().collect()),
                    quick_filters: Some(quick_filters.get()),
                };
                // `to_value` on a plain struct never fails; the fallback
                // keeps the signature honest without naming a null variant.
                let value = serde_json::to_value(extra).unwrap_or_default();
                core_for_save.persist_extra(value);
            });
        }

        // ── 5. Derived signals ──────────────────────────────────────────
        let all_columns = Memo::new(move |_| {
            core.meta
                .get()
                .as_ref()
                .map(list_columns)
                .unwrap_or_default()
        });

        let visible_columns = Memo::new(move |_| {
            let all = all_columns.get();
            let set = visible_cols.get();
            if set.is_empty() {
                all
            } else {
                all.into_iter()
                    .filter(|c| set.contains(&c.fieldname))
                    .collect()
            }
        });

        let standard_filters = Memo::new(move |_| {
            core.meta
                .get()
                .as_ref()
                .map(standard_filter_fields)
                .unwrap_or_default()
        });

        let filter_fields = Memo::new(move |_| {
            core.meta
                .get()
                .as_ref()
                .map(all_filter_fields)
                .unwrap_or_default()
        });

        let sort_state = Memo::new(move |_| split_order(&order_by.get()));

        Self {
            core,
            order_by,
            visible_cols,
            quick_filters,
            only_liked,
            engagement,
            pending_like,
            all_columns,
            visible_columns,
            standard_filters,
            filter_fields,
            sort_state,
        }
    }

    // ── Filtering / search ──────────────────────────────────────────────
    // Names deliberately differ from `ViewService::{set_filters, on_search}`
    // so a concrete `ListService` never accidentally calls the trait's
    // default.

    /// Replace the active filters. The core starts the result set again at
    /// offset zero because the query changed.
    pub fn apply_filters(&self, filters: Vec<Filter>) {
        self.core.set_filters(filters);
    }

    /// Replace the filters owned by the quick rows. The bridge effect copies
    /// them into `core.query.quick_filters`, next to (not over) the advanced
    /// dropdown's `core.query.filters`.
    pub fn apply_quick_filters(&self, filters: Vec<Filter>) {
        self.quick_filters.set(filters);
    }

    /// Search input handler.
    pub fn search(&self, term: String) {
        self.core.set_search(term);
    }

    /// Drop every restriction at once — filters, search box and the "only my
    /// likes" toggle — and leave the list showing the whole table. A single
    /// `query` update so the memo-driven fetch fires once (two calls would
    /// reload twice and flash the loading skeleton).
    pub fn clear_filtering(&self) {
        self.only_liked.set(false);
        self.quick_filters.set(Vec::new());
        self.core.query.update(|q| {
            q.filters = Vec::new();
            q.quick_filters = Vec::new();
            q.search = String::new();
        });
    }

    // ── Sorting ─────────────────────────────────────────────────────────

    /// Order by an explicit field + direction (order-by group).
    pub fn set_sort(&self, field: String, desc: bool) {
        self.order_by
            .set(format!("{} {}", field, if desc { "desc" } else { "asc" }));
    }

    /// Cycle the sort direction for a column: none → asc → desc → asc …
    pub fn sort_column(&self, fieldname: String) {
        let (current, desc) = self.sort_state.get_untracked();
        let next = if current == fieldname {
            format!("{} {}", fieldname, if desc { "asc" } else { "desc" })
        } else {
            format!("{} asc", fieldname)
        };
        self.order_by.set(next);
    }

    // ── Column visibility ───────────────────────────────────────────────

    pub fn toggle_column(&self, fieldname: String, on: bool) {
        let all: Vec<String> = self
            .all_columns
            .get_untracked()
            .into_iter()
            .map(|column| column.fieldname)
            .collect();
        self.visible_cols.update(|set| {
            *set = toggled_columns(&all, set, &fieldname, on);
        });
    }

    /// Restore the default set of columns (all `in_list_view` fields).
    pub fn reset_columns(&self) {
        self.visible_cols.set(HashSet::new());
    }

    // ── Likes ───────────────────────────────────────────────────────────

    /// Toggle the "only my likes" filter. The core refetches from zero because
    /// the document set changes.
    pub fn toggle_only_liked(&self) {
        let next = !self.only_liked.get_untracked();
        self.only_liked.set(next);
    }

    /// Like a document, or unlike it when already liked, and fold the result
    /// back into the page state.
    ///
    /// The server decides the outcome and reports the new count, so the heart
    /// does not have to guess whether the click landed. The write is skipped
    /// while one is already in flight, which is what a double click would
    /// otherwise turn into.
    pub fn toggle_like(&self, doctype: &str, name: &str) {
        if self.pending_like.get_untracked() {
            return;
        }
        self.pending_like.set(true);

        let (doctype, name) = (doctype.to_string(), name.to_string());
        let service = self.clone();
        leptos::task::spawn_local(async move {
            let outcome = match DocStore::runtime() {
                Ok(store) => store.toggle_like(&doctype, &name).await,
                Err(e) => Err(e),
            };
            service.pending_like.set(false);
            if let Ok(like) = outcome {
                service.engagement.update(|page| {
                    if like.liked {
                        page.liked.insert(name.clone());
                    } else {
                        page.liked.remove(&name);
                    }
                    page.like_counts.insert(name, like.count);
                });
            }
        });
    }

    // ── Selection (thin wrappers over the core) ─────────────────────────

    pub fn toggle_all(&self, on: bool) {
        self.core.toggle_all_on_page(on);
    }

    pub fn toggle_row(&self, name: String, on: bool) {
        self.core.toggle_row(name, on);
    }

    // ── Export ──────────────────────────────────────────────────────────

    pub fn export_selection(&self) {
        let cols = self.visible_columns.get_untracked();
        self.core.bulk_export.run(cols);
    }

    pub fn export_current_page(&self) {
        let cols = self.visible_columns.get_untracked();
        self.core.export_page.run(cols);
    }

    // ── Metadata helpers ────────────────────────────────────────────────

    pub fn view_modes(&self) -> Vec<ViewSpec> {
        self.core
            .meta
            .get()
            .as_ref()
            .map(|m| view_modes(m, &self.core.module.get(), &self.core.doctype.get()))
            .unwrap_or_default()
    }

    pub fn can_create(&self, user_roles: &[String]) -> bool {
        self.core
            .meta
            .get()
            .as_ref()
            .map(|m| can_create(m, user_roles))
            .unwrap_or(false)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

/// The column set after toggling `fieldname`, with the empty set read as "all".
///
/// `visible_cols` stores an empty set for "the declared columns", which is what
/// a fresh view has. Hiding a column from that state must *materialise* the
/// current set first; otherwise `remove` on an empty set would still mean "all"
/// and the column would stay on screen. The last visible column is never
/// removed: an empty table is not a view.
fn toggled_columns(
    all: &[String],
    current: &HashSet<String>,
    fieldname: &str,
    on: bool,
) -> HashSet<String> {
    // No columns yet (metadata in flight): a toggle has nothing to materialise
    // and must not collapse the set to the one field being turned on.
    if all.is_empty() {
        return current.clone();
    }
    let mut next: HashSet<String> = if current.is_empty() {
        all.iter().cloned().collect()
    } else {
        current.clone()
    };
    if on {
        next.insert(fieldname.to_string());
    } else if next.len() > 1 {
        next.remove(fieldname);
    }
    next
}

#[cfg(test)]
mod tests {
    use super::{ListExtra, split_order, toggled_columns};
    use crate::api::Filter;
    use crate::api::types::FilterOp;
    use std::collections::HashSet;

    #[test]
    fn quick_filters_round_trip_through_extra() {
        let extra = ListExtra {
            quick_filters: Some(vec![Filter {
                field: "status".into(),
                op: FilterOp::Equals,
                value: "Open".into(),
                or: false,
            }]),
            ..Default::default()
        };
        let value = serde_json::to_value(&extra).expect("extra serializes");
        let back: ListExtra = serde_json::from_value(value).expect("extra deserializes");
        assert_eq!(back.quick_filters, extra.quick_filters);
    }

    /// A save written before quick filters existed has no key for them; it must
    /// restore as an empty list, not fail the whole extra.
    #[test]
    fn old_extra_without_quick_filters_is_empty() {
        let value = serde_json::json!({ "order_by": "modified desc" });
        let back: ListExtra = serde_json::from_value(value).expect("old extra deserializes");
        assert_eq!(back.quick_filters, None);
    }

    #[test]
    fn parses_direction() {
        assert_eq!(split_order("creation desc"), ("creation".into(), true));
        assert_eq!(split_order("creation ASC"), ("creation".into(), false));
        assert_eq!(split_order("name"), ("name".into(), false));
    }

    #[test]
    fn empty_defaults_to_name_asc() {
        assert_eq!(split_order(""), ("name".into(), false));
        assert_eq!(split_order("   "), ("name".into(), false));
    }

    #[test]
    fn extra_whitespace_is_ignored() {
        assert_eq!(
            split_order("  modified   desc  "),
            ("modified".into(), true)
        );
    }

    /// The implicit "all" is materialised before a column is hidden: removing
    /// from an empty set used to leave the same set, so the checkbox turned
    /// itself back on.
    #[test]
    fn hiding_a_column_from_the_implicit_all_keeps_the_rest_visible() {
        let all = vec![
            "name".to_string(),
            "status".to_string(),
            "owner".to_string(),
        ];
        let next = toggled_columns(&all, &HashSet::new(), "status", false);
        assert!(!next.contains("status"));
        assert!(next.contains("name") && next.contains("owner"));
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn the_last_column_cannot_be_hidden() {
        let all = vec!["name".to_string(), "status".to_string()];
        let one: HashSet<String> = ["name".to_string()].into_iter().collect();
        let next = toggled_columns(&all, &one, "name", false);
        assert_eq!(next, one, "an empty table is not a view");
    }

    #[test]
    fn showing_a_column_adds_it_to_the_materialised_set() {
        let all = vec!["name".to_string(), "status".to_string()];
        let one: HashSet<String> = ["name".to_string()].into_iter().collect();
        let next = toggled_columns(&all, &one, "status", true);
        assert!(next.contains("name") && next.contains("status"));
    }
}
