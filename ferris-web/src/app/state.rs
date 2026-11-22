//! Non-component Desk state: the feed refreshes, the route rules, and the
//! workspace shapes the sidebar, the grid and the module menu share.
//!
//! Split out of `app/mod.rs` to keep the component readable (and the module
//! under the repository's file-size limit). Nothing here renders: these are the
//! functions the shell calls on boot, on sign-in and when the notification panel
//! opens, plus the data types other modules name as `crate::app::Workspace`.
//! `mod.rs` re-exports them at the historical `crate::app::…` paths, so no
//! caller had to change.

use leptos::prelude::*;

use crate::api::drive;
use crate::desk::session::{
    self as desk_session, DeskIcon, DeskModule, NotificationFeed, NotificationSettings,
    SiteSettings,
};
use crate::ext::AppUi;

use super::{DESK_PATH, LOGIN_PATH};

/// Fetch the signed-in user's desk and populate the sidebar workspaces and the
/// grid's icons.
pub(crate) fn refresh_workspaces(
    workspaces: RwSignal<Vec<Workspace>>,
    icons: RwSignal<Vec<DeskIcon>>,
) {
    drive(async move {
        if let Ok(layout) = desk_session::desk_layout().await {
            workspaces.set(
                layout
                    .workspaces
                    .into_iter()
                    .map(module_to_workspace)
                    .collect(),
            );
            icons.set(layout.icons);
        }
    });
}

/// Refresh the notification feed and the preferences that decide how it is
/// presented. Called on sign-in, whenever the panel opens and after the user
/// saves their settings, so the sidebar badge and the panel never disagree.
pub(crate) fn refresh_notifications(
    notifications: RwSignal<NotificationFeed>,
    settings: RwSignal<NotificationSettings>,
) {
    drive(async move {
        if let Ok(feed) = desk_session::notifications().await {
            notifications.set(feed);
        }
        if let Ok(fresh) = desk_session::notification_settings().await {
            settings.set(fresh);
        }
    });
}

/// Refresh the install-wide presentation settings (`/api/site-settings`), which
/// drive the tab title and the navbar announcement. Called wherever the desk
/// data is seeded — on boot and on sign-in — so the chrome reflects the current
/// `Website Settings`/`Navbar Settings` rows without a reload.
pub(crate) fn refresh_site(site: RwSignal<SiteSettings>) {
    drive(async move {
        if let Ok(fresh) = desk_session::site_settings().await {
            site.set(fresh);
        }
    });
}

/// Build the `/login?next=...` URL for a page the browser was refused.
#[cfg(target_arch = "wasm32")]
pub(super) fn login_redirect(target: &str) -> String {
    format!("{LOGIN_PATH}?next={}", encode_query(target))
}

/// Whether a route renders without the desk chrome: the standalone sign-in
/// page, the desk grid (a rail of workspaces beside the thing that picks
/// workspaces is noise), or a plugin website page that declared
/// `kind: "bare"` (see [`crate::views::plugin_page::declared_bare_path`]).
///
/// A trailing slash is insignificant — `/desk/` is still the desk — which is
/// what a bare `== DESK_PATH` check gets wrong.
pub(super) fn page_wants_bare_chrome(apps: &[AppUi], path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path == LOGIN_PATH
        || path == DESK_PATH
        || crate::views::plugin_page::declared_bare_path(apps, path)
}

/// Percent-encode a value for a query string: RFC 3986 unreserved characters
/// pass through untouched and everything else is percent-escaped. Space is
/// `%20` rather than `+`, which the `?next=` decoding accepts either way.
#[cfg(target_arch = "wasm32")]
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Tab title carrying the unread count, so a backgrounded Desk still advertises
/// what is waiting. It honours the same `desktop` preference as the bell badge,
/// and the site name comes from `Website Settings.title_prefix` (falling back to
/// `"Ferris Desk"` when the setting is empty or not yet loaded).
pub(super) fn desk_title(
    feed: RwSignal<NotificationFeed>,
    settings: RwSignal<NotificationSettings>,
    site: RwSignal<SiteSettings>,
) -> String {
    let unread = if settings.with(|s| s.desktop) {
        feed.with(|f| f.unread)
    } else {
        0
    };
    let name = site.with(|s| {
        let prefix = s.title_prefix.trim();
        if prefix.is_empty() {
            "Ferris Desk".to_string()
        } else {
            prefix.to_string()
        }
    });
    if unread > 0 {
        format!("({unread}) {name}")
    } else {
        name
    }
}

fn module_to_workspace(m: DeskModule) -> Workspace {
    let module = m.module.clone();
    Workspace {
        name: m.name.clone(),
        label: if m.label.trim().is_empty() {
            m.name
        } else {
            m.label
        },
        module,
        icon: m.icon,
        items: m.items.into_iter().map(workspace_entry_to_item).collect(),
    }
}

fn workspace_entry_to_item(entry: crate::desk::session::DeskSidebarEntry) -> WorkspaceItem {
    let kind = match entry.kind.as_str() {
        "doctype" => WorkspaceItemKind::Doctype,
        "url" => WorkspaceItemKind::Url,
        "group" => WorkspaceItemKind::Group,
        _ => WorkspaceItemKind::Route,
    };
    WorkspaceItem {
        name: entry
            .name
            .clone()
            .or_else(|| entry.doctype.clone())
            .unwrap_or_else(|| entry.label.clone()),
        label: entry.label,
        kind,
        doctype: entry.doctype,
        icon: entry.icon,
        route: entry.route,
        href: entry.href,
        open_in_new_tab: entry.open_in_new_tab,
        children: entry
            .children
            .into_iter()
            .map(workspace_entry_to_item)
            .collect(),
    }
}

/// What a sidebar item renders as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WorkspaceItemKind {
    /// Link to a doctype's list view.
    Doctype,
    /// Internal desk route.
    Route,
    /// External URL (optionally in a new tab).
    Url,
    /// Group holding nested items (dropdown).
    Group,
}

/// Workspace definition (app-declared or auto-derived from modules).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Workspace {
    pub name: String,
    pub label: String,
    pub module: String,
    pub icon: Option<String>,
    pub items: Vec<WorkspaceItem>,
}

impl Workspace {
    /// Where selecting this workspace takes you: its first link, groups walked
    /// depth-first. External links are skipped so that switching workspace
    /// never navigates off the desk.
    ///
    /// `None` means the workspace has no internal link at all, which is allowed:
    /// picking it still swaps the sidebar, it just has nowhere to navigate to.
    pub fn first_route(&self) -> Option<String> {
        fn walk(items: &[WorkspaceItem]) -> Option<String> {
            items.iter().find_map(|item| match item.kind {
                WorkspaceItemKind::Group => walk(&item.children),
                WorkspaceItemKind::Doctype | WorkspaceItemKind::Route => item.route.clone(),
                WorkspaceItemKind::Url => None,
            })
        }
        walk(&self.items)
    }
}

/// One sidebar link inside a workspace.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceItem {
    pub name: String,
    pub label: String,
    pub kind: WorkspaceItemKind,
    pub doctype: Option<String>,
    pub icon: Option<String>,
    pub route: Option<String>,
    pub href: Option<String>,
    pub open_in_new_tab: bool,
    pub children: Vec<WorkspaceItem>,
}
