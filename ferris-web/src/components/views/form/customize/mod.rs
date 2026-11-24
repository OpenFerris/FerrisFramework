//! The fieldtype catalogue shared by the form and the dashboard parameter panel.
//!
//! This module used to hold the in-place "Personalizar" dialog. The `/layout`
//! editor replaced it (with a diff, undo/redo and validation), so only the
//! catalogue remains. `FIELD_TYPES`, `hint` and `nonempty` stay re-exported here
//! so every existing `customize::…` path keeps resolving.

pub mod types;

pub use types::{FIELD_TYPES, hint, nonempty};
