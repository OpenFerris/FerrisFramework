//! Route-level dispatch for `/desk/:module/:doctype/:name`.
//!
//! A doctype has one **document view** (`meta.view`: form, dashboard or an
//! app's custom view) and several **collection views**. The URL keeps both: the
//! third segment names a collection view when it is one of the reserved
//! keywords (`list`, `kanban`, `calendar`, `tree`, `gantt`, `report`) and is a
//! document name otherwise.
//!
//! `layout` is the fourth reserved segment, and the only one that renders the
//! form: it opens the DocType's **layout editor** — the same metadata form, with
//! no document selected, editing the shape (custom fields and property setters)
//! rather than a record. It has its own URL so the mode can be linked, bookmarked
//! and shared; the service recognises the segment itself
//! (`FormService::layout_mode`), which is what makes it work on the same `<Route>`
//! definition as a document — the router does not rebuild a view when only the
//! params change.
//!
//! The record path needs no switch here — [`FormView`] already resolves
//! dashboard/custom from the metadata, so `/desk/:module/:doctype/:id` opens the
//! view the doctype declares, and `new` still creates a document.
//!
//! The index (`/desk/:module/:doctype`) renders the list; a doctype whose
//! metadata declares `default_view` opens that view instead, through a
//! one-time redirect in [`ListView`] (see its module docs). The URL of the
//! collection view stays its own, so a bookmark to the index keeps resolving
//! and picking "Lista" is still possible after the redirect.
//!
//! Trade-off: a document whose name is exactly `list`, `kanban`, `calendar`,
//! `tree`, `gantt`, `report` or `layout` is shadowed by the view keyword. That
//! is the price of the short URL; the doctype's naming series never produces
//! those names, and the alternative
//! (`/desk/:module/:doctype/view/:kind`) is the longer form we can add later
//! without breaking these links.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::views::services::form_service::LAYOUT_SEGMENT;
use crate::views::{CalendarView, FormView, GanttView, KanbanView, ListView, ReportView, TreeView};

#[component]
pub fn DocRoute() -> impl IntoView {
    let params = use_params_map();
    let segment = Memo::new(move |_| {
        params.with(|params| params.get("name").unwrap_or_default().to_lowercase())
    });

    view! {
        {move || match segment.get().as_str() {
            "list" => view! { <ListView /> }.into_any(),
            "kanban" => view! { <KanbanView /> }.into_any(),
            "calendar" => view! { <CalendarView /> }.into_any(),
            "tree" => view! { <TreeView /> }.into_any(),
            "gantt" => view! { <GanttView /> }.into_any(),
            "report" => view! { <ReportView /> }.into_any(),
            // The layout editor *is* the form (see the module docs): the flag
            // comes from the route through the service, not through a prop, so
            // this arm only has to keep the segment out of the document path.
            // The `contents` wrapper adds no box — it exists so the route is
            // identifiable in the DOM (`data-name`) without changing the form's
            // own layout.
            LAYOUT_SEGMENT => view! {
                <div class="contents" data-name="DocLayoutRoute">
                    <FormView />
                </div>
            }
            .into_any(),
            _ => view! { <FormView /> }.into_any(),
        }}
    }
}
