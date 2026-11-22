//! The board's grouping model: which field groups, which columns it yields,
//! and the one write a move performs.
//!
//! Kept apart from the component so the rules are testable without a browser:
//! the renderer owns layout, this module owns the decisions.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;

use crate::api::{self, DocStore, DynMeta};

/// The column of a document whose grouping value is empty.
pub const EMPTY_COLUMN: &str = "—";

/// How the board groups and whether a card may be moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpec {
    pub field: String,
    /// Declared columns, empty for a `Link` (they come from the data).
    pub options: Vec<String>,
    /// `docstatus` is a lifecycle column: read-only grouping.
    pub movable: bool,
}

impl GroupSpec {
    /// The board a doctype's metadata describes.
    #[must_use]
    pub fn from_meta(meta: &DynMeta) -> Self {
        if let Some(name) = meta
            .kanban_group_field
            .as_deref()
            .filter(|name| !name.is_empty())
            && let Some(field) = meta.fields.iter().find(|field| field.fieldname == name)
        {
            let options = if field.fieldtype == "Select" {
                field
                    .options
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .map(str::trim)
                    .filter(|option| !option.is_empty())
                    .map(str::to_string)
                    .collect()
            } else {
                Vec::new()
            };
            return Self {
                field: name.to_string(),
                options,
                movable: matches!(field.fieldtype.as_str(), "Select" | "Link"),
            };
        }
        if let Some(field) = meta.fields.iter().find(|field| {
            field.fieldtype == "Select"
                && field.options.as_deref().unwrap_or_default().contains('\n')
        }) {
            return Self {
                field: field.fieldname.clone(),
                options: field
                    .options
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .map(str::trim)
                    .filter(|option| !option.is_empty())
                    .map(str::to_string)
                    .collect(),
                movable: true,
            };
        }
        Self {
            field: "docstatus".to_string(),
            options: vec!["0".to_string(), "1".to_string(), "2".to_string()],
            movable: false,
        }
    }
}

/// Column key of one document: its value, or [`EMPTY_COLUMN`].
#[must_use]
pub fn column_key(row: &Value, field: &str) -> String {
    row.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map_or_else(|| EMPTY_COLUMN.to_string(), str::to_string)
}

/// The columns to draw, in order, and their cards.
///
/// Declared `Select` options come first, in their declared order; a value the
/// document carries but the declaration does not list still gets a column (data
/// must never be invisible). `Link` boards derive the order from the values;
/// the empty column goes last.
#[must_use]
pub fn columns<S: std::hash::BuildHasher>(
    rows: &[Value],
    group: &GroupSpec,
    moved: &HashMap<String, String, S>,
) -> Vec<(String, Vec<Value>)> {
    let mut buckets: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for row in rows {
        let name = api::record_name(row);
        let key = moved
            .get(&name)
            .cloned()
            .unwrap_or_else(|| column_key(row, &group.field));
        buckets.entry(key).or_default().push(row.clone());
    }
    let mut ordered: Vec<(String, Vec<Value>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if group.movable && !group.options.is_empty() {
        for option in &group.options {
            seen.push(option.clone());
            ordered.push((option.clone(), buckets.remove(option).unwrap_or_default()));
        }
        for (key, cards) in buckets {
            if !seen.contains(&key) {
                ordered.push((key, cards));
            }
        }
    } else {
        let empty = buckets.remove(EMPTY_COLUMN);
        for (key, cards) in buckets {
            ordered.push((key, cards));
        }
        if let Some(cards) = empty {
            ordered.push((EMPTY_COLUMN.to_string(), cards));
        }
    }
    ordered
}

/// The neighbouring column, in the declared order, that a card can move to.
#[must_use]
pub fn neighbour(current: &str, labels: &[String], forward: bool) -> Option<String> {
    let index = labels.iter().position(|label| label == current)?;
    if forward {
        labels.get(index + 1).cloned()
    } else {
        index
            .checked_sub(1)
            .map(|previous| labels[previous].clone())
    }
}

pub fn default_group() -> GroupSpec {
    GroupSpec {
        field: "docstatus".to_string(),
        options: vec!["0".to_string(), "1".to_string(), "2".to_string()],
        movable: false,
    }
}

/// Write the grouping field and report the outcome.
///
/// # Errors
///
/// The runtime/store message, or the API's own error text. Nothing is swallowed:
/// the caller reverts the optimistic move and displays this.
pub async fn write_group(
    doctype: &str,
    field: &str,
    name: &str,
    target: &str,
) -> Result<(), String> {
    let store = DocStore::runtime()?;
    let value = if target == EMPTY_COLUMN {
        Value::String(String::new())
    } else {
        Value::String(target.to_string())
    };
    let mut patch = serde_json::Map::new();
    patch.insert(field.to_string(), value);
    store
        .update(doctype, name, Value::Object(patch))
        .await
        .map(|_| ())
}
