//! Reusable cross-cutting pickers.
//!
//! [`DocPicker`] is the single "pick documents of a DocType" control: the Link
//! fields, the multi-select fields, tags and the share dialog all mount it, so
//! the search behaviour (debounce, keyboard, accessible listbox, labels) exists
//! exactly once.

mod doc_picker;
mod file_picker;

pub use doc_picker::{DocPicker, PickerMode};
pub use file_picker::FilePicker;
