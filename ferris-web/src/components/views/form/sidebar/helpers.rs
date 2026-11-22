//! Pure helpers of the form sidebar.

use serde_json::Value;

use crate::api::DynMeta;
use crate::api::meta::title_field;

/// Split the document's `tags` field into individual tags.
pub(super) fn parse_tags(raw: Option<&Value>) -> Vec<String> {
    raw.and_then(Value::as_str)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

/// The `owner` user id of a document, or an empty string.
pub(super) fn owner_of(doc: &Value) -> String {
    doc.get("owner")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `modified_by` user id of a document, with a fallback to `owner`.
pub(super) fn edited_by(doc: &Value) -> String {
    doc.get("modified_by")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && *value != "—")
        .map(str::to_string)
        .unwrap_or_else(|| owner_of(doc))
}

/// A timestamp field of a document, or an empty string.
pub(super) fn stamp_of(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The display title of the open document: its declared title field, else its
/// name.
///
/// The sidebar header describes **the document on screen**, not the person who
/// created it: opening a User shows that user's full name and email, opening a
/// task shows the task's title and its id.
pub(super) fn document_title(doc: &Value, meta: Option<&DynMeta>) -> String {
    let field = meta.map(title_field).unwrap_or("name");
    let text_of = |key: &str| {
        doc.get(key)
            .and_then(|value| match value {
                Value::String(text) => Some(text.trim().to_string()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .filter(|text| !text.is_empty())
    };
    // The declared title is authoritative. When the metadata only points at the
    // name (`Widgets`, a doctype without `title_field`), the shared heuristic
    // picks the field that reads as a title; the id is the last resort.
    let declared = text_of(field).filter(|_| field != "name");
    declared
        .or_else(|| {
            let label = crate::api::doc_label(doc);
            (!label.is_empty()).then_some(label)
        })
        .or_else(|| text_of("name"))
        .unwrap_or_default()
}

/// The line under the title: the email of a user document, the document id when
/// the title is something else, nothing when the id is already the title.
pub(super) fn document_subtitle(doc: &Value, doctype: &str, title: &str) -> String {
    if doctype.eq_ignore_ascii_case("User") {
        return doc
            .get("email")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
    }
    let name = doc.get("name").and_then(Value::as_str).unwrap_or_default();
    if name.is_empty() || name == title {
        String::new()
    } else {
        name.to_string()
    }
}

/// The document's own picture, when it declares one.
pub(super) fn document_image(doc: &Value) -> String {
    for key in ["image", "user_image", "photo"] {
        if let Some(text) = doc.get(key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.trim().to_string();
        }
    }
    String::new()
}
