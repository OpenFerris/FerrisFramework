//! Workspace item rows → the nested tree the registry stores.

use ferris_core::workspaces::WorkspaceItemKind;

use crate::descriptor::registries::workspace_spec;
use crate::ferris::framework::descriptor::{Workspace, WorkspaceItem};

/// The flattened `id`/`parent` rows rebuild into the nested tree
/// `ferris-core` renders: a group keeps its children, links keep type.
#[test]
fn workspace_tree_rebuilds_nested_items() {
    let workspace = Workspace {
        name: "operations".into(),
        label: "Operations".into(),
        module: "test".into(),
        icon: Some("Settings".into()),
        folder: None,
        items: vec![
            WorkspaceItem {
                id: 0,
                parent: None,
                label: "Tasks".into(),
                icon: Some("SquareCheck".into()),
                doctype: Some("Task".into()),
                route: None,
                href: None,
                open_in_new_tab: false,
            },
            WorkspaceItem {
                id: 1,
                parent: None,
                label: "Docs".into(),
                icon: Some("BookOpen".into()),
                doctype: None,
                route: None,
                href: Some("https://wiki.test".into()),
                open_in_new_tab: true,
            },
            WorkspaceItem {
                id: 2,
                parent: None,
                label: "Enlace".into(),
                icon: None,
                doctype: None,
                route: Some("/dashboard".into()),
                href: None,
                open_in_new_tab: false,
            },
            WorkspaceItem {
                id: 3,
                parent: None,
                label: "Agrupado".into(),
                icon: Some("Folder".into()),
                doctype: None,
                route: None,
                href: None,
                open_in_new_tab: false,
            },
            WorkspaceItem {
                id: 4,
                parent: Some(3),
                label: "Proyectos".into(),
                icon: None,
                doctype: Some("Project".into()),
                route: None,
                href: None,
                open_in_new_tab: false,
            },
        ],
    };
    let spec = workspace_spec(&workspace);
    assert_eq!(spec.name, "operations");
    assert_eq!(spec.label, "Operations");
    assert_eq!(spec.icon.as_deref(), Some("Settings"));
    assert_eq!(spec.items.len(), 4);

    let task = &spec.items[0];
    assert_eq!(task.doctype.as_deref(), Some("Task"));
    assert_eq!(task.kind(), WorkspaceItemKind::Doctype);
    assert_eq!(task.icon.as_deref(), Some("SquareCheck"));

    let docs = &spec.items[1];
    assert_eq!(docs.href.as_deref(), Some("https://wiki.test"));
    assert!(docs.open_in_new_tab);
    assert_eq!(docs.kind(), WorkspaceItemKind::Url);

    assert_eq!(spec.items[2].kind(), WorkspaceItemKind::Route);

    let group = &spec.items[3];
    assert_eq!(group.kind(), WorkspaceItemKind::Group);
    assert_eq!(group.children.len(), 1);
    assert_eq!(group.children[0].doctype.as_deref(), Some("Project"));
}

/// A row whose parent is dangling/forward is treated as top-level: it must
/// not be dropped, and the rebuild must not loop on its own kind.
#[test]
fn workspace_tree_degrades_dangling_parent() {
    let workspace = Workspace {
        name: "odd".into(),
        label: "Odd".into(),
        module: "test".into(),
        icon: None,
        folder: None,
        items: vec![
            WorkspaceItem {
                id: 0,
                parent: None,
                label: "Top".into(),
                icon: None,
                doctype: Some("Task".into()),
                route: None,
                href: None,
                open_in_new_tab: false,
            },
            WorkspaceItem {
                id: 1,
                parent: Some(9), // dangling
                label: "Dangle".into(),
                icon: None,
                doctype: None,
                route: Some("/lost".into()),
                href: None,
                open_in_new_tab: false,
            },
        ],
    };
    let spec = workspace_spec(&workspace);
    assert_eq!(spec.items.len(), 2);
    assert_eq!(spec.items[1].kind(), WorkspaceItemKind::Route);
}
