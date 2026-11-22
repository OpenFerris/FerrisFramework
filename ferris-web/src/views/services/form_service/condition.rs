//! Visibility conditions (`depends_on`) for form fields.
//!
//! A field may carry a condition (metadata's `depends_on`, and the same text on
//! a widget parameter) that decides whether the form shows it. The expression
//! comes from a DocType or an app's widget descriptor and is evaluated in the
//! browser, so handing it to a JavaScript engine would turn every metadata
//! author into code execution. This module implements a deliberately small,
//! side-effect-free subset instead:
//!
//! - field references (`status`, or `doc.status` for Frappe-style expressions);
//! - string (`'open'`, `"open"`), number and `true`/`false`/`null` literals;
//! - `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `||`, `!` and parentheses;
//! - an optional `eval:` prefix.
//!
//! Arithmetic, function calls and property access beyond the `doc.` prefix are
//! not part of the language. A malformed expression **shows** the field
//! (fail-open): a typo in metadata must not silently hide a parameter the user
//! needs in order to save the form. The parser therefore returns an error and
//! [`evaluate`] maps it to `true`.

use std::cmp::Ordering;

use leptos::prelude::Get;

use super::layout::FormField;

/// A resolved operand: a field's current value or an expression literal.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Text or a value that is not a check box (numbers arrive as text).
    Text(String),
    /// A `Check` field's boolean state.
    Bool(bool),
    /// A numeric literal.
    Number(f64),
}

impl Value {
    /// JS-like truthiness with the one convention the framework already uses
    /// for stored values: `"0"` and `"false"` are false, not just the empty
    /// string.
    #[must_use]
    pub fn truthy(&self) -> bool {
        match self {
            Self::Text(text) => {
                let text = text.trim();
                !text.is_empty() && text != "0" && !text.eq_ignore_ascii_case("false")
            }
            Self::Bool(value) => *value,
            Self::Number(number) => *number != 0.0,
        }
    }

    /// The value in the shape a string comparison expects. A check box has no
    /// text of its own; it compares as the `"1"`/`"0"` it is stored as.
    fn as_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Bool(true) => "1".to_string(),
            Self::Bool(false) => "0".to_string(),
            Self::Number(number) => number.to_string(),
        }
    }

    fn as_number(&self) -> Option<f64> {
        match self {
            Self::Text(text) => text.trim().parse::<f64>().ok(),
            Self::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
            Self::Number(number) => Some(*number),
        }
    }
}

/// The current value of one form field, read reactively.
#[must_use]
pub fn field_value(field: &FormField) -> Value {
    if field.fieldtype == "Check" {
        Value::Bool(field.checked.get())
    } else {
        Value::Text(field.value.get())
    }
}

/// Whether `field` is visible in the form that holds `fields`.
///
/// The single visibility rule: the renderer and validation both ask this, so a
/// field hidden by its condition is neither drawn nor able to block the save
/// with a "required" error.
#[must_use]
pub fn is_visible(fields: &[FormField], field: &FormField) -> bool {
    let Some(expression) = field.depends_on.as_deref() else {
        return true;
    };
    evaluate(expression, &|name| {
        fields
            .iter()
            .find(|candidate| candidate.fieldname == name)
            .map(field_value)
    })
}

/// Evaluate a `depends_on` expression against `lookup`.
///
/// An empty expression means "always visible". A syntax error or an unknown
/// token also means visible; an unknown *field* resolves to the empty string,
/// which is falsy, the same way an undefined value behaves in JavaScript.
#[must_use]
pub fn evaluate(expression: &str, lookup: &dyn Fn(&str) -> Option<Value>) -> bool {
    let expression = expression.trim();
    let expression = expression
        .strip_prefix("eval:")
        .map_or(expression, str::trim);
    if expression.is_empty() {
        return true;
    }
    let Ok(tokens) = tokenize(expression) else {
        return true;
    };
    match Parser::new(&tokens).parse(lookup) {
        Ok(value) => value.truthy(),
        Err(()) => true,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Text(String),
    Number(f64),
    True,
    False,
    Null,
    Eq,
    Ne,
    Ge,
    Le,
    Gt,
    Lt,
    And,
    Or,
    Not,
    LParen,
    RParen,
}

/// Split an expression into tokens, or `None` on a character the language does
/// not define.
fn tokenize(input: &str) -> Result<Vec<Token>, ()> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let current = chars[i];
        if current.is_whitespace() {
            i += 1;
            continue;
        }
        if i + 1 < chars.len() {
            let pair = (current, chars[i + 1]);
            let two_char = match pair {
                ('=', '=') => Some(Token::Eq),
                ('!', '=') => Some(Token::Ne),
                ('>', '=') => Some(Token::Ge),
                ('<', '=') => Some(Token::Le),
                ('&', '&') => Some(Token::And),
                ('|', '|') => Some(Token::Or),
                _ => None,
            };
            if let Some(token) = two_char {
                tokens.push(token);
                i += 2;
                continue;
            }
        }
        match current {
            '>' => {
                tokens.push(Token::Gt);
                i += 1;
            }
            '<' => {
                tokens.push(Token::Lt);
                i += 1;
            }
            '!' => {
                tokens.push(Token::Not);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '\'' | '"' => {
                let quote = current;
                i += 1;
                let mut text = String::new();
                let mut closed = false;
                while i < chars.len() {
                    let character = chars[i];
                    if character == '\\' {
                        i += 1;
                        if i < chars.len() {
                            text.push(chars[i]);
                            i += 1;
                        } else {
                            return Err(());
                        }
                    } else if character == quote {
                        i += 1;
                        closed = true;
                        break;
                    } else {
                        text.push(character);
                        i += 1;
                    }
                }
                if !closed {
                    return Err(());
                }
                tokens.push(Token::Text(text));
            }
            character
                if character.is_ascii_digit()
                    || (character == '.'
                        && i + 1 < chars.len()
                        && chars[i + 1].is_ascii_digit()) =>
            {
                let start = i;
                let mut seen_dot = false;
                while i < chars.len() {
                    let character = chars[i];
                    if character.is_ascii_digit() {
                        i += 1;
                    } else if character == '.' && !seen_dot {
                        seen_dot = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
                let raw: String = chars[start..i].iter().collect();
                tokens.push(Token::Number(raw.parse::<f64>().map_err(|_| ())?));
            }
            character if character.is_ascii_alphabetic() || character == '_' => {
                let start = i;
                i += 1;
                while i < chars.len() {
                    let character = chars[i];
                    if character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.') {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let word: String = chars[start..i].iter().collect();
                tokens.push(match word.as_str() {
                    "true" => Token::True,
                    "false" => Token::False,
                    "null" => Token::Null,
                    _ => Token::Ident(word),
                });
            }
            _ => return Err(()),
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self {
            tokens,
            position: 0,
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn advance(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.position);
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn parse(&mut self, lookup: &dyn Fn(&str) -> Option<Value>) -> Result<Value, ()> {
        let value = self.parse_or(lookup)?;
        if self.position == self.tokens.len() {
            Ok(value)
        } else {
            Err(())
        }
    }

    fn parse_or(&mut self, lookup: &dyn Fn(&str) -> Option<Value>) -> Result<Value, ()> {
        let mut left = self.parse_and(lookup)?;
        while self.eat(&Token::Or) {
            let right = self.parse_and(lookup)?;
            left = Value::Bool(left.truthy() || right.truthy());
        }
        Ok(left)
    }

    fn parse_and(&mut self, lookup: &dyn Fn(&str) -> Option<Value>) -> Result<Value, ()> {
        let mut left = self.parse_equality(lookup)?;
        while self.eat(&Token::And) {
            let right = self.parse_equality(lookup)?;
            left = Value::Bool(left.truthy() && right.truthy());
        }
        Ok(left)
    }

    fn parse_equality(&mut self, lookup: &dyn Fn(&str) -> Option<Value>) -> Result<Value, ()> {
        let mut left = self.parse_comparison(lookup)?;
        loop {
            if self.eat(&Token::Eq) {
                let right = self.parse_comparison(lookup)?;
                left = Value::Bool(equal(&left, &right));
            } else if self.eat(&Token::Ne) {
                let right = self.parse_comparison(lookup)?;
                left = Value::Bool(!equal(&left, &right));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_comparison(&mut self, lookup: &dyn Fn(&str) -> Option<Value>) -> Result<Value, ()> {
        let mut left = self.parse_unary(lookup)?;
        loop {
            let operator = match self.peek() {
                Some(Token::Gt) => Token::Gt,
                Some(Token::Lt) => Token::Lt,
                Some(Token::Ge) => Token::Ge,
                Some(Token::Le) => Token::Le,
                _ => return Ok(left),
            };
            self.position += 1;
            let right = self.parse_unary(lookup)?;
            let ordering = compare(&left, &right);
            let result = match (operator, ordering) {
                (Token::Gt, Some(Ordering::Greater)) => true,
                (Token::Lt, Some(Ordering::Less)) => true,
                (Token::Ge, Some(Ordering::Greater | Ordering::Equal)) => true,
                (Token::Le, Some(Ordering::Less | Ordering::Equal)) => true,
                _ => false,
            };
            left = Value::Bool(result);
        }
    }

    fn parse_unary(&mut self, lookup: &dyn Fn(&str) -> Option<Value>) -> Result<Value, ()> {
        if self.eat(&Token::Not) {
            let value = self.parse_unary(lookup)?;
            return Ok(Value::Bool(!value.truthy()));
        }
        self.parse_primary(lookup)
    }

    fn parse_primary(&mut self, lookup: &dyn Fn(&str) -> Option<Value>) -> Result<Value, ()> {
        match self.advance() {
            Some(Token::LParen) => {
                let value = self.parse_or(lookup)?;
                if !self.eat(&Token::RParen) {
                    return Err(());
                }
                Ok(value)
            }
            Some(Token::Text(text)) => Ok(Value::Text(text.clone())),
            Some(Token::Number(number)) => Ok(Value::Number(*number)),
            Some(Token::True) => Ok(Value::Bool(true)),
            Some(Token::False) => Ok(Value::Bool(false)),
            Some(Token::Null) => Ok(Value::Text(String::new())),
            Some(Token::Ident(name)) => {
                let name = name.strip_prefix("doc.").unwrap_or(name);
                Ok(lookup(name).unwrap_or_else(|| Value::Text(String::new())))
            }
            _ => Err(()),
        }
    }
}

fn equal(left: &Value, right: &Value) -> bool {
    match (left.as_number(), right.as_number()) {
        (Some(left), Some(right)) => (left - right).abs() < f64::EPSILON,
        _ => left.as_text() == right.as_text(),
    }
}

fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    match (left.as_number(), right.as_number()) {
        (Some(left), Some(right)) => left.partial_cmp(&right),
        _ => Some(left.as_text().cmp(&right.as_text())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluate against a fixed lookup, the shape the tests need.
    fn eval(expression: &str, fields: &[(&str, &str)]) -> bool {
        evaluate(expression, &|name| {
            fields
                .iter()
                .find(|(field, _)| *field == name)
                .map(|(_, value)| Value::Text((*value).to_string()))
        })
    }

    #[test]
    fn empty_expression_is_always_visible() {
        assert!(eval("", &[]));
        assert!(eval("   ", &[]));
        assert!(eval("eval:", &[]));
    }

    #[test]
    fn compares_strings_like_frappe_expressions() {
        let fields = [("status", "Open")];
        assert!(eval("doc.status == 'Open'", &fields));
        assert!(eval("eval:doc.status == \"Open\"", &fields));
        assert!(!eval("doc.status == 'Closed'", &fields));
        assert!(eval("doc.status != 'Closed'", &fields));
        assert!(eval("status", &fields));
    }

    #[test]
    fn checks_are_truthy_and_compare_as_stored() {
        assert!(evaluate("enabled", &|name| {
            (name == "enabled").then_some(Value::Bool(true))
        }));
        assert!(!evaluate("enabled", &|name| {
            (name == "enabled").then_some(Value::Bool(false))
        }));
        assert!(evaluate("enabled == true", &|name| {
            (name == "enabled").then_some(Value::Bool(true))
        }));
        assert!(evaluate("enabled == '1'", &|name| {
            (name == "enabled").then_some(Value::Bool(true))
        }));
        assert!(evaluate("!enabled", &|name| {
            (name == "enabled").then_some(Value::Bool(false))
        }));
    }

    #[test]
    fn numbers_compare_numerically_not_lexicographically() {
        let fields = [("count", "10")];
        assert!(eval("doc.count > 3", &fields));
        assert!(eval("doc.count >= 10", &fields));
        assert!(!eval("doc.count < 9", &fields));
        assert!(eval("doc.count == 10.0", &fields));
    }

    #[test]
    fn boolean_operators_and_parentheses_short_circuit_nothing_but_compose() {
        let fields = [("a", "1"), ("b", ""), ("c", "yes")];
        assert!(eval("a && c", &fields));
        assert!(!eval("a && b", &fields));
        assert!(eval("a || b", &fields));
        assert!(eval("(a || b) && c", &fields));
        assert!(!eval("!(a && c)", &fields));
        assert!(eval("a && !b", &fields));
    }

    #[test]
    fn widget_parameter_names_may_contain_hyphens() {
        let fields = [("page-size", "25")];
        assert!(eval("page-size >= 10", &fields));
        assert!(!eval("page-size == 10", &fields));
    }

    #[test]
    fn unknown_fields_are_falsy_and_malformed_expressions_fail_open() {
        assert!(!eval("doc.missing", &[]));
        assert!(eval("!doc.missing", &[]));
        // Fail-open: a typo must not hide the control.
        assert!(eval("doc.count ==", &[("count", "10")]));
        assert!(eval("doc.count &&& 1", &[("count", "10")]));
        assert!(eval("doc.count == 'unterminated", &[("count", "10")]));
    }
}
