//! Session-scoped layout history: the undo/redo stacks outlive one form mount.
//!
//! The stacks live on [`FormService`], which is built by the form view, and the
//! router remounts that view when the matched route changes (for example when a
//! metadata reload navigates, or when the user re-enters `/layout`). Keeping the
//! stacks here, keyed by `DocType`, makes "undo this session's layout edits"
//! survive those remounts without keeping the whole form service alive.
//!
//! The scope is the **layout session**, not the `DocType`: leaving the editor
//! ([`clear`]) ends it, because the document view has no layout on screen and an
//! undo there would revert an edit the user cannot see (see
//! `FormService`'s session effect). The store exists only so a remount *inside*
//! the editor does not lose the session.
//!
//! The store is memory only: a reload starts empty, which is the honest scope
//! of "this session". On the server it is inert (no layout edit originates
//! there) but harmless; it is a `thread_local` so a request thread never sees
//! another's state.

use std::cell::RefCell;
use std::collections::HashMap;

use super::layout_ops::LayoutOp;

/// The two stacks of one `DocType`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LayoutHistory {
    pub undo: Vec<LayoutOp>,
    pub redo: Vec<LayoutOp>,
}

thread_local! {
    static HISTORY: RefCell<HashMap<String, LayoutHistory>> = RefCell::new(HashMap::new());
}

/// The recorded operations of `doctype`, or the empty history.
pub(super) fn load(doctype: &str) -> LayoutHistory {
    HISTORY.with(|history| history.borrow().get(doctype).cloned().unwrap_or_default())
}

/// Persist the current stacks of `doctype`.
pub(super) fn store(doctype: &str, history: LayoutHistory) {
    HISTORY.with(|store| {
        store.borrow_mut().insert(doctype.to_string(), history);
    });
}

/// Drop the session of `doctype`: the layout editor was left, so its undo
/// history has no surface left to act on.
///
/// Removing the entry instead of storing an empty one keeps the map from
/// growing with every doctype the user ever opened the editor for.
pub(super) fn clear(doctype: &str) {
    HISTORY.with(|store| {
        store.borrow_mut().remove(doctype);
    });
}

#[cfg(test)]
mod tests {
    use super::{LayoutHistory, clear, load, store};
    use crate::views::services::form_service::LayoutOp;

    fn op(fieldname: &str) -> LayoutOp {
        LayoutOp::PropertySet {
            fieldname: Some(fieldname.to_string()),
            property: "insert_after".to_string(),
            previous: None,
            value: "title".to_string(),
        }
    }

    #[test]
    fn a_history_is_stored_per_doctype() {
        store(
            "Stored A",
            LayoutHistory {
                undo: vec![op("a")],
                redo: Vec::new(),
            },
        );
        store(
            "Stored B",
            LayoutHistory {
                undo: Vec::new(),
                redo: vec![op("b")],
            },
        );
        assert_eq!(load("Stored A").undo, vec![op("a")]);
        assert!(load("Stored A").redo.is_empty());
        assert!(load("Stored B").undo.is_empty());
        assert_eq!(load("Stored B").redo, vec![op("b")]);
        assert_eq!(load("Never touched"), LayoutHistory::default());
    }

    /// Leaving the layout editor ends its session: the next mount starts empty
    /// instead of inheriting stacks the document view must never apply.
    #[test]
    fn clearing_a_session_leaves_the_other_doctypes_alone() {
        store(
            "Session A",
            LayoutHistory {
                undo: vec![op("a")],
                redo: vec![op("b")],
            },
        );
        store(
            "Session B",
            LayoutHistory {
                undo: vec![op("c")],
                redo: Vec::new(),
            },
        );
        clear("Session A");
        assert_eq!(load("Session A"), LayoutHistory::default());
        assert_eq!(load("Session B").undo, vec![op("c")]);
    }
}
