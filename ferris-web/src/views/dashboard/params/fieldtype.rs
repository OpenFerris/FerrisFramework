//! The `WidgetParam.value_type` → form fieldtype mapping.

use ferris_schema::contract::WidgetParam;

/// Fieldtypes a widget parameter may be edited with.
///
/// This is the form view's own editable catalog
/// ([`crate::components::views::form::customize::FIELD_TYPES`]) minus the one
/// entry that is **document-bound** (`Attachment Gallery`: its files live in
/// the engagement store keyed by the document, so in a dialog with no document
/// it renders a dead control), plus `Table MultiSelect`, which the customize
/// dialog leaves out but a widget needs for multi-value parameters.
///
/// Every entry renders without document context (no `FormDocContext`), carries
/// a value of its own and round-trips through `WidgetInstance::params` as a
/// string. Kept as the render-safety allowlist for the open `value_type`
/// vocabulary — deliberately *not* the schema allowlist (`ferris-schema`):
/// that one answers "which fieldtypes exist on the wire", this one answers
/// "which controls make sense in a widget's settings panel". The
/// `params_offer_every_editable_form_fieldtype` test pins the parity, so a new
/// form type cannot silently stop being available to widgets.
///
/// `Link` and `Table MultiSelect` are the two that give a parameter an
/// **autocomplete**: `Link` searches the target doctype's documents for a
/// single value, `Table MultiSelect` does the same but collects many into
/// chips. An app declares the target as the param's `options[0]` (a doctype
/// name) instead of asking the operator to type a comma-separated list, which
/// is error-prone and cannot be validated. Both controls condition their value
/// as the document engine expects it (a bare name, or a JSON array of names),
/// so a widget reads back the same shape whether the param came from a
/// document field or a widget instance.
///
/// `Dynamic Link` is included for parity but degrades to a plain text input:
/// its target is another *field* of the same document, and a widget parameter
/// has no such sibling, so there is nothing to search. It never fails to
/// render — it simply behaves like `Data`.
pub(super) const PARAM_FIELDTYPES: &[&str] = &[
    "Data",
    "Text",
    "Small Text",
    "Long Text",
    "Text Editor",
    "Markdown Editor",
    "Code",
    "Link",
    "Dynamic Link",
    "Select",
    "Int",
    "Float",
    "Currency",
    "Percent",
    "Rating",
    "Duration",
    "Check",
    "Date",
    "Datetime",
    "Time",
    "Week",
    "Password",
    "Color",
    "Email",
    "Phone",
    "URL",
    "JSON",
    "Geolocation",
    "Signature",
    "Barcode",
    "Image",
    "File",
    "Attach",
    "Attach Image",
    "Table MultiSelect",
    // Layout and display-only fieldtypes: a widget declares the *shape* of its
    // settings panel with the same markers a DocType uses. `Tab Break` starts
    // a tab (`label` names it), `Section Break` a titled block, `Column Break`
    // a new column in the current block, `Heading` a caption, `HTML` a trusted
    // block and `Read Only` a value shown without an editor. They carry no
    // storable value, so [`super::sync::sync_params`] skips them.
    "Tab Break",
    "Section Break",
    "Column Break",
    "Heading",
    "HTML",
    "Read Only",
];

/// The form fieldtype that edits a widget parameter.
///
/// A canonical fieldtype the panel can draw wins as declared; then a
/// non-empty `options` means "select" (the pre-WIT-9 contract of
/// `WidgetParam`); then the legacy kind table; the rest is `Data`.
pub(super) fn param_fieldtype(param: &WidgetParam) -> String {
    if let Some(ft) = ferris_schema::fieldtype::canonical(&param.value_type)
        && PARAM_FIELDTYPES.contains(&ft)
    {
        return ft.to_string();
    }
    if !param.options.is_empty() {
        return "Select".to_string();
    }
    match param.value_type.as_str() {
        "bool" => "Check",
        "number" => "Float",
        "doctype" => "Link",
        "json" => "JSON",
        // `string` and any vocabulary the Desk does not know: a plain
        // single-line input, which is what the panel showed before the
        // form-field reuse and never fails to render.
        _ => "Data",
    }
    .to_string()
}

/// The `options` string the control needs: newline-joined for `Select` (the
/// same shape `DynField::options` has), the target doctype for a `Link` (or
/// `Table MultiSelect`): a legacy `"doctype"` param always links `DocType`,
/// while an explicit `Link`/`Table MultiSelect` declares its target as the
/// first option. `Currency` and `Barcode` take their first option as the
/// prefix/symbology the renderer already reads from `options` (an empty value
/// falls back to the control's default, `$` and `CODE128`).
pub(super) fn options_value(param: &WidgetParam, fieldtype: &str) -> Option<String> {
    match fieldtype {
        "Select" => {
            if param.options.is_empty() {
                None
            } else {
                Some(param.options.join("\n"))
            }
        }
        "Link" if param.value_type == "doctype" => Some("DocType".to_string()),
        "Link" | "Table MultiSelect" | "Currency" | "Barcode" => param.options.first().cloned(),
        // The form renderer draws an `HTML` field from its `options` string.
        "HTML" => param.options.first().cloned(),
        _ => None,
    }
}

/// Fieldtypes that occupy a place in the layout but hold no value of their own.
///
/// The list mirrors the skip sets in `validate_form`/`build_form_payload` plus
/// `Read Only` (a mirror of a stored value, never a source of one). A parameter
/// with one of these types is never written back to the instance.
#[must_use]
pub(super) fn carries_value(fieldtype: &str) -> bool {
    !matches!(
        fieldtype,
        "Tab Break"
            | "Section Break"
            | "Column Break"
            | "Heading"
            | "HTML"
            | "Read Only"
            | "Table"
            | "Button"
            | "Attachment Gallery"
    )
}
