//! Route-derived breadcrumb trails for the shared top bar.
//!
//! The trail is built here, not in `ferris-web-ui`, because only the Desk owns
//! the route table: an ancestor is a link only when it has a real page. The
//! module segment of `/desk/Users/User` has none — linking it would answer 404
//! from the catch-all route — so it stays plain text.
//!
//! The shapes:
//!
//! - `/desk/<module>/<doctype>[/<name>]` — `Escritorio` links home, the module is
//!   a label, the doctype links to its list and the document name (or the doctype
//!   itself, on the list) is the current page.
//! - `/desk/<module>/<doctype>/<view>` — a collection view: the doctype still
//!   links back to its list and the view's Spanish name (`Kanban`, `Árbol`, …)
//!   is the current page.
//! - `/desk/<module>/<doctype>/layout` — the same trail, but the last crumb is
//!   `Diseño`: the page is the DocType's layout editor, not a record called
//!   "layout". The module and that mode have no page of their own, so they stay
//!   labels.
//! - `/report/<name>`, `/kanban/<doctype>`, `/tree/<doctype>`, `/gantt/<doctype>`
//!   — legacy section routes: the section is a label (there is no section index
//!   page) and the item is the current page.
//! - `/calendar`, `/settings[/<rest>]` — current page, with `Ajustes` as a
//!   label when a section is open.
//! - Everything else — the desk home, plugin dashboards, website pages — has no
//!   trail: a dashboard is the page, not a step in a navigation path.
//!
//! Labels are Spanish because the trail is user-visible chrome; module, doctype
//! and document names keep the casing they were given, and `Kanban` stays as it
//! is (it names the board, not a translated concept).

use crate::components::ui::breadcrumb::Crumb;
use crate::views::services::form_service::LAYOUT_SEGMENT;
use ferris_web_shared::route::percent_decode;

/// The trail for `path`, in reading order.
pub(super) fn trail(path: &str) -> Vec<Crumb> {
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();

    match segments.as_slice() {
        ["desk", module, doctype] => {
            vec![link("Escritorio", "/desk"), plain(module), current(doctype)]
        }
        ["desk", module, doctype, LAYOUT_SEGMENT] => vec![
            link("Escritorio", "/desk"),
            plain(module),
            // The list is a real page and the way out of the editor, so it is
            // the one link in the tail. `/desk/<module>` has no page at all and
            // the mode segment is the page itself.
            link(doctype, format!("/desk/{module}/{doctype}")),
            current("Diseño"),
        ],
        ["desk", module, doctype, name] if collection_label(name).is_some() => vec![
            link("Escritorio", "/desk"),
            plain(module),
            link(doctype, format!("/desk/{module}/{doctype}")),
            current(collection_label(name).unwrap_or_default()),
        ],
        ["desk", module, doctype, name] => vec![
            link("Escritorio", "/desk"),
            plain(module),
            link(doctype, format!("/desk/{module}/{doctype}")),
            // `new` is the route marker for an unsaved document, not a record
            // called "new": the trail is Spanish chrome, so it reads "Nuevo"
            // (the mobile header prefixes the DocType to it: "Task · Nuevo").
            if *name == "new" {
                current("Nuevo")
            } else {
                current(name)
            },
        ],
        ["report", name] => vec![plain("Informe"), current(name)],
        ["kanban", doctype] => vec![plain("Kanban"), current(doctype)],
        ["tree", doctype] => vec![plain("Árbol"), current(doctype)],
        ["gantt", doctype] => vec![plain("Gantt"), current(doctype)],
        ["calendar"] => vec![current("Calendario")],
        ["settings"] => vec![current("Ajustes")],
        ["settings", rest @ ..] => vec![
            plain("Ajustes"),
            current(settings_section(rest.last().copied().unwrap_or_default())),
        ],
        _ => Vec::new(),
    }
}

fn link(label: &str, href: impl Into<String>) -> Crumb {
    Crumb {
        label: crumb_label(label),
        href: Some(href.into()),
        is_current: false,
    }
}

fn plain(label: &str) -> Crumb {
    Crumb {
        label: crumb_label(label),
        href: None,
        is_current: false,
    }
}

fn current(label: &str) -> Crumb {
    Crumb {
        label: crumb_label(label),
        href: None,
        is_current: true,
    }
}

/// The visible name of a collection-view segment, or `None` when the segment is
/// a document name.
///
/// The list is the one exception: its crumb is `Lista`, but `/desk/m/d/list`
/// and the index name the same view. A doctype whose row happens to be named
/// `kanban` is shadowed by the route keyword (see `DocRoute`), so the crumb
/// agrees with the page rather than with the row.
fn collection_label(segment: &str) -> Option<&'static str> {
    match segment {
        "list" => Some("Lista"),
        "kanban" => Some("Kanban"),
        "calendar" => Some("Calendario"),
        "tree" => Some("Árbol"),
        "gantt" => Some("Gantt"),
        "report" => Some("Informe"),
        _ => None,
    }
}

/// The visible name of a settings section. Unlike a free path segment these
/// are a closed set, so the crumb reads in the same language as the tab
/// (`Seguridad`, not `Security`).
fn settings_section(segment: &str) -> &str {
    match segment {
        "permissions" => "Permisos",
        "security" => "Seguridad",
        "about" => "Acerca de",
        "general" => "General",
        other => other,
    }
}

/// A path segment as crumb text: percent-decoded, first letter capitalised and
/// the rest left untouched.
///
/// Not title case on purpose: module and doctype names already carry their own
/// casing (`CRM`, `Sales Invoice`) and a document id such as `TASK-0001` must
/// not be re-cased or split on its punctuation.
fn crumb_label(segment: &str) -> String {
    let decoded = percent_decode(segment);
    let mut chars = decoded.chars();
    match chars.next() {
        Some(first) => {
            let mut out: String = first.to_uppercase().collect();
            out.push_str(chars.as_str());
            out
        }
        None => decoded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(crumbs: &[Crumb]) -> Vec<&str> {
        crumbs.iter().map(|crumb| crumb.label.as_str()).collect()
    }

    fn hrefs(crumbs: &[Crumb]) -> Vec<Option<&str>> {
        crumbs.iter().map(|crumb| crumb.href.as_deref()).collect()
    }

    /// The list view: home link, module label, doctype as the current page.
    #[test]
    fn a_list_route_ends_on_the_doctype_and_the_module_is_not_a_link() {
        let crumbs = trail("/desk/Users/User");
        assert_eq!(labels(&crumbs), ["Escritorio", "Users", "User"]);
        assert_eq!(hrefs(&crumbs), [Some("/desk"), None, None]);
        assert!(crumbs[2].is_current);
        assert!(!crumbs[0].is_current && !crumbs[1].is_current);
    }

    /// The form view: the doctype links back to its list and the document name
    /// is the current page.
    #[test]
    fn a_form_route_links_the_doctype_list_and_ends_on_the_name() {
        let crumbs = trail("/desk/Users/User/Administrator");
        assert_eq!(
            labels(&crumbs),
            ["Escritorio", "Users", "User", "Administrator"]
        );
        assert_eq!(
            hrefs(&crumbs),
            [Some("/desk"), None, Some("/desk/Users/User"), None]
        );
        assert!(crumbs[3].is_current);
    }

    /// `/new` is the route marker for an unsaved document, not a record called
    /// "new": the trail is Spanish chrome and reads "Nuevo".
    #[test]
    fn the_new_document_marker_is_labelled_in_spanish() {
        let crumbs = trail("/desk/Users/User/new");
        assert_eq!(labels(&crumbs), ["Escritorio", "Users", "User", "Nuevo"]);
        assert!(crumbs[3].is_current);
        assert_eq!(crumbs[2].href.as_deref(), Some("/desk/Users/User"));
    }

    /// `/layout` is the DocType's layout editor, not a document called "layout":
    /// the trail names the mode, and the only links are the two real pages
    /// (the desk home and the DocType's list).
    #[test]
    fn the_layout_route_ends_on_the_layout_label() {
        let crumbs = trail("/desk/Users/User/layout");
        assert_eq!(labels(&crumbs), ["Escritorio", "Users", "User", "Diseño"]);
        assert_eq!(
            hrefs(&crumbs),
            [Some("/desk"), None, Some("/desk/Users/User"), None]
        );
        assert!(crumbs[3].is_current);
    }

    /// A collection view keeps the doctype link and names the view, instead of
    /// pretending "kanban" is a document called Kanban.
    #[test]
    fn a_collection_view_is_named_and_the_doctype_still_links_back() {
        for (segment, label) in [
            ("list", "Lista"),
            ("kanban", "Kanban"),
            ("calendar", "Calendario"),
            ("tree", "Árbol"),
            ("gantt", "Gantt"),
            ("report", "Informe"),
        ] {
            let crumbs = trail(&format!("/desk/Support/Task/{segment}"));
            assert_eq!(
                labels(&crumbs),
                ["Escritorio", "Support", "Task", label],
                "{segment}"
            );
            assert_eq!(
                hrefs(&crumbs),
                [Some("/desk"), None, Some("/desk/Support/Task"), None],
                "{segment}"
            );
            assert!(crumbs[3].is_current);
        }
    }

    /// No trail on the desk home, plugin dashboards or website pages.
    #[test]
    fn dashboards_and_plugin_pages_have_no_trail() {
        for path in ["/", "/desk", "/planta", "/app/ops-overview", "/status"] {
            assert!(trail(path).is_empty(), "{path} should have no trail");
        }
    }

    /// The section routes keep their item as the current page; the section is a
    /// label because there is no section index route to link to.
    #[test]
    fn section_routes_label_the_section_and_end_on_the_item() {
        let report = trail("/report/ops");
        assert_eq!(labels(&report), ["Informe", "Ops"]);
        assert_eq!(hrefs(&report), [None, None]);
        assert!(report[1].is_current);

        let kanban = trail("/kanban/Task");
        assert_eq!(labels(&kanban), ["Kanban", "Task"]);
        assert!(kanban[1].is_current);

        let settings = trail("/settings/general");
        assert_eq!(labels(&settings), ["Ajustes", "General"]);
        assert!(settings[1].is_current);
        assert_eq!(
            labels(&trail("/settings/security")),
            ["Ajustes", "Seguridad"]
        );
        assert_eq!(
            labels(&trail("/settings/permissions")),
            ["Ajustes", "Permisos"]
        );

        let calendar = trail("/calendar");
        assert_eq!(labels(&calendar), ["Calendario"]);
        assert!(calendar[0].is_current);
    }

    /// Labels preserve the name's own casing instead of title-casing it, which
    /// would turn `CRM` into `Crm` and `TASK-0001` into `Task 0001`.
    #[test]
    fn labels_keep_their_own_casing() {
        let crumbs = trail("/desk/Support/CRM/TASK-0001");
        assert_eq!(
            labels(&crumbs),
            ["Escritorio", "Support", "CRM", "TASK-0001"]
        );
    }

    /// Percent-encoded segments are decoded so a document name reads as it was
    /// written (`ada@example.com`, not `ada%40example.com`).
    #[test]
    fn percent_encoded_segments_are_decoded() {
        let crumbs = trail("/desk/Users/User/ada%40example.com");
        assert_eq!(
            labels(&crumbs),
            ["Escritorio", "Users", "User", "Ada@example.com"]
        );

        // A literal `%` that is not an escape survives, and the text after it
        // is not swallowed.
        assert_eq!(percent_decode("100%25%"), "100%%");
    }
}
