//! The hierarchy: rows to nodes, cycles to roots, and the expansion rules.
//!
//! Pure data, kept apart from the component so the tree's guarantees — every
//! row survives, a search keeps the path to each hit, the first open expands a
//! declared number of levels — are testable without a browser.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{self, DynMeta};

/// One node of the hierarchy, carrying its document so a row never looks the
/// row up again.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeNode {
    pub row: Value,
    pub name: String,
    pub parent: String,
    pub children: Vec<Self>,
}

/// The expansion the user left open, persisted per view key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct TreeState {
    #[serde(default)]
    pub(super) expanded: BTreeSet<String>,
    /// Whether the default expansion was applied. Kept in the stored value so a
    /// tree the user collapsed completely stays collapsed after a reload.
    #[serde(default)]
    pub(super) initialized: bool,
}

/// Build the hierarchy from `rows`.
///
/// A row whose parent is empty, unknown or part of a cycle is a **root**: the
/// alternative is losing it from the page, which hides data instead of showing a
/// broken link the reader can fix.
#[must_use]
pub fn build_tree(rows: &[Value], parent_field: &str) -> Vec<TreeNode> {
    let built: Vec<TreeNode> = rows
        .iter()
        .map(|row| {
            let name = api::record_name(row);
            let parent = row
                .get(parent_field)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            TreeNode {
                row: row.clone(),
                name,
                parent,
                children: Vec::new(),
            }
        })
        .collect();

    let by_name: BTreeMap<String, usize> = built
        .iter()
        .enumerate()
        .map(|(index, node)| (node.name.clone(), index))
        .collect();

    // Which rows are inside a cycle. A member of a cycle cannot be attached:
    // every member has an existing parent, so none of them would be a root and
    // the whole ring would vanish from the render.
    let mut in_cycle: HashSet<String> = HashSet::new();
    for node in &built {
        let mut seen: HashSet<String> = HashSet::new();
        let mut current = node;
        loop {
            if !seen.insert(current.name.clone()) {
                // `current` is where the walk met itself: every name in `seen`
                // is part of the ring.
                in_cycle.extend(seen.iter().cloned());
                break;
            }
            let Some(parent_index) = by_name.get(current.parent.as_str()) else {
                break;
            };
            let parent = &built[*parent_index];
            if parent.name == current.name {
                in_cycle.insert(current.name.clone());
                break;
            }
            current = parent;
        }
    }

    let roots_of = |node: &TreeNode| -> bool {
        node.parent.is_empty()
            || !by_name.contains_key(node.parent.as_str())
            || in_cycle.contains(node.name.as_str())
    };

    // Attach children in a second pass. Children are pushed in the source order
    // of `rows` (the request asks for `creation asc`), so two visits agree.
    let mut children: BTreeMap<String, Vec<TreeNode>> = BTreeMap::new();
    let mut roots: Vec<TreeNode> = Vec::new();
    for node in built {
        if roots_of(&node) {
            roots.push(node);
        } else {
            children.entry(node.parent.clone()).or_default().push(node);
        }
    }
    roots
        .into_iter()
        .map(|root| attach(root, &mut children))
        .collect()
}

/// Attach the children collected for `node`, depth first.
fn attach(mut node: TreeNode, children: &mut BTreeMap<String, Vec<TreeNode>>) -> TreeNode {
    if let Some(kids) = children.remove(&node.name) {
        node.children = kids
            .into_iter()
            .map(|child| attach(child, children))
            .collect();
    }
    node
}

/// The rows a search leaves visible: matching nodes and their ancestors.
///
/// A node matches by its **document name or its visible label**: the tree paints
/// the label field (a subject, a code), and searching only the id would leave a
/// user who typed what they can see with an empty page. Searching a tree shows
/// the path to each match, so a hit three levels deep is still located in its
/// branch instead of arriving as an orphan row.
#[must_use]
pub fn filter_tree(nodes: Vec<TreeNode>, needle: &str, label_field: &str) -> Vec<TreeNode> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return nodes;
    }
    nodes
        .into_iter()
        .filter_map(|node| keep(node, &needle, label_field))
        .collect()
}

/// Whether `node` matches `needle` by name or by the label the tree shows.
fn node_matches(node: &TreeNode, needle: &str, label_field: &str) -> bool {
    if node.name.to_lowercase().contains(needle) {
        return true;
    }
    node.row
        .get(label_field)
        .and_then(Value::as_str)
        .is_some_and(|label| label.to_lowercase().contains(needle))
}

/// Keep `node` when it matches, or when a descendant does.
fn keep(node: TreeNode, needle: &str, label_field: &str) -> Option<TreeNode> {
    let matches = node_matches(&node, needle, label_field);
    let kept: Vec<TreeNode> = node
        .children
        .into_iter()
        .filter_map(|child| keep(child, needle, label_field))
        .collect();
    (matches || !kept.is_empty()).then_some(TreeNode {
        children: kept,
        ..node
    })
}

/// Every node name below the roots, in first-paint expansion order.
///
/// `levels` is the count of levels expanded: `0` leaves every root collapsed,
/// `1` opens the roots, `2` their children, and so on.
#[must_use]
pub fn default_expanded(roots: &[TreeNode], levels: u8) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for root in roots {
        walk(root, 0, levels, &mut out);
    }
    out
}

/// Insert into `out` every node below `node` that the depth still opens.
fn walk(node: &TreeNode, depth: u8, levels: u8, out: &mut BTreeSet<String>) {
    if depth >= levels || node.children.is_empty() {
        return;
    }
    out.insert(node.name.clone());
    for child in &node.children {
        walk(child, depth.saturating_add(1), levels, out);
    }
}

/// Counts nodes, for the "N documentos" summary.
#[must_use]
pub fn count_nodes(nodes: &[TreeNode]) -> usize {
    nodes
        .iter()
        .map(|node| 1 + count_nodes(&node.children))
        .sum()
}

/// The conventional tree parent: a `Link` field named `parent` pointing at the
/// same doctype.
pub fn conventional_parent(meta: &DynMeta) -> Option<String> {
    meta.fields
        .iter()
        .find(|field| {
            field.fieldname == "parent"
                && field.fieldtype == "Link"
                && field.options.as_deref() == Some(meta.name.as_str())
        })
        .map(|field| field.fieldname.clone())
}
