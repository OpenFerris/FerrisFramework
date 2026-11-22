use super::{build_tree, count_nodes, default_expanded, filter_tree};
use serde_json::{Value, json};

fn row(name: &str, parent: &str) -> Value {
    json!({ "name": name, "parent_task": parent })
}

#[test]
fn children_attach_under_their_parent() {
    let rows = vec![row("A", ""), row("B", "A"), row("C", "A"), row("D", "B")];
    let tree = build_tree(&rows, "parent_task");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, "A");
    assert_eq!(tree[0].children.len(), 2);
    assert_eq!(tree[0].children[0].name, "B");
    assert_eq!(tree[0].children[0].children[0].name, "D");
    assert_eq!(count_nodes(&tree), 4);
}

/// A parent that is not in the page is a root, not a lost row: hiding a
/// document because its parent was filtered out is worse than showing it at the
/// top.
#[test]
fn an_unknown_parent_is_a_root() {
    let rows = vec![row("A", ""), row("B", "missing")];
    let tree = build_tree(&rows, "parent_task");
    let names: Vec<&str> = tree.iter().map(|node| node.name.as_str()).collect();
    assert_eq!(names, ["A", "B"]);
}

/// A cycle must not remove its members from the page.
#[test]
fn a_cycle_becomes_roots_instead_of_disappearing() {
    let rows = vec![row("A", "B"), row("B", "A"), row("C", "A")];
    let tree = build_tree(&rows, "parent_task");
    assert_eq!(count_nodes(&tree), 3, "every row survives");
    let names: Vec<&str> = tree.iter().map(|node| node.name.as_str()).collect();
    assert!(names.contains(&"A"));
    assert!(names.contains(&"B"));
}

#[test]
fn a_self_parent_is_a_root() {
    let rows = vec![row("A", "A"), row("B", "A")];
    let tree = build_tree(&rows, "parent_task");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, "A");
    assert_eq!(tree[0].children.len(), 1);
}

#[test]
fn the_default_expansion_follows_the_declared_depth() {
    let rows = vec![row("A", ""), row("B", "A"), row("C", "B"), row("D", "")];
    let tree = build_tree(&rows, "parent_task");
    let none = default_expanded(&tree, 0);
    assert!(none.is_empty(), "0 collapses every root");

    let one = default_expanded(&tree, 1);
    assert!(one.contains("A") && !one.contains("B"));

    let all = default_expanded(&tree, 10);
    assert!(all.contains("A") && all.contains("B"));
    assert!(!all.contains("D"), "a leaf has nothing to expand");
}

/// A search keeps the ancestors of every match, so a hit nested three levels
/// deep is still shown inside its branch.
#[test]
fn a_search_keeps_the_path_to_each_match() {
    let rows = vec![row("A", ""), row("B", "A"), row("C", "B"), row("D", "")];
    let tree = filter_tree(build_tree(&rows, "parent_task"), "c", "name");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, "A");
    assert_eq!(tree[0].children[0].name, "B");
    assert_eq!(tree[0].children[0].children[0].name, "C");
}

#[test]
fn a_search_with_no_match_hides_everything() {
    let rows = vec![row("A", ""), row("B", "A")];
    assert!(filter_tree(build_tree(&rows, "parent_task"), "zzz", "name").is_empty());
}

/// The tree paints the label field, so a search must match what the user sees:
/// matching only the document id left a typed subject with an empty page.
#[test]
fn a_search_matches_the_visible_label_too() {
    let rows = vec![
        json!({"name": "TASK001", "subject": "Raíz E2E", "parent_task": ""}),
        json!({"name": "TASK002", "subject": "Hija distinta", "parent_task": "TASK001"}),
    ];
    let tree = filter_tree(build_tree(&rows, "parent_task"), "e2e", "subject");
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, "TASK001");
    assert!(
        tree[0].children.is_empty(),
        "a non-matching child is hidden under a matching parent"
    );

    let by_label = filter_tree(build_tree(&rows, "parent_task"), "hija", "subject");
    assert_eq!(by_label.len(), 1);
    assert_eq!(by_label[0].name, "TASK001");
    assert_eq!(by_label[0].children[0].name, "TASK002");
}
