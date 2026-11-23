//! Web forms: the public field/form models, the submission validator and the
//! server-rendered form the host serves.
//!
//! There is no template engine: the markup is built here from the declared
//! `DocType` metadata, with every value escaped. That keeps the public surface a
//! first-class artifact of the form model instead of a client-only page, and it
//! makes the GET/POST pair testable without a browser.

use std::collections::HashMap;
use std::fmt::Write as _;

use ferris_core::i18n::Locale;
use ferris_core::{AuthzError, Error, I18n, Result, User, ValidationError, Value};
use ferris_db::Db;
use ferris_web_shared::html::escape_html;

use super::page::stored_flag;

/// A field on a public web form.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebFormField {
    pub fieldname: String,
    pub label: String,
    pub fieldtype: String,
    pub reqd: bool,
    /// Options of a `Select` field, one per line in the metadata.
    #[serde(default)]
    pub options: Vec<String>,
}

impl WebFormField {
    pub fn new(
        fieldname: impl Into<String>,
        label: impl Into<String>,
        fieldtype: impl Into<String>,
    ) -> Self {
        Self {
            fieldname: fieldname.into(),
            label: label.into(),
            fieldtype: fieldtype.into(),
            reqd: false,
            options: Vec::new(),
        }
    }

    #[must_use]
    pub const fn required(mut self) -> Self {
        self.reqd = true;
        self
    }

    #[must_use]
    pub fn with_options(mut self, options: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.options = options.into_iter().map(Into::into).collect();
        self
    }
}

/// A public form backed by a `DocType` (Frappe `Web Form`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebForm {
    pub name: String,
    pub doctype: String,
    pub fields: Vec<WebFormField>,
    pub success_message: String,
    pub require_login: bool,
    /// Route the form is published at, normalised (`/contact`).
    #[serde(default)]
    pub route: String,
    /// Store front language; empty resolves through English.
    #[serde(default)]
    pub locale: String,
}

/// Fieldtypes that only lay out a form/document and never hold a value.
const LAYOUT_FIELDTYPES: [&str; 9] = [
    "Section Break",
    "Column Break",
    "Tab Break",
    "Heading",
    "HTML",
    "Button",
    "Fold",
    "Table",
    "Table MultiSelect",
];

/// Upper bound on the inputs one public form renders.
///
/// The `Web Form` `DocType` carries no field list, so the form is derived from the
/// target `DocType`'s metadata; a document with sixty columns must not turn into
/// sixty public inputs. Twenty-four covers every ordinary form and keeps the
/// page — and the attack surface — bounded.
const MAX_FORM_FIELDS: usize = 24;

impl WebForm {
    pub fn new(
        name: impl Into<String>,
        doctype: impl Into<String>,
        fields: Vec<WebFormField>,
    ) -> Self {
        let name = name.into();
        Self {
            route: format!("/{}", name.to_lowercase().replace(' ', "-")),
            name,
            doctype: doctype.into(),
            fields,
            success_message: String::new(),
            require_login: false,
            locale: String::new(),
        }
    }

    /// The catalogue and the locale this form renders in.
    fn catalogue(&self) -> (I18n, Locale) {
        (
            I18n::default(),
            Locale::parse(&self.locale).unwrap_or_default(),
        )
    }

    /// Translate one of the form's own messages.
    fn tr(&self, key: &str) -> String {
        let (i18n, locale) = self.catalogue();
        i18n.translate(&locale, key).to_string()
    }

    /// Translate and interpolate one of the form's own messages.
    fn tr_with(&self, key: &str, args: &[(&str, &str)]) -> String {
        let (i18n, locale) = self.catalogue();
        i18n.translate_with(&locale, key, args)
    }

    /// Build the public form a stored `Web Form` row describes.
    ///
    /// `None` for a draft, a row without a route, or a row without a target
    /// `DocType`: a form must have a URL and somewhere to write, and neither can
    /// be invented. The publication rule lives here so the host cannot serve an
    /// unpublished form by reading the row without a filter.
    pub fn from_value(value: &Value) -> Option<Self> {
        let route = value.get("route").and_then(Value::as_str)?.trim();
        let doctype = value.get("doc_type").and_then(Value::as_str)?.trim();
        if route.is_empty() || doctype.is_empty() || !stored_flag(value.get("published")) {
            return None;
        }
        let name = value
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(route);
        let mut form = Self::new(name, doctype, Vec::new());
        form.route = super::normalize_route(route);
        form.require_login = stored_flag(value.get("login_required"));
        if let Some(message) = value
            .get("success_message")
            .and_then(Value::as_str)
            .filter(|message| !message.trim().is_empty())
        {
            form.success_message = message.to_string();
        }
        Some(form)
    }

    /// Populate the inputs from the target `DocType`'s editable fields.
    ///
    /// `Web Form` stores no field list, so the declared `DocType` is the single
    /// source of truth: hidden/read-only columns, layout breaks, child tables
    /// and engine-managed system fields never become public inputs. The cap
    /// ([`MAX_FORM_FIELDS`]) and the order (declaration order) are both part of
    /// the behaviour: a form must not grow a larger attack surface because a
    /// custom field was added.
    #[must_use]
    pub fn with_fields_from_meta(mut self, meta: &ferris_schema::DoctypeMeta) -> Self {
        self.fields = meta
            .fields
            .iter()
            .filter(|field| {
                !field.hidden
                    && !field.read_only
                    && !LAYOUT_FIELDTYPES.contains(&field.fieldtype.as_str())
                    && !is_system_field(&field.fieldname)
            })
            .take(MAX_FORM_FIELDS)
            .map(|field| {
                let options: Vec<String> = field
                    .options
                    .as_deref()
                    .map(|options| options.lines().map(str::trim).map(str::to_string))
                    .map(|lines| {
                        lines
                            .filter(|line| !line.is_empty())
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default();
                WebFormField {
                    fieldname: field.fieldname.clone(),
                    label: field
                        .label
                        .clone()
                        .filter(|label| !label.trim().is_empty())
                        .unwrap_or_else(|| field.fieldname.clone()),
                    fieldtype: field.fieldtype.clone(),
                    reqd: field.reqd,
                    options,
                }
            })
            .collect();
        self
    }

    /// Whether the caller may submit: a form that requires login refuses a
    /// sessionless request instead of writing as a guest.
    #[must_use]
    pub const fn can_submit(&self, user: Option<&User>) -> bool {
        !self.require_login || user.is_some()
    }

    /// Turn raw URL-encoded fields into the typed submission [`Self::validate`]
    /// expects. Unchecked checkboxes are absent from a form body, so a `Check`
    /// field is always materialised as a bool — that is what lets a required
    /// acceptance box fail instead of reading as "not sent, therefore ignored".
    #[must_use]
    pub fn submission_from_fields(&self, fields: &HashMap<String, String>) -> Value {
        let mut submission = ferris_core::Map::new();
        for field in &self.fields {
            let raw = fields.get(&field.fieldname).map(String::as_str);
            let value = match field.fieldtype.as_str() {
                "Check" => Value::Bool(raw.is_some_and(truthy_field)),
                "Int" => raw
                    .and_then(|raw| raw.trim().parse::<i64>().ok())
                    .map_or_else(|| string_or_null(raw), |value| Value::Number(value.into())),
                "Float" | "Currency" => raw
                    .and_then(|raw| raw.trim().parse::<f64>().ok())
                    .map_or_else(
                        || string_or_null(raw),
                        |value| {
                            serde_json::Number::from_f64(value)
                                .map_or_else(|| string_or_null(raw), Value::Number)
                        },
                    ),
                _ => string_or_null(raw),
            };
            submission.insert(field.fieldname.clone(), value);
        }
        Value::Object(submission)
    }

    /// Validate and clean a submission: required fields must be present,
    /// unknown fields are rejected, and values are coerced per field type.
    pub fn validate(&self, submission: &ferris_core::Value) -> Result<ferris_core::Value> {
        let ferris_core::Value::Object(input) = submission else {
            return Err(Error::Validation(ValidationError::Invalid(
                self.tr("components.website.form.submission_object"),
            )));
        };

        for key in input.keys() {
            if !self.fields.iter().any(|f| &f.fieldname == key) {
                return Err(Error::Validation(ValidationError::Invalid(self.tr_with(
                    "components.website.form.unknown_field",
                    &[("field", key)],
                ))));
            }
        }

        let mut cleaned = ferris_core::Map::new();
        for field in &self.fields {
            let value = input
                .get(&field.fieldname)
                .cloned()
                .unwrap_or(ferris_core::Value::Null);
            if value.is_null() || value.as_str() == Some("") {
                if field.reqd {
                    return Err(Error::Validation(ValidationError::Required(
                        field.label.clone(),
                    )));
                }
                continue;
            }

            match field.fieldtype.as_str() {
                "Email" => {
                    let text = value.as_str().unwrap_or_default();
                    if !ferris_document::is_valid_email(text) {
                        return Err(Error::Validation(ValidationError::InvalidValue {
                            field: field.label.clone(),
                            message: self.tr("components.website.form.invalid_email"),
                        }));
                    }
                }
                "Int" | "Float" | "Currency" => {
                    if value.as_f64().is_none() {
                        return Err(Error::Validation(ValidationError::InvalidValue {
                            field: field.label.clone(),
                            message: self.tr("components.website.form.invalid_number"),
                        }));
                    }
                }
                "Check" => {
                    let Some(checked) = value.as_bool() else {
                        return Err(Error::Validation(ValidationError::InvalidValue {
                            field: field.label.clone(),
                            message: self.tr("components.website.form.invalid_boolean"),
                        }));
                    };
                    if field.reqd && !checked {
                        return Err(Error::Validation(ValidationError::Required(
                            field.label.clone(),
                        )));
                    }
                }
                _ => {}
            }
            cleaned.insert(field.fieldname.clone(), value);
        }

        Ok(ferris_core::Value::Object(cleaned))
    }

    /// Create the document the form describes.
    ///
    /// A session caller goes through [`ferris_api::DocStore`] as that user, so
    /// the engine applies the same permissions a REST create would. A guest is
    /// only reachable when the form allows it, and writes through an
    /// `allow_anonymous` context: that is the explicit opt-in `Web Form`
    /// represents, not a relaxation of the REST path.
    ///
    /// # Errors
    ///
    /// A validation error for the submitted values, the engine's authorization
    /// error when the session caller may not create the target document, and
    /// its storage/metadata error while writing.
    #[allow(clippy::result_large_err)] // `Error` is the crate-wide type; boxing it would leak into every caller.
    pub async fn submit(
        &self,
        pool: &Db,
        user: Option<&User>,
        submission: &Value,
    ) -> Result<Value> {
        if !self.can_submit(user) {
            return Err(Error::Authz(AuthzError::PermissionDenied {
                action: "submit".to_string(),
                resource: self.name.clone(),
            }));
        }
        let cleaned = self.validate(submission)?;
        match user {
            Some(user) => {
                ferris_document::with_user(
                    Some(user.clone()),
                    ferris_api::DocStore::create(pool, &self.doctype, cleaned),
                )
                .await
            }
            None => create_as_guest(pool, &self.doctype, cleaned).await,
        }
    }

    /// The complete HTML document with the declared fields and labels.
    ///
    /// `csrf_token` is the session's token when there is one; a guest form has
    /// no ambient credential for CSRF to protect, so it omits the hidden field
    /// and the host checks the request origin instead.
    #[must_use]
    pub fn render_html(
        &self,
        action: &str,
        csrf_token: Option<&str>,
        error: Option<&str>,
    ) -> String {
        let mut body = String::new();
        let _ = write!(body, "<h1>{}</h1>", escape_html(&self.name));
        if let Some(error) = error {
            let _ = write!(
                body,
                "<p class=\"error\" role=\"alert\">{}</p>",
                escape_html(error)
            );
        }
        let _ = write!(
            body,
            "<form method=\"post\" action=\"{}\">",
            escape_html(action)
        );
        if let Some(token) = csrf_token {
            let _ = write!(
                body,
                "<input type=\"hidden\" name=\"_csrf\" value=\"{}\">",
                escape_html(token)
            );
        }
        // Honeypot: a hidden field no person fills. A filled one means a bot
        // and the submission is acknowledged without writing anything.
        body.push_str(
            "<div hidden aria-hidden=\"true\"><input type=\"text\" name=\"_website\" \
             tabindex=\"-1\" autocomplete=\"off\"></div>",
        );
        if self.fields.is_empty() {
            let message = self.tr("components.website.form.no_fields");
            let _ = write!(
                body,
                "<p class=\"error\" role=\"alert\">{}</p>",
                escape_html(&message)
            );
        }
        for field in &self.fields {
            body.push_str(&field_html(field));
        }
        let _ = write!(
            body,
            "<button type=\"submit\">{}</button></form>",
            escape_html(&self.tr("components.website.form.submit"))
        );
        self.html_document(&body)
    }

    /// What the caller sees after a successful submission.
    #[must_use]
    pub fn render_success(&self) -> String {
        let message = if self.success_message.trim().is_empty() {
            self.tr("components.website.form.success_default")
        } else {
            self.success_message.clone()
        };
        let body = format!(
            "<h1>{}</h1><p role=\"status\">{}</p>",
            escape_html(&self.name),
            escape_html(&message)
        );
        self.html_document(&body)
    }

    /// Wrap `body` in the standalone public form document.
    fn html_document(&self, body: &str) -> String {
        let lang = Locale::parse(&self.locale)
            .unwrap_or_default()
            .as_str()
            .to_string();
        html_document(&self.name, &lang, body)
    }
}

/// Markup for one declared field.
fn field_html(field: &WebFormField) -> String {
    let name = escape_html(&field.fieldname);
    let label = escape_html(&field.label);
    let required = if field.reqd { " required" } else { "" };
    let control = match field.fieldtype.as_str() {
        "Text" | "Small Text" | "Long Text" | "Text Editor" | "Code" => {
            format!("<textarea id=\"{name}\" name=\"{name}\"{required}></textarea>")
        }
        "Select" => {
            let mut options = String::new();
            for option in &field.options {
                let option = escape_html(option);
                let _ = write!(options, "<option value=\"{option}\">{option}</option>");
            }
            format!("<select id=\"{name}\" name=\"{name}\"{required}>{options}</select>")
        }
        "Check" => {
            format!("<input type=\"checkbox\" id=\"{name}\" name=\"{name}\" value=\"1\"{required}>")
        }
        other => {
            let kind = match other {
                "Email" => "email",
                "Int" | "Float" | "Currency" => "number",
                "Date" => "date",
                "Datetime" => "datetime-local",
                "Password" => "password",
                "Time" => "time",
                _ => "text",
            };
            let step = if matches!(other, "Float" | "Currency") {
                " step=\"any\""
            } else {
                ""
            };
            format!("<input type=\"{kind}\" id=\"{name}\" name=\"{name}\"{required}{step}>")
        }
    };
    format!("<p><label for=\"{name}\">{label}</label>{control}</p>")
}

/// A standalone document for the public form. System colours, like the 404
/// page: the site has no stylesheet at this layer.
fn html_document(title: &str, lang: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{}</title>\
         <style>body{{font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;\
         padding:0 1rem;color:CanvasText;background:Canvas}}label{{display:block;\
         margin-bottom:.25rem}}input,select,textarea{{width:100%;padding:.4rem;\
         margin-bottom:.75rem}}.error{{color:FireBrick}}</style></head>\
         <body>{body}</body></html>",
        escape_html(lang),
        escape_html(title)
    )
}

/// A missing field becomes `Null`; present text stays a string, which is what
/// lets `validate` report a bad number as "must be a number".
fn string_or_null(raw: Option<&str>) -> Value {
    raw.map_or(Value::Null, |raw| Value::String(raw.to_string()))
}

/// The values a browser sends for a checked box.
fn truthy_field(raw: &str) -> bool {
    matches!(raw.trim(), "1" | "on" | "true" | "yes")
}

/// Fieldnames the engine owns; a public form never collects them.
fn is_system_field(fieldname: &str) -> bool {
    fieldname == "name"
        || fieldname == "docstatus"
        || ferris_document::SYSTEM_FIELDS.contains(&fieldname)
}

/// Insert through the engine with the explicit anonymous opt-in a public form
/// carries. No ambient user is set, so the context says exactly who is writing.
#[allow(clippy::result_large_err)] // `Error` is the crate-wide type; boxing it would leak into every caller.
async fn create_as_guest(pool: &Db, doctype: &str, data: Value) -> Result<Value> {
    let meta = ferris_api::effective_meta(pool, doctype).await?;
    let series = meta
        .as_ref()
        .map(|meta| meta.naming_series.clone())
        .unwrap_or_default();
    let mut context = ferris_document::EngineContext::new(doctype, series).allow_anonymous(true);
    if let Some(meta) = &meta {
        context = context.with_meta(meta);
    }
    ferris_document::DocumentEngine::new(pool.clone())
        .create(&context, data)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(require_login: bool) -> WebForm {
        let mut form = WebForm::new(
            "Contact",
            "Lead",
            vec![
                WebFormField::new("email", "Email", "Email").required(),
                WebFormField::new("message", "Message", "Data").required(),
                WebFormField::new("age", "Age", "Int"),
                WebFormField::new("accept", "Accept terms", "Check").required(),
            ],
        );
        form.require_login = require_login;
        form
    }

    #[test]
    fn textfields_and_labels_render_escaped() {
        let html = form(false).render_html("/contact", Some("token-1"), None);
        assert!(html.contains("<label for=\"email\">Email</label>"));
        assert!(html.contains("type=\"email\""));
        assert!(html.contains("name=\"_csrf\" value=\"token-1\""));
        assert!(html.contains("name=\"_website\""));
        assert!(html.contains("required"));
    }

    #[test]
    fn a_honeypot_and_csrf_field_are_not_submission_fields() {
        let form = form(false);
        let mut fields = HashMap::from([
            ("email".to_string(), "a@example.com".to_string()),
            ("message".to_string(), "hi".to_string()),
            ("_website".to_string(), "bot".to_string()),
        ]);
        fields.insert("accept".to_string(), "on".to_string());
        let submission = form.submission_from_fields(&fields);
        assert_eq!(
            submission.get("email").and_then(Value::as_str),
            Some("a@example.com")
        );
        assert!(submission.get("_website").is_none());
        assert!(form.validate(&submission).is_ok());
    }

    #[test]
    fn an_unchecked_required_checkbox_fails_validation() {
        let form = form(false);
        let fields = HashMap::from([
            ("email".to_string(), "a@example.com".to_string()),
            ("message".to_string(), "hi".to_string()),
        ]);
        let submission = form.submission_from_fields(&fields);
        assert!(form.validate(&submission).is_err());
    }

    #[test]
    fn a_login_only_form_refuses_a_guest() {
        assert!(!form(true).can_submit(None));
        let user = User {
            id: "ada@example.com".to_string(),
            email: "ada@example.com".to_string(),
            ..Default::default()
        };
        assert!(form(true).can_submit(Some(&user)));
        assert!(form(false).can_submit(None));
    }

    #[test]
    fn a_draft_or_routeless_form_is_not_public() {
        let row = ferris_core::value!({
            "route": "/contact",
            "doc_type": "Lead",
            "published": false,
        });
        assert!(WebForm::from_value(&row).is_none());
        let published = ferris_core::value!({
            "route": "/Contact/",
            "doc_type": "Lead",
            "published": true,
            "login_required": false,
        });
        let form = WebForm::from_value(&published).expect("published form resolves");
        assert_eq!(form.route, "/contact");
        assert!(!form.require_login);
    }
}
