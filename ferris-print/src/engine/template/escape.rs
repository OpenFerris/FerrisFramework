//! Escaping helpers for the Jinja → Typst boundary.
//!
//! A print format is Typst source with Jinja expressions in it. A document
//! value is arbitrary text, so `{{ doc.title }}` must be escaped for **Typst
//! markup**, not HTML: an unescaped `#`, `*`, `[` or `$` changes the meaning of
//! the surrounding document (or refuses to compile). The filters are explicit
//! (`| typst`, `| typst_str`, `| typst_value`) because the right escaping
//! depends on where the value lands: text, a string literal, or code.

/// Escape a value for Typst **markup**.
#[must_use]
pub(super) fn typst_markup(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '\\' | '#'
                | '*'
                | '_'
                | '$'
                | '['
                | ']'
                | '@'
                | '<'
                | '>'
                | '~'
                | '-'
                | '+'
                | '/'
                | '='
                | '`'
                | '"'
                | '\''
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Escape a value for a Typst **string literal** (`"..."`).
#[must_use]
pub(super) fn typst_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

/// Render a JSON value as a Typst **code literal**.
///
/// This is what lets a template build a value once and use it as code:
///
/// ```text
/// #let rows = {{ doc.items | typst_value }}
/// #for row in rows [ #row.name ]
/// ```
#[must_use]
pub(super) fn typst_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "none".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => format!("\"{}\"", typst_string(value)),
        serde_json::Value::Array(values) => {
            let mut out = String::from("(");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&typst_value(value));
            }
            // A one-element Typst array needs a trailing comma.
            if values.len() == 1 {
                out.push(',');
            }
            out.push(')');
            out
        }
        serde_json::Value::Object(values) => {
            if values.is_empty() {
                return "(:)".to_string();
            }
            let mut out = String::from("(");
            for (index, (key, value)) in values.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                if is_identifier(key) {
                    out.push_str(key);
                } else {
                    out.push('"');
                    out.push_str(&typst_string(key));
                    out.push('"');
                }
                out.push_str(": ");
                out.push_str(&typst_value(value));
            }
            out.push(')');
            out
        }
    }
}

/// Whether `key` can be used as a Typst identifier (and therefore as a named
/// pair key without quotes).
fn is_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_escapes_syntax_characters() {
        assert_eq!(typst_markup("#1 *bold* [x]"), "\\#1 \\*bold\\* \\[x\\]");
        assert_eq!(typst_markup("plain text"), "plain text");
    }

    #[test]
    fn string_escapes_quotes_and_newlines() {
        assert_eq!(typst_string("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }

    #[test]
    fn value_renders_a_typst_literal() {
        let value = serde_json::json!({
            "name": "A \"B\"",
            "qty": 2,
            "rows": [1, 2],
            "one": [1],
        });
        let rendered = typst_value(&value);
        assert!(rendered.contains("name: \"A \\\"B\\\"\""), "{rendered}");
        assert!(rendered.contains("qty: 2"), "{rendered}");
        assert!(rendered.contains("rows: (1, 2)"), "{rendered}");
        assert!(rendered.contains("one: (1,)"), "{rendered}");
    }
}
