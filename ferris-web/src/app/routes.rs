//! The Desk's route table.
//!
//! Split out of `app/mod.rs` (which must stay under the repository's 600-line
//! limit). The table is declared in matching order: every specific route wins,
//! and the trailing catch-all resolves an app-declared dashboard or website
//! page. The server registers those declared paths concretely; in the browser
//! this table is the resolver.

use leptos::prelude::*;
use leptos_router::components::{Route, Routes};
use leptos_router::path;

use crate::desk::{Desktop, LoginPage};
use crate::views::views_components::NotFoundPage;
use crate::views::{
    CalendarView, DocRoute, GanttView, HomeView, KanbanView, ListView, PluginPage, ReportView,
    SettingsView, TreeView,
};

/// Every page the Desk serves.
#[component]
pub fn DeskRoutes() -> impl IntoView {
    view! {
        <Routes fallback=NotFoundPage>
            <Route path=path!("/login") view=LoginPage />
            <Route path=path!("/") view=HomeView />
            <Route path=path!("/desk") view=Desktop />
            <Route path=path!("/desk/:module/:doctype") view=ListView />
            // The third segment is a collection view
            // (`list`/`kanban`/`calendar`/`tree`/`gantt`/`report`), the layout
            // editor (`layout`) or a document name; `DocRoute` decides. One
            // route on purpose: the keywords and a record are the same shape,
            // and only the dispatcher knows which is which.
            <Route path=path!("/desk/:module/:doctype/:name") view=DocRoute />
            // Legacy section routes, kept so an old bookmark still resolves.
            // They render the same views and the top bar's switcher moves the
            // user onto the canonical `/desk/...` URL.
            <Route path=path!("/report/:name") view=ReportView />
            <Route path=path!("/kanban/:doctype") view=KanbanView />
            <Route path=path!("/calendar/:doctype") view=CalendarView />
            <Route path=path!("/calendar") view=CalendarView />
            <Route path=path!("/tree/:doctype") view=TreeView />
            <Route path=path!("/gantt/:doctype") view=GanttView />
            // Both the bare section and its subsections are real pages: the
            // user menu and the first settings tab link to `/settings`, and a
            // `*rest` route alone answers 404 for that exact path.
            <Route path=path!("/settings") view=SettingsView />
            <Route path=path!("/settings/*rest") view=SettingsView />
            // Kept last on purpose: routes are tried in declaration order, so
            // every specific route wins and only paths nothing else claims
            // (plugin dashboards like `/app/ops-overview`, website pages like
            // `/status`) fall through to the plugin page.
            <Route path=path!("/*rest") view=PluginPage />
        </Routes>
    }
}
