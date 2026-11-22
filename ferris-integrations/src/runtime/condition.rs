//! A small, safe condition language for Webhooks, Automation Flows and
//! Assignment Rules.
//!
//! The catalogue shapes these fields as plain text (Frappe lets a hook author
//! write a Python expression there). Executing Python from a text field is a
//! code-execution vector and out of scope; this parser covers the conditions
//! people actually write, deterministically:
//!
//! ```text
//! status == "Open" and amount >= 1000
//! workflow_state == "Approved" or priority == "high"
//! title contains "invoice" and active == true
//! ```
//!
//! * operands: `==`, `!=`, `>`, `>=`, `<`, `<=`, `contains`
//! * connectors: `and`, `or` (`and` binds tighter, as usual)
//! * literals: quoted strings (`'…'` or `"…"`), numbers, `true`/`false`
//! * fields may keep a Frappe-ish `doc.` prefix (`doc.status == "Open"`)
//!
//! A missing field never matches (including under `!=` — a document that
//! dropped a field must not suddenly satisfy negations). An unparseable or
//! unevaluable condition is an `Err`, which callers log and treat as *no match*
//! so a typo cannot cause a hook to fire on every document.

use std::collections::HashMap;

use ferris_core::Value;

use super::state::ValueMap;

/// Evaluate `expr` against `fields`.
///
/// # Errors
///
/// Returns `Err` when the expression does not tokenize or parse, or when a
/// comparison cannot be evaluated — callers log it and treat the condition as
/// *no match*.
pub fn evaluate(expr: &str, fields: &ValueMap) -> Result<bool, String> {
    let tokens = tokenize(expr)?;
    if tokens.is_empty() {
        return Err("condition is empty".into());
    }
    let (parsed, rest) = parse_or(&tokens, 0)?;
    if rest < tokens.len() {
        return Err(format!("unexpected token {}", tokens[rest].describe()));
    }
    eval_node(&parsed, fields)
}

/// Evaluate a plain-text condition that may be empty/blank (treated as "always
/// matches").
///
/// # Errors
///
/// Propagates [`evaluate`]'s parse/evaluation errors for non-blank input.
pub fn evaluate_optional(expr: Option<&str>, fields: &ValueMap) -> Result<bool, String> {
    match expr {
        None => Ok(true),
        Some(expr) if expr.trim().is_empty() => Ok(true),
        Some(expr) => evaluate(expr, fields),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Num(String),
    Bool(bool),
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
    And,
    Or,
    LParen,
    RParen,
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Self::Ident(s) | Self::Str(s) | Self::Num(s) => format!("`{s}`"),
            Self::Bool(b) => format!("`{b}`"),
            Self::Eq => "`==`".into(),
            Self::Ne => "`!=`".into(),
            Self::Gt => "`>`".into(),
            Self::Ge => "`>=`".into(),
            Self::Lt => "`<`".into(),
            Self::Le => "`<=`".into(),
            Self::Contains => "`contains`".into(),
            Self::And => "`and`".into(),
            Self::Or => "`or`".into(),
            Self::LParen => "`(`".into(),
            Self::RParen => "`)`".into(),
        }
    }
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            let mut value = String::new();
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == quote {
                    closed = true;
                    i += 1;
                    break;
                }
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                value.push(chars[i]);
                i += 1;
            }
            if !closed {
                return Err("unterminated string literal".into());
            }
            tokens.push(Token::Str(value));
            continue;
        }
        if c.is_ascii_digit() || (c == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == '-')
            {
                i += 1;
            }
            let num: String = chars[start..i].iter().collect();
            tokens.push(Token::Num(num));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            tokens.push(match word.as_str() {
                "and" => Token::And,
                "or" => Token::Or,
                "true" => Token::Bool(true),
                "false" => Token::Bool(false),
                "contains" => Token::Contains,
                _ => Token::Ident(word),
            });
            continue;
        }
        if i + 1 < chars.len() {
            let pair: String = [c, chars[i + 1]].iter().collect();
            let two = match pair.as_str() {
                "==" => Some(Token::Eq),
                "!=" => Some(Token::Ne),
                ">=" => Some(Token::Ge),
                "<=" => Some(Token::Le),
                _ => None,
            };
            if let Some(tok) = two {
                tokens.push(tok);
                i += 2;
                continue;
            }
        }
        match c {
            '>' => tokens.push(Token::Gt),
            '<' => tokens.push(Token::Lt),
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            other => return Err(format!("unexpected character `{other}`")),
        }
        i += 1;
    }
    Ok(tokens)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Compare {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
}

#[derive(Debug, Clone, PartialEq)]
enum Literal {
    Str(String),
    Num(String),
    Bool(bool),
}

#[derive(Debug)]
enum Node {
    Leaf {
        field: String,
        op: Compare,
        literal: Literal,
    },
    Join {
        left: Box<Self>,
        right: Box<Self>,
        op: JoinKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum JoinKind {
    And,
    Or,
}

fn parse_leaf(tokens: &[Token], i: usize) -> Result<(Node, usize), String> {
    let Some(Token::Ident(field)) = tokens.get(i) else {
        return Err(tokens.get(i).map_or_else(
            || "expected a field, found end of condition".to_string(),
            |tok| format!("expected a field, found {}", tok.describe()),
        ));
    };
    let field = field.clone();
    let j = i + 1;
    let (op, k) = match tokens.get(j) {
        Some(Token::Eq) => (Compare::Eq, j + 1),
        Some(Token::Ne) => (Compare::Ne, j + 1),
        Some(Token::Gt) => (Compare::Gt, j + 1),
        Some(Token::Ge) => (Compare::Ge, j + 1),
        Some(Token::Lt) => (Compare::Lt, j + 1),
        Some(Token::Le) => (Compare::Le, j + 1),
        Some(Token::Contains) => (Compare::Contains, j + 1),
        Some(tok) => {
            return Err(format!(
                "expected a comparison operator after `{field}`, found {}",
                tok.describe()
            ));
        }
        None => return Err(format!("expected a comparison operator after `{field}`")),
    };
    let (literal, end) = match tokens.get(k) {
        Some(Token::Str(s)) => (Literal::Str(s.clone()), k + 1),
        Some(Token::Num(n)) => (Literal::Num(n.clone()), k + 1),
        Some(Token::Bool(b)) => (Literal::Bool(*b), k + 1),
        Some(tok) => {
            return Err(format!("expected a value, found {}", tok.describe()));
        }
        None => return Err("expected a value, found end of condition".to_string()),
    };
    Ok((Node::Leaf { field, op, literal }, end))
}

/// One condition *unit*: a leaf comparison or a `(…)` group. Groups parse a full
/// `or` expression so operator precedence inside them is self-contained.
fn parse_primary(tokens: &[Token], i: usize) -> Result<(Node, usize), String> {
    match tokens.get(i) {
        Some(Token::LParen) => {
            let (node, j) = parse_or(tokens, i + 1)?;
            if !matches!(tokens.get(j), Some(Token::RParen)) {
                return Err(tokens.get(j).map_or_else(
                    || "expected `)` found end of condition".to_string(),
                    |tok| format!("expected `)`, found {}", tok.describe()),
                ));
            }
            Ok((node, j + 1))
        }
        _ => parse_leaf(tokens, i),
    }
}

fn parse_and(tokens: &[Token], i: usize) -> Result<(Node, usize), String> {
    let (mut node, mut j) = parse_primary(tokens, i)?;
    while matches!(tokens.get(j), Some(Token::And)) {
        let (right, k) = parse_primary(tokens, j + 1)?;
        node = Node::Join {
            left: Box::new(node),
            right: Box::new(right),
            op: JoinKind::And,
        };
        j = k;
    }
    Ok((node, j))
}

fn parse_or(tokens: &[Token], i: usize) -> Result<(Node, usize), String> {
    let (mut node, mut j) = parse_and(tokens, i)?;
    while matches!(tokens.get(j), Some(Token::Or)) {
        let (right, k) = parse_and(tokens, j + 1)?;
        node = Node::Join {
            left: Box::new(node),
            right: Box::new(right),
            op: JoinKind::Or,
        };
        j = k;
    }
    Ok((node, j))
}

fn field_value<'a>(fields: &'a HashMap<String, Value>, field: &str) -> Option<&'a Value> {
    let clean = field
        .strip_prefix("doc.")
        .or_else(|| field.strip_prefix("this."))
        .unwrap_or(field);
    fields.get(clean)
}

fn norm_string(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.trim().to_string();
    }
    if let Some(b) = value.as_bool() {
        return b.to_string();
    }
    if let Some(i) = value.as_i64() {
        return i.to_string();
    }
    if let Some(f) = value.as_f64() {
        return fmt_f64(f);
    }
    if value.is_null() {
        return String::new();
    }
    serde_json::to_string(&value.to_json()).unwrap_or_default()
}

pub(crate) fn fmt_f64(f: f64) -> String {
    if f.fract() == 0.0 && f.is_finite() && f.abs() < 1e15 {
        // The range guard keeps |f| < 2^53, so the `f64 -> i64` cast is exact
        // (`cast_possible_truncation` cannot see the `fract()` + range guards).
        #[allow(clippy::cast_possible_truncation)]
        (f as i64).to_string()
    } else {
        f.to_string()
    }
}

fn as_number(value: &Value) -> Option<f64> {
    if let Some(i) = value.as_i64() {
        // Condition compares against 1e15-guarded values elsewhere; an integer
        // beyond f64's exact range rounds only at the 53rd bit, which no real
        // condition reaches (`cast_precision_loss`).
        #[allow(clippy::cast_precision_loss)]
        return Some(i as f64);
    }
    if let Some(f) = value.as_f64() {
        return Some(f);
    }
    value.as_str().and_then(|s| s.trim().parse::<f64>().ok())
}

fn literal_str(literal: &Literal) -> String {
    match literal {
        Literal::Str(s) => s.clone(),
        Literal::Num(n) => n.clone(),
        Literal::Bool(b) => b.to_string(),
    }
}

/// Order two comparable values (`PartialOrd`) by the ordering operator.
fn order_by<T: PartialOrd>(op: Compare, a: &T, b: &T) -> bool {
    match op {
        Compare::Gt => a > b,
        Compare::Ge => a >= b,
        Compare::Lt => a < b,
        _ => a <= b,
    }
}

fn compare(op: Compare, value: &Value, literal: &Literal) -> bool {
    match op {
        Compare::Eq => norm_string(value) == literal_str(literal),
        Compare::Ne => norm_string(value) != literal_str(literal),
        Compare::Contains => norm_string(value).contains(&literal_str(literal)),
        Compare::Gt | Compare::Ge | Compare::Lt | Compare::Le => {
            match (as_number(value), literal_str(literal).parse::<f64>().ok()) {
                (Some(a), Some(b)) => order_by(op, &a, &b),
                // Non-numeric operands fall back to lexicographic order (ISO
                // datetimes sort correctly this way).
                _ => order_by(op, &norm_string(value), &literal_str(literal)),
            }
        }
    }
}

fn eval_node(node: &Node, fields: &ValueMap) -> Result<bool, String> {
    match node {
        Node::Leaf { field, op, literal } => {
            let Some(value) = field_value(fields, field) else {
                return Ok(false); // a missing field never matches
            };
            Ok(compare(*op, value, literal))
        }
        Node::Join { left, right, op } => {
            let l = eval_node(left, fields)?;
            let r = eval_node(right, fields)?;
            Ok(match op {
                JoinKind::And => l && r,
                JoinKind::Or => l || r,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn f(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn equality_on_strings_numbers_and_bools() {
        let fields = f(&[
            ("status", Value::from("Open")),
            ("amount", Value::from(1000)),
            ("active", Value::from(true)),
        ]);
        assert!(evaluate(r#"status == "Open""#, &fields).unwrap());
        assert!(!evaluate(r#"status == "Closed""#, &fields).unwrap());
        assert!(evaluate("amount == 1000", &fields).unwrap());
        assert!(evaluate("amount >= 1000", &fields).unwrap());
        assert!(evaluate("active == true", &fields).unwrap());
        assert!(evaluate("active != false", &fields).unwrap());
    }

    #[test]
    fn and_or_combinations() {
        let fields = f(&[
            ("a", Value::from(1)),
            ("b", Value::from(2)),
            ("c", Value::from(3)),
        ]);
        assert!(evaluate("a == 1 and b == 2 or c == 99", &fields).unwrap());
        assert!(!evaluate("a == 1 and (b == 9)", &fields).unwrap());
        assert!(evaluate("a == 9 or b == 2 or c == 99", &fields).unwrap());
        assert!(!evaluate("a == 9 or b == 9 or c == 99", &fields).unwrap());
    }

    #[test]
    fn contains_is_case_sensitive_substring() {
        let fields = f(&[("title", Value::from("Quarterly Invoice"))]);
        assert!(evaluate(r#"title contains "Invoice""#, &fields).unwrap());
        assert!(!evaluate(r#"title contains "invoice""#, &fields).unwrap());
    }

    #[test]
    fn doc_prefixed_fields_resolve() {
        let fields = f(&[("status", Value::from("Open"))]);
        assert!(evaluate(r#"doc.status == "Open""#, &fields).unwrap());
    }

    #[test]
    fn missing_field_never_matches_even_for_inequality() {
        let fields = f(&[]);
        assert!(!evaluate(r#"status == "Open""#, &fields).unwrap());
        assert!(!evaluate(r#"status != "Open""#, &fields).unwrap());
    }

    #[test]
    fn numeric_falls_back_to_lexicographic_for_dates() {
        let fields = f(&[("due_on", Value::from("2026-12-01"))]);
        assert!(evaluate(r#"due_on > "2026-01-01""#, &fields).unwrap());
        assert!(!evaluate(r#"due_on < "2026-01-01""#, &fields).unwrap());
    }

    #[test]
    fn syntax_errors_are_errors() {
        let fields = f(&[]);
        assert!(evaluate("status", &fields).is_err());
        assert!(evaluate("status == ", &fields).is_err());
        assert!(evaluate("== 42", &fields).is_err());
        assert!(evaluate("status === \"x\"", &fields).is_err());
        assert!(evaluate("", &fields).is_err());
    }

    #[test]
    fn blank_conditions_always_match_via_optional() {
        let fields = f(&[]);
        assert!(evaluate_optional(None, &fields).unwrap());
        assert!(evaluate_optional(Some("   "), &fields).unwrap());
        assert!(evaluate_optional(Some("amount > 1"), &f(&[("amount", Value::from(2))])).unwrap());
    }

    #[test]
    fn numbers_interpolate_and_compare() {
        let fields = f(&[("total", Value::from_json(json!(10.5)))]);
        assert!(evaluate("total > 10", &fields).unwrap());
        assert!(!evaluate("total < 10", &fields).unwrap());
    }
}
