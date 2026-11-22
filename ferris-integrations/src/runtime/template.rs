//! Small, dependency-free template renderer for the integrations.
//!
//! Webhook bodies, automation action messages and auto-email-report subjects all
//! interpolate the triggering document with `{{ … }}` placeholders:
//!
//! | Placeholder | Rendered as |
//! |---|---|
//! | `{{ doc }}` | the whole document as JSON |
//! | `{{ doc.<field> }}` | the field value — a string as-is, anything else as JSON |
//! | `{{ doctype }}` | the DocType name |
//! | `{{ docname }}` | the document name |
//!
//! Unknown fields render as the empty string (a template must not fail a save
//! because a field was renamed). Malformed `{{ … }}` blocks are left untouched.

use std::collections::HashMap;

use ferris_core::Value;

use super::state::ValueMap;

/// Render `text` against `fields` of the document named `docname` (`doctype`).
#[must_use]
pub fn render(text: &str, doctype: &str, docname: &str, fields: &ValueMap) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str("{{");
            out.push_str(after);
            rest = "";
            break;
        };
        let token = after[..close].trim();
        let rendered = match token {
            "" => String::new(),
            "doc" => {
                serde_json::to_string(&fields_to_json(fields)).unwrap_or_else(|_| "{}".to_string())
            }
            "doctype" => doctype.to_string(),
            "docname" => docname.to_string(),
            // `doc.<field>`, possibly with a `doc.`/`this`.`-style qualifier in
            // a Frappe-shaped condition.
            _ => token
                .strip_prefix("doc.")
                .or_else(|| token.strip_prefix("this."))
                .map(|field| render_field(fields, field))
                .unwrap_or_default(),
        };
        out.push_str(&rendered);
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

fn render_field(fields: &HashMap<String, Value>, field: &str) -> String {
    match fields.get(field) {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(value) => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn fields_to_json(fields: &HashMap<String, Value>) -> serde_json::Value {
    serde_json::Value::Object(
        fields
            .iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> HashMap<String, Value> {
        HashMap::from([
            ("title".to_string(), Value::from("Invoice 42")),
            ("amount".to_string(), Value::from(1000)),
            ("enabled".to_string(), Value::from(true)),
            (
                "nested".to_string(),
                Value::from_json(serde_json::json!({"a": 1})),
            ),
        ])
    }

    #[test]
    fn interpolates_fields_doctype_and_docname() {
        let text = "{{ doc.title }} / {{ doctype }} / {{ docname }}";
        assert_eq!(
            render(text, "Sales Invoice", "INV-0001", &fields()),
            "Invoice 42 / Sales Invoice / INV-0001"
        );
    }

    #[test]
    fn non_string_fields_render_as_json() {
        assert_eq!(
            render("{{ doc.amount }}|{{ doc.enabled }}", "X", "1", &fields()),
            "1000|true"
        );
    }

    #[test]
    fn whole_doc_render_is_json() {
        let rendered = render("{{ doc }}", "X", "1", &fields());
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
        assert_eq!(parsed["title"], "Invoice 42");
    }

    #[test]
    fn unknown_field_is_empty_and_malformed_token_is_left_alone() {
        assert_eq!(render("a{{ doc.missing }}b", "X", "1", &fields()), "ab");
        assert_eq!(
            render("a{{ unfinished", "X", "1", &fields()),
            "a{{ unfinished"
        );
    }

    #[test]
    fn strips_a_frappe_style_doc_qualifier_in_field_token() {
        let text = "{{ doc.title }}";
        assert_eq!(render(text, "X", "1", &fields()), "Invoice 42");
    }
}
