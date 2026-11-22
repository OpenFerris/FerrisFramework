//! Pure layout/validation logic for the form view.
//!
//! Split out of `form_service` so each file stays a single concern: this one
//! holds the field model and the functions that turn metadata + a document into
//! a form layout and back into a payload, with no signals, no I/O and no
//! `RwSignal` state. `form_service::mod` owns the reactive service that drives
//! them.

use leptos::prelude::*;
use serde_json::Value;

use super::condition;
use crate::api::DynMeta;
use crate::api::NamingType;
use crate::api::meta::{docstatus_rank, form_fields, label_of};

/// Form field with reactive state.
#[derive(Clone)]
pub struct FormField {
    pub fieldname: String,
    pub label: String,
    pub fieldtype: String,
    pub options: Option<String>,
    pub reqd: bool,
    pub read_only: bool,
    pub placeholder: Option<String>,
    pub description: Option<String>,
    pub min_value: Option<String>,
    pub max_value: Option<String>,
    /// Render the label in bold (`FieldMeta::bold`).
    pub bold: bool,
    /// Hard cap on the text length, forwarded as the input's `maxlength`.
    pub max_length: Option<u32>,
    /// Decimal places for numeric fields, used to derive the input step.
    pub precision: Option<u8>,
    /// `"source_field.target_field"`: when the `Link` in `source_field`
    /// changes, copy `target_field` of the linked document into this field.
    pub fetch_from: Option<String>,
    /// Apply [`Self::fetch_from`] only while this field is empty.
    pub fetch_if_empty: bool,
    /// Whether this is the synthetic document-id field the form injects in
    /// `new`, rather than a declared field. Drives the payload rule that omits
    /// an untouched naming-series template so the engine mints the name.
    pub is_name: bool,
    /// Expression that decides whether the field is shown; `None` (or an
    /// empty string) always shows it. Evaluated by [`condition`], the same
    /// rule the renderer and validation use.
    pub depends_on: Option<String>,
    pub value: RwSignal<String>,
    pub checked: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
}

/// A rendered section: one grid of fields, optionally split by columns.
#[derive(Clone, PartialEq)]
pub struct FormSection {
    pub tab: String,
    pub heading: String,
    pub cols: u32,
    pub items: Vec<FormSectionItem>,
}

#[derive(Clone, PartialEq)]
pub enum FormSectionItem {
    Field(usize),
    Table(usize),
}

/// Turn the flat form field list into tabs -> sections -> columns.
pub fn build_form_layout(fields: &[FormField]) -> (Vec<String>, Vec<FormSection>) {
    let mut tab_order: Vec<String> = Vec::new();
    let mut sections: Vec<FormSection> = Vec::new();
    let mut cur_tab = String::new();
    let mut section_idx: Option<usize> = None;
    let mut col_count: u32 = 1;

    macro_rules! ensure_section {
        () => {{
            if section_idx.is_none() {
                sections.push(FormSection {
                    tab: cur_tab.clone(),
                    heading: String::new(),
                    cols: 1,
                    items: Vec::new(),
                });
                section_idx = Some(sections.len() - 1);
                col_count = 1;
            }
        }};
    }

    let close_section =
        |sections: &mut Vec<FormSection>, section_idx: &mut Option<usize>, col_count: &mut u32| {
            if let Some(k) = *section_idx {
                sections[k].cols = (*col_count).max(1);
            }
        };

    for (i, f) in fields.iter().enumerate() {
        let ft = f.fieldtype.as_str();
        match ft {
            "Tab Break" => {
                close_section(&mut sections, &mut section_idx, &mut col_count);
                let label = f.fieldname.trim().to_string();
                if !label.is_empty() {
                    // Fields that came before the first `Tab Break` belong to
                    // it: in `new` mode the form leads with the synthetic `ID`
                    // field, and giving that one a tab of its own produced a
                    // duplicate "General" tab whose hidden panel held every
                    // declared field — the form showed only the ID.
                    if tab_order.is_empty() {
                        for section in &mut sections {
                            if section.tab.is_empty() {
                                section.tab.clone_from(&label);
                            }
                        }
                    }
                    if !tab_order.contains(&label) {
                        tab_order.push(label.clone());
                    }
                    cur_tab = label;
                }
                section_idx = None;
            }
            "Section Break" => {
                close_section(&mut sections, &mut section_idx, &mut col_count);
                sections.push(FormSection {
                    tab: cur_tab.clone(),
                    heading: f.label.clone(),
                    cols: 1,
                    items: Vec::new(),
                });
                section_idx = Some(sections.len() - 1);
                col_count = 1;
            }
            "Column Break" => {
                col_count += 1;
            }
            "Table" => {
                ensure_section!();
                if let Some(index) = section_idx {
                    sections[index].items.push(FormSectionItem::Table(i));
                }
            }
            // `HTML` is content, not a layout marker: it flows into the section
            // as a regular field so the renderer can draw the block from its
            // `options`. It carries no value, so the payload/validate skip sets
            // still ignore it.
            _ => {
                ensure_section!();
                if let Some(index) = section_idx {
                    sections[index].items.push(FormSectionItem::Field(i));
                }
            }
        }
    }
    if let Some(idx) = section_idx {
        sections[idx].cols = col_count.max(1);
    }
    if tab_order.is_empty() {
        tab_order.push(String::new());
    }
    (tab_order, sections)
}

/// The three shapes a field list renders as: the tabs to paint (value + human
/// label, empty value = the default "General" tab), the tab selected first, and
/// the sections themselves.
///
/// This is the single derivation used by the document form and by a widget's
/// parameter panel. A tab with no fields is dropped, because an empty tab is
/// a dead end for the user; a `Tab Break`'s `label` names the tab when it has
/// one, and its `fieldname` is the fallback (that is how DocTypes have always
/// declared tabs).
#[must_use]
pub fn form_layout(fields: &[FormField]) -> (Vec<(String, String)>, String, Vec<FormSection>) {
    let (tabs, sections) = build_form_layout(fields);
    let tab_labels: Vec<(String, String)> = tabs
        .into_iter()
        .filter(|tab| {
            sections
                .iter()
                .any(|section| &section.tab == tab && !section.items.is_empty())
        })
        .map(|tab| {
            let declared = fields
                .iter()
                .find(|field| field.fieldtype == "Tab Break" && field.fieldname.trim() == tab)
                .map(|field| field.label.trim())
                .filter(|label| !label.is_empty())
                .map(str::to_string);
            let label = declared.unwrap_or_else(|| {
                if tab.is_empty() {
                    "General".to_string()
                } else {
                    tab.clone()
                }
            });
            (tab, label)
        })
        .collect();
    let default_tab = tab_labels
        .first()
        .map(|(value, _)| value.clone())
        .unwrap_or_default();
    (tab_labels, default_tab, sections)
}

/// Build the initial form field signals from metadata and document data.
///
/// In `new` mode the form leads with a synthetic `ID` field, because a
/// document that does not exist yet has no name to show — yet the naming
/// strategy is exactly what the user needs to see. The field is prefilled from
/// the doctype's [`NamingType`] (the series template for `Series`, empty and
/// read-only for the engine-owned strategies, empty and required for
/// `Prompt`); [`build_form_payload`] then drops an untouched template so the
/// engine still mints the real name.
pub fn build_form_fields(meta: &DynMeta, doc: &Value, is_new: bool) -> Vec<FormField> {
    let mut fields_out = Vec::new();
    if is_new && !meta.is_single {
        fields_out.push(synthetic_name_field(meta));
    }
    // Frappe's post-submit lock (`allow_on_submit`) and the Virtual DocField
    // read-only rule are both decided here, where the loaded document is in
    // hand. A cancelled document is fully locked; a submitted one keeps only
    // its `allow_on_submit` fields editable; a virtual field is never editable.
    let docstatus = doc.get("docstatus").map(docstatus_rank).unwrap_or(0);
    for fm in form_fields(meta) {
        let raw = doc.get(&fm.fieldname).unwrap_or(&Value::Null);
        // Canonicalize once, here: every match below — the value
        // coercion, the layout branches and the renderer — is keyed on the
        // wire spelling, so a field persisted with a legacy camelCase alias
        // (`SmallText`) still routes to its dedicated component.
        let fieldtype = legacy_alias(&fm.fieldtype)
            .unwrap_or(fm.fieldtype.as_str())
            .to_string();
        let (text, checked) = match (fieldtype.as_str(), raw) {
            // A new document has no value for the flag, so the declared
            // default applies — the same rule every other fieldtype follows
            // through the `_` arm below. Without this, a Check declared
            // `default = "1"` opened unchecked and sent `false` on the first
            // save, silently flipping a flag the metadata had turned on.
            ("Check", Value::Null) => (
                String::new(),
                matches!(fm.default.as_deref(), Some("1" | "true")),
            ),
            ("Check", _) => (
                String::new(),
                raw.as_bool().unwrap_or(false) || raw.as_i64() == Some(1),
            ),
            // A `JSON` field stores an object/array and the widget edits text:
            // the document has to come back as formatted JSON or the textarea
            // reopened empty on every load.
            ("JSON", value @ (Value::Object(_) | Value::Array(_))) => (
                serde_json::to_string_pretty(value).unwrap_or_default(),
                false,
            ),
            // An `Int` never shows a fractional part, even when an older save
            // stored it as `12.0`.
            ("Int", Value::Number(n)) => (
                n.as_i64()
                    .map_or_else(|| n.to_string(), |value| value.to_string()),
                false,
            ),
            // Money and percentages read as an amount with the declared
            // precision: storing `10` and showing `10.0` in a field configured
            // for two decimals is the kind of detail that reads as a bug.
            ("Float" | "Currency" | "Percent", Value::Number(n)) => {
                let text = match (fm.precision, n.as_f64()) {
                    (Some(precision), Some(number)) => {
                        format!("{number:.precision$}", precision = precision as usize)
                    }
                    _ => n.to_string(),
                };
                (text, false)
            }
            (_, Value::Bool(b)) => (b.to_string(), false),
            (_, Value::String(s)) => (s.clone(), false),
            (_, Value::Number(n)) => (n.to_string(), false),
            // A child table keeps its rows in the same `value` signal as the
            // other fields, serialized as the JSON array the engine stores. The
            // child-table editor parses and rewrites it; keeping one shape here
            // is what lets save/duplicate/uniqueness all see real rows.
            ("Table", Value::Array(rows)) => (
                serde_json::to_string(rows).unwrap_or_else(|_| "[]".to_string()),
                false,
            ),
            // A stored value that is not an array is preserved verbatim so the
            // editor can report it and the payload omits it. Turning it into
            // `[]` here would make "open a document, press Save" erase rows the
            // user never touched.
            ("Table", Value::Null) => ("[]".to_string(), false),
            ("Table", other) => (other.to_string(), false),
            _ => (
                fm.default.clone().unwrap_or_default(),
                fm.default.as_deref() == Some("1"),
            ),
        };
        fields_out.push(FormField {
            fieldname: fm.fieldname.clone(),
            label: match fieldtype.as_str() {
                "Tab Break" => fm.label.clone().unwrap_or_default(),
                _ => label_of(fm),
            },
            fieldtype,
            options: fm.options.clone(),
            reqd: fm.reqd,
            read_only: fm.read_only
                || fm.r#virtual
                || docstatus == 2
                || (docstatus == 1 && !fm.allow_on_submit),
            placeholder: fm.placeholder.clone(),
            description: fm.description.clone(),
            min_value: fm.min_value.clone(),
            max_value: fm.max_value.clone(),
            bold: fm.bold,
            max_length: fm.max_length,
            precision: fm.precision,
            fetch_from: fm.fetch_from.clone(),
            fetch_if_empty: fm.fetch_if_empty,
            is_name: false,
            depends_on: fm.depends_on.clone(),
            value: RwSignal::new(text),
            checked: RwSignal::new(checked),
            error: RwSignal::new(None),
        });
    }
    fields_out
}

/// The synthetic `ID` field a `new` form leads with.
///
/// Its shape is the doctype's naming declaration: a `Series` is editable and
/// prefilled with the template, `Prompt` is editable and required, and
/// `AutoIncrement`/`Uuid`/`Field` are shown read-only because the engine (or
/// another field) owns them. `Field` names the source field so the user knows
/// where the id comes from.
fn synthetic_name_field(meta: &DynMeta) -> FormField {
    let (value, read_only, reqd, placeholder, description) = match meta.naming() {
        NamingType::Series => (
            meta.naming_series.clone(),
            false,
            false,
            String::new(),
            Some("Déjalo como está para usar la serie configurada.".to_string()),
        ),
        NamingType::AutoIncrement => (
            String::new(),
            true,
            false,
            "Se generará automáticamente".to_string(),
            Some("Contador automático.".to_string()),
        ),
        NamingType::Uuid => (
            String::new(),
            true,
            false,
            "Se generará automáticamente".to_string(),
            Some("Identificador único automático.".to_string()),
        ),
        NamingType::Field => (
            String::new(),
            true,
            false,
            meta.naming_field
                .as_deref()
                .map_or_else(String::new, |f| format!("Se tomará de «{f}»")),
            Some("El identificador se toma del campo indicado.".to_string()),
        ),
        NamingType::Prompt => (
            String::new(),
            false,
            true,
            "Escribe el identificador".to_string(),
            Some("El identificador es obligatorio.".to_string()),
        ),
        // These are all generated by the engine from metadata or from another
        // field, so the id is shown read-only rather than letting the user type
        // a value the engine will overwrite.
        NamingType::NamingSeries => (
            String::new(),
            true,
            false,
            "Se generará automáticamente".to_string(),
            Some("Se formatea la serie elegida en el documento.".to_string()),
        ),
        NamingType::Expression => (
            String::new(),
            true,
            false,
            "Se generará automáticamente".to_string(),
            Some("Expresión configurada en el DocType.".to_string()),
        ),
        NamingType::ExpressionOldStyle => (
            String::new(),
            true,
            false,
            "Se generará automáticamente".to_string(),
            Some("Expresión configurada en el DocType.".to_string()),
        ),
        NamingType::Random => (
            String::new(),
            true,
            false,
            "Se generará automáticamente".to_string(),
            Some("Nombre aleatorio.".to_string()),
        ),
        NamingType::Script => (
            String::new(),
            true,
            false,
            "Se generará automáticamente".to_string(),
            Some("Generado por el proveedor de nombres.".to_string()),
        ),
    };
    FormField {
        fieldname: "name".to_string(),
        label: "ID".to_string(),
        fieldtype: "Data".to_string(),
        options: None,
        reqd,
        read_only,
        placeholder: (!placeholder.is_empty()).then_some(placeholder),
        description,
        min_value: None,
        max_value: None,
        bold: true,
        max_length: None,
        precision: None,
        fetch_from: None,
        fetch_if_empty: false,
        is_name: true,
        depends_on: None,
        value: RwSignal::new(value),
        checked: RwSignal::new(false),
        error: RwSignal::new(None),
    }
}

/// Validate all form fields. Returns true if valid.
pub fn validate_form(fields: &[FormField]) -> bool {
    let mut ok = true;
    for f in fields {
        f.error.set(None);
        // A field whose `depends_on` is false is not on the form, so it
        // cannot be a required field that blocks the save.
        if !condition::is_visible(fields, f) {
            continue;
        }
        let ft = f.fieldtype.as_str();
        // A child table's value is the serialized array, which is never empty
        // as a string; required-ness is about the rows, not the JSON text.
        if ft == "Table" {
            if f.reqd {
                let empty = serde_json::from_str::<Vec<serde_json::Value>>(&f.value.get())
                    .map_or(true, |rows| rows.is_empty());
                if empty {
                    f.error.set(Some("Añade al menos una fila".to_string()));
                    ok = false;
                }
            }
            continue;
        }
        if matches!(
            ft,
            "Tab Break"
                | "Section Break"
                | "Column Break"
                | "HTML"
                | "Table"
                | "Button"
                | "Attachment Gallery"
                | "Heading"
                // A `Widget` field hosts a widget; it has no value of its own,
                // so a required one must not block a save.
                | "Widget"
        ) {
            continue;
        }
        if f.read_only || ft == "Check" {
            continue;
        }
        let v = f.value.get();
        let trimmed = v.trim().to_string();
        if f.reqd && trimmed.is_empty() {
            f.error.set(Some("Campo obligatorio".to_string()));
            ok = false;
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        match ft {
            "Int" | "Float" | "Currency" | "Percent" => {
                if let Some(message) = number_error(f, &trimmed) {
                    f.error.set(Some(message));
                    ok = false;
                }
            }
            "Email" => {
                if !trimmed.contains('@') {
                    f.error.set(Some("Email inválido".to_string()));
                    ok = false;
                }
            }
            "Phone" => {
                let digits: usize = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
                if digits < 10 {
                    f.error.set(Some("Teléfono inválido".to_string()));
                    ok = false;
                }
            }
            "Table MultiSelect" => {
                // The widget writes a JSON array of names; a bare name is a
                // tolerated legacy value, but anything that *looks* like an
                // array and fails to parse (`["a",1]`) is rejected here rather
                // than silently sent as one garbage element.
                let raw = trimmed.as_str();
                if raw.starts_with('[') && serde_json::from_str::<Vec<String>>(raw).is_err() {
                    f.error.set(Some(
                        "Lista inválida: se esperan nombres entrecomillados".to_string(),
                    ));
                    ok = false;
                }
            }
            // The widget edits text; the wire value is JSON. Rejecting an
            // invalid document here is the difference between "the server said
            // no after a round-trip" and a field-level message before saving.
            "JSON" if !is_json_document(&trimmed) => {
                f.error.set(Some(
                    "JSON inválido: se espera un objeto o una lista".to_string(),
                ));
                ok = false;
            }
            _ => {}
        }
    }
    ok
}

/// The reason a numeric field's text is not valid, or `None`.
///
/// Split out of [`validate_form`] to keep that function inside the repository's
/// length budget; the rules (parse, declared bounds, `Int` integrality) are the
/// same.
fn number_error(f: &FormField, trimmed: &str) -> Option<String> {
    // The widget accepts the locale's comma; the engine wants a number.
    let Ok(n) = trimmed.replace(',', ".").parse::<f64>() else {
        return Some("Número inválido".to_string());
    };
    if let Some(min) = f.min_value.as_deref().and_then(|m| m.parse::<f64>().ok())
        && n < min
    {
        return Some(format!("Valor mínimo {min}"));
    }
    if let Some(max) = f.max_value.as_deref().and_then(|m| m.parse::<f64>().ok())
        && n > max
    {
        return Some(format!("Valor máximo {max}"));
    }
    if f.fieldtype == "Int" && n.fract() != 0.0 {
        return Some("Debe ser un entero".to_string());
    }
    None
}

/// The canonical spelling of `name`, accepting the camelCase aliases older
/// metadata carries (`SmallText` → `Small Text`).
///
/// The strict vocabulary in [`ferris_schema::fieldtype`] is the write gate: a
/// new field with an unknown type is refused there. This is the **read** side,
/// where a table written before the strictness still has rows the Desk must
/// render instead of degrading to plain inputs.
fn legacy_alias(name: &str) -> Option<&'static str> {
    if let Some(canonical) = ferris_schema::fieldtype::canonical(name) {
        return Some(canonical);
    }
    match name.trim() {
        "SmallText" => Some("Small Text"),
        "LongText" => Some("Long Text"),
        "TextEditor" => Some("Text Editor"),
        "MarkdownEditor" => Some("Markdown Editor"),
        "DynamicLink" => Some("Dynamic Link"),
        "AttachImage" => Some("Attach Image"),
        _ => None,
    }
}

/// A `JSON` field's wire value must be an object or an array.
fn is_json_document(raw: &str) -> bool {
    matches!(
        serde_json::from_str::<serde_json::Value>(raw),
        Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_))
    )
}
