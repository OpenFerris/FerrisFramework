//! The properties panel's field list: `WidgetParam` → [`FormField`].

use leptos::prelude::*;

use ferris_schema::contract::{WidgetInstance, WidgetParam};

use crate::views::services::FormField;

use super::fieldtype::{options_value, param_fieldtype};

/// Whether a parameter starts a new structural region (tab, section or column).
///
/// When a definition declares one of these, the widget is describing its own
/// form and `group` is irrelevant. Without one, the panel keeps the historical
/// `group`-derived sections so every existing definition renders as before.
#[must_use]
fn declares_layout(param: &WidgetParam) -> bool {
    matches!(
        param_fieldtype(param).as_str(),
        "Tab Break" | "Section Break" | "Column Break"
    )
}

/// Build the editable [`FormField`] for `param`, seeded with the instance's
/// stored value (or the definition's `default`, which is what the panel has
/// always displayed for an unset parameter).
pub(super) fn param_to_field(param: &WidgetParam, instance: &WidgetInstance) -> FormField {
    let fieldtype = param_fieldtype(param);
    let options = options_value(param, &fieldtype);
    let stored = instance
        .params
        .iter()
        .find(|(key, _)| *key == param.name)
        .map(|(_, value)| value.as_str());
    let raw = stored.map_or_else(|| param.default.clone().unwrap_or_default(), str::to_string);
    let is_check = fieldtype == "Check";
    FormField {
        fieldname: param.name.clone(),
        label: param
            .label
            .clone()
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| param.name.clone()),
        fieldtype,
        options,
        reqd: param.required,
        read_only: param.read_only,
        placeholder: param.placeholder.clone(),
        description: param.description.clone(),
        min_value: None,
        max_value: None,
        bold: false,
        max_length: None,
        precision: None,
        fetch_from: None,
        fetch_if_empty: false,
        is_name: false,
        depends_on: param.depends_on.clone(),
        value: RwSignal::new(if is_check { String::new() } else { raw.clone() }),
        checked: RwSignal::new(is_check && raw == "1"),
        error: RwSignal::new(None),
    }
}

/// Build the panel's field list from the definition and the instance.
///
/// Two shapes, in this order:
///
/// 1. **Declared layout** — the definition carries a `Tab Break`, `Section
///    Break` or `Column Break`; the parameters are mapped one-to-one, exactly
///    like a DocType's fields. `group` is ignored, because the widget is
///    already saying where each control goes.
/// 2. **Legacy groups** — no layout marker; a `Section Break` is synthesized
///    whenever `group` changes, preserving the panel every existing
///    definition was written against.
///
/// `hidden` parameters are dropped in both paths; their stored values are left
/// untouched because they never enter the field list.
#[cfg(test)]
#[must_use]
pub(in crate::views::dashboard) fn param_fields(
    defs: &[WidgetParam],
    instance: &WidgetInstance,
) -> Vec<FormField> {
    param_fields_with_section(defs, instance, "Ajustes")
}

/// Like `param_fields`, with the fallback section heading translated for the
/// caller's locale.
#[must_use]
pub(in crate::views::dashboard) fn param_fields_with(
    defs: &[WidgetParam],
    instance: &WidgetInstance,
    i18n: &(impl crate::i18n::Translate + Sync),
) -> Vec<FormField> {
    param_fields_with_section(defs, instance, &i18n.t("views.dashboard.settings"))
}

fn param_fields_with_section(
    defs: &[WidgetParam],
    instance: &WidgetInstance,
    default_section: &str,
) -> Vec<FormField> {
    let visible = || defs.iter().filter(|param| !param.hidden);
    if defs.iter().any(declares_layout) {
        return visible()
            .map(|param| param_to_field(param, instance))
            .collect();
    }
    let mut fields = Vec::new();
    let mut current: Option<String> = None;
    for param in visible() {
        let group = param
            .group
            .clone()
            .filter(|group| !group.is_empty())
            .unwrap_or_else(|| default_section.to_string());
        if current.as_deref() != Some(group.as_str()) {
            fields.push(section_break(&group));
            current = Some(group);
        }
        fields.push(param_to_field(param, instance));
    }
    fields
}

/// The synthetic `Section Break` that gives a legacy `group` its heading.
fn section_break(heading: &str) -> FormField {
    FormField {
        fieldname: String::new(),
        label: heading.to_string(),
        fieldtype: "Section Break".to_string(),
        options: None,
        reqd: false,
        read_only: false,
        placeholder: None,
        description: None,
        min_value: None,
        max_value: None,
        bold: false,
        max_length: None,
        precision: None,
        fetch_from: None,
        fetch_if_empty: false,
        is_name: false,
        depends_on: None,
        value: RwSignal::new(String::new()),
        checked: RwSignal::new(false),
        error: RwSignal::new(None),
    }
}
