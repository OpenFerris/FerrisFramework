//! Writing the edited fields back into the instance's parameter pairs.

use leptos::prelude::*;

use ferris_schema::contract::{WidgetInstance, WidgetParam};

use crate::views::services::FormField;

use super::fieldtype::carries_value;

/// Write the edited fields back into the instance's `(name, value)` pairs,
/// following the materialization rules of the module docs:
///
/// - `Check`: checked → `"1"`; unchecked → `"0"` only when the definition's
///   default is `"1"` (otherwise the key would resurrect the default), else
///   the key is removed;
/// - any other type: empty → remove the key (empty means "use the default");
///   a value equal to the default with no stored key → write nothing; any
///   other value → upsert.
pub(in crate::views::dashboard) fn sync_params(
    widget: &mut WidgetInstance,
    fields: &[FormField],
    defs: &[WidgetParam],
) {
    for f in fields {
        // Layout markers, headings, HTML and read-only mirrors are not values:
        // writing them would create a `("", "")`-style key or freeze a display
        // default into the instance.
        if !carries_value(&f.fieldtype) {
            continue;
        }
        let default = defs
            .iter()
            .find(|param| param.name == f.fieldname)
            .and_then(|param| param.default.as_deref());
        let had_key = widget.params.iter().any(|(key, _)| *key == f.fieldname);
        let desired: Option<String> = if f.fieldtype == "Check" {
            if f.checked.get() {
                Some("1".to_string())
            } else if default == Some("1") {
                Some("0".to_string())
            } else {
                None
            }
        } else {
            let raw = f.value.get();
            if raw.is_empty() {
                None
            } else if !had_key && default == Some(raw.as_str()) {
                // Untouched: the definition's default already applies
                // implicitly — do not freeze it into the instance.
                continue;
            } else {
                Some(raw)
            }
        };
        match (
            widget
                .params
                .iter_mut()
                .find(|(key, _)| *key == f.fieldname),
            desired,
        ) {
            (Some((_, slot)), Some(value)) => *slot = value,
            (Some(_), None) => {
                widget.params.retain(|(key, _)| *key != f.fieldname);
            }
            (None, Some(value)) => widget.params.push((f.fieldname.clone(), value)),
            (None, None) => {}
        }
    }
}
