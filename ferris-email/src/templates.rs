//! Email template engine

use ferris_core::{EmailError, Error, Result, Value};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Template engine with the framework's built-in templates pre-registered.
///
/// Keys are file names including the extension (`welcome.html`,
/// `welcome.txt`), which is exactly what the job handler looks up for a payload
/// whose `template` is `welcome`. The same convention is used by
/// [`TemplateEngine::load_from_dir`], so a directory load and the compiled
/// built-ins cannot drift: a file named `welcome.html` replaces the built-in of
/// the same key.
pub struct TemplateEngine {
    templates: Arc<RwLock<HashMap<String, String>>>,
}

impl TemplateEngine {
    /// An engine with the built-in templates already registered.
    ///
    /// `EmailJobHandler` builds this with `TemplateEngine::new()` (see
    /// `ferris-server`'s email hub), so the server path can render a templated
    /// email without any extra wiring. Before this, `new()` returned an empty
    /// registry and every templated send fell through to an empty body.
    #[must_use]
    pub fn new() -> Self {
        let engine = Self {
            templates: Arc::new(RwLock::new(HashMap::new())),
        };
        for template in builtin::all() {
            engine.register_template(&template);
        }
        engine
    }

    /// Register an [`EmailTemplate`]'s bodies under its file-name keys.
    ///
    /// A template with no body registers nothing, so a caller that asks for it
    /// gets the engine's `NotFound` instead of an empty message.
    pub fn register_template(&self, template: &EmailTemplate) {
        if let Some(html) = &template.html_template {
            self.add_template(format!("{}.html", template.name), html.clone());
        }
        if let Some(text) = &template.text_template {
            self.add_template(format!("{}.txt", template.name), text.clone());
        }
    }

    /// Add template
    pub fn add_template(&self, name: String, content: String) {
        self.templates.write().insert(name, content);
    }

    /// Get template
    pub fn get_template(&self, name: &str) -> Option<String> {
        self.templates.read().get(name).cloned()
    }

    /// Remove template
    pub fn remove_template(&self, name: &str) -> bool {
        self.templates.write().remove(name).is_some()
    }

    /// List templates
    pub fn list_templates(&self) -> Vec<String> {
        self.templates.read().keys().cloned().collect()
    }

    /// Render template with context, HTML-escaping substituted values.
    ///
    /// This is the safe default for HTML bodies; use [`TemplateEngine::render_raw`]
    /// for plain-text output.
    pub fn render(&self, template_name: &str, context: &Value) -> Result<String> {
        self.render_with(template_name, context, true)
    }

    /// Render template without escaping (plain-text bodies).
    pub fn render_raw(&self, template_name: &str, context: &Value) -> Result<String> {
        self.render_with(template_name, context, false)
    }

    fn render_with(&self, template_name: &str, context: &Value, escape: bool) -> Result<String> {
        let template = {
            let templates = self.templates.read();
            templates.get(template_name).cloned().ok_or_else(|| {
                Error::Email(EmailError::NotFound(format!(
                    "Template {} not found",
                    template_name
                )))
            })?
        };
        self.render_str(&template, context, escape)
    }

    /// Render a template string (no registry lookup), optionally HTML-escaping
    /// the substituted values.
    ///
    /// Substitution is a **single pass over the original template**: a value
    /// that itself contains `{{ other }}` is emitted literally. The previous
    /// implementation applied a `String::replace` per context key to the
    /// running result, so a value could be expanded by a later iteration —
    /// order-dependent and an injection vector when `escape` is false.
    ///
    /// An unknown `{{ key }}` is left verbatim rather than blanked, so a
    /// template referring to a missing value is visible instead of silently
    /// producing an empty body. An unterminated `{{` leaves the remainder of
    /// the template untouched.
    pub fn render_str(&self, template: &str, context: &Value, escape: bool) -> Result<String> {
        let Value::Object(obj) = context else {
            return Ok(template.to_string());
        };
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                out.push_str(&rest[start..]);
                return Ok(out);
            };
            let key = after[..end].trim();
            let Some(replacement) = obj.get(key).map(|value| match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => serde_json::to_string(value).unwrap_or_default(),
            }) else {
                // Unknown placeholder: keep it visible, do not blank it.
                out.push_str(&rest[start..start + 2 + end + 2]);
                rest = &after[end + 2..];
                continue;
            };
            if escape {
                out.push_str(&escape_html(&replacement));
            } else {
                out.push_str(&replacement);
            }
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// Render template with typed context
    pub fn render_typed<T: serde::Serialize>(
        &self,
        template_name: &str,
        context: &T,
    ) -> Result<String> {
        let value = ferris_core::Value::from(serde_json::to_value(context)?);
        self.render(template_name, &value)
    }

    /// Load templates from a directory.
    ///
    /// Files are keyed by their name **including the extension**
    /// (`welcome.html`), the same key the job handler and the built-ins use.
    /// The previous implementation keyed by file stem, so a directory loaded at
    /// boot produced `welcome` while the handler looked up `welcome.html` and
    /// every templated send fell through to an empty body.
    ///
    /// # Errors
    ///
    /// Fails when the directory does not exist or a file cannot be read.
    pub async fn load_from_dir(&self, dir: std::path::PathBuf) -> Result<()> {
        let mut templates = self.templates.write();

        if !dir.exists() {
            return Err(Error::Email(EmailError::NotFound(format!(
                "Template directory not found: {:?}",
                dir
            ))));
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("html")
                || path.extension().and_then(|s| s.to_str()) == Some("txt")
            {
                let content = std::fs::read_to_string(&path)?;
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                templates.insert(name.to_string(), content);
            }
        }

        Ok(())
    }
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal HTML escaping for interpolated template values.
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Email template with subject and body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailTemplate {
    pub name: String,
    pub subject: String,
    pub html_template: Option<String>,
    pub text_template: Option<String>,
    pub variables: Vec<String>, // Expected variables
}

impl EmailTemplate {
    pub fn new(name: String, subject: String) -> Self {
        Self {
            name,
            subject,
            html_template: None,
            text_template: None,
            variables: Vec::new(),
        }
    }

    pub fn with_html(mut self, template: String) -> Self {
        self.html_template = Some(template);
        self
    }

    pub fn with_text(mut self, template: String) -> Self {
        self.text_template = Some(template);
        self
    }

    pub fn with_variables(mut self, variables: Vec<String>) -> Self {
        self.variables = variables;
        self
    }

    /// Render email with context.
    ///
    /// `html_template`/`text_template` hold template **source** (as produced by
    /// the builtins); the subject is rendered from `self.subject`.
    pub fn render(&self, engine: &TemplateEngine, context: &Value) -> Result<EmailRender> {
        let subject = engine.render_str(&self.subject, context, false)?;

        let html = match &self.html_template {
            Some(template) => Some(engine.render_str(template, context, true)?),
            None => None,
        };

        let text = match &self.text_template {
            Some(template) => Some(engine.render_str(template, context, false)?),
            None => None,
        };

        Ok(EmailRender {
            subject,
            html,
            text,
        })
    }
}

/// Rendered email
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailRender {
    pub subject: String,
    pub html: Option<String>,
    pub text: Option<String>,
}

/// Built-in email templates
pub mod builtin {
    use super::*;

    /// Every compiled built-in, in registration order.
    ///
    /// [`TemplateEngine::new`] walks this list so the server's default engine
    /// is not empty. Keep it in sync when adding a template.
    #[must_use]
    pub fn all() -> Vec<EmailTemplate> {
        vec![
            welcome_template(),
            password_reset_template(),
            notification_template(),
            invoice_template(),
        ]
    }

    pub fn welcome_template() -> EmailTemplate {
        EmailTemplate::new(
            "welcome".to_string(),
            "Welcome to {{ app_name }}!".to_string(),
        )
        .with_html(include_str!("templates/welcome.html").to_string())
        .with_text(include_str!("templates/welcome.txt").to_string())
        .with_variables(vec![
            "app_name".to_string(),
            "user_name".to_string(),
            "login_url".to_string(),
        ])
    }

    pub fn password_reset_template() -> EmailTemplate {
        EmailTemplate::new(
            "password_reset".to_string(),
            "Reset your password".to_string(),
        )
        .with_html(include_str!("templates/password_reset.html").to_string())
        .with_text(include_str!("templates/password_reset.txt").to_string())
        .with_variables(vec![
            "reset_url".to_string(),
            "user_name".to_string(),
            "expiry_hours".to_string(),
        ])
    }

    pub fn notification_template() -> EmailTemplate {
        EmailTemplate::new("notification".to_string(), "{{ title }}".to_string())
            .with_html(include_str!("templates/notification.html").to_string())
            .with_text(include_str!("templates/notification.txt").to_string())
            .with_variables(vec![
                "title".to_string(),
                "message".to_string(),
                "action_url".to_string(),
                "action_label".to_string(),
            ])
    }

    pub fn invoice_template() -> EmailTemplate {
        EmailTemplate::new(
            "invoice".to_string(),
            "Invoice {{ invoice_number }}".to_string(),
        )
        .with_html(include_str!("templates/invoice.html").to_string())
        .with_text(include_str!("templates/invoice.txt").to_string())
        .with_variables(vec![
            "invoice_number".to_string(),
            "customer_name".to_string(),
            "amount".to_string(),
            "due_date".to_string(),
            "pdf_url".to_string(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_engine() {
        let engine = TemplateEngine::new();
        engine.add_template("test".to_string(), "Hello {{ name }}!".to_string());

        let mut context = std::collections::HashMap::new();
        context.insert("name".to_string(), ferris_core::Value::from("World"));

        let result = engine
            .render("test", &ferris_core::Value::Object(context))
            .unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_email_template() {
        let template = EmailTemplate::new("test".to_string(), "Hello {{ name }}".to_string())
            .with_html("<h1>Hello {{ name }}</h1>".to_string())
            .with_text("Hello {{ name }}".to_string());

        assert_eq!(template.name, "test");
        assert!(template.html_template.is_some());
        assert!(template.text_template.is_some());
    }

    #[test]
    fn builtins_are_registered_under_the_lookup_keys() {
        let engine = TemplateEngine::new();
        for template in builtin::all() {
            if template.html_template.is_some() {
                assert!(
                    engine
                        .get_template(&format!("{}.html", template.name))
                        .is_some(),
                    "{} html body missing",
                    template.name
                );
            }
            if template.text_template.is_some() {
                assert!(
                    engine
                        .get_template(&format!("{}.txt", template.name))
                        .is_some(),
                    "{} text body missing",
                    template.name
                );
            }
        }
        let rendered = engine
            .render(
                "welcome.html",
                &ferris_core::value!({ "app_name": "Ferris" }),
            )
            .expect("welcome.html renders");
        assert!(rendered.contains("Ferris"), "{rendered}");
    }

    #[test]
    fn substitution_is_single_pass() {
        let engine = TemplateEngine::new();
        let context = ferris_core::value!({
            "a": "{{ b }}",
            "b": "expanded",
        });
        let rendered = engine
            .render_str("x={{ a }} y={{ b }}", &context, false)
            .unwrap();
        assert_eq!(rendered, "x={{ b }} y=expanded");
    }

    #[test]
    fn unknown_placeholder_stays_visible() {
        let engine = TemplateEngine::new();
        assert_eq!(
            engine
                .render_str(
                    "Hello {{ missing }}!",
                    &Value::Object(Default::default()),
                    false
                )
                .unwrap(),
            "Hello {{ missing }}!"
        );
        assert_eq!(
            engine
                .render_str(
                    "unterminated {{ tail",
                    &Value::Object(Default::default()),
                    false
                )
                .unwrap(),
            "unterminated {{ tail"
        );
    }
}
