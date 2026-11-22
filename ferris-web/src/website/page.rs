//! Pages: the `Web Page` model, its in-memory registry and the HTML/sitemap
//! renderer.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::RwLock;

use ferris_core::{Error, Result, ValidationError, Value};
use ferris_web_shared::html::{escape_html, sanitize_html};

/// A CMS page.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebPage {
    pub route: String,
    pub title: String,
    /// Rendered HTML content (the mapping's MDX is compiled ahead of time).
    pub content: String,
    pub template: String,
    pub published: bool,
    pub meta_tags: HashMap<String, String>,
    pub script: Option<String>,
    pub style: Option<String>,
}

impl WebPage {
    pub fn new(
        route: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            route: normalize_route(&route.into()),
            title: title.into(),
            content: content.into(),
            template: "layout/main".to_string(),
            published: false,
            meta_tags: HashMap::new(),
            script: None,
            style: None,
        }
    }

    pub fn published(mut self) -> Self {
        self.published = true;
        self
    }

    pub fn with_template(mut self, template: impl Into<String>) -> Self {
        self.template = template.into();
        self
    }

    /// Build a page from an MDX/Markdown source with front matter.
    ///
    /// `title`, `template` and other front-matter keys become the page title,
    /// template and meta tags; the body is rendered to HTML.
    pub fn from_markdown(route: impl Into<String>, source: &str) -> Self {
        let rendered = crate::markdown::render_page(source);
        let mut page = WebPage::new(
            route,
            rendered.front_matter.title.clone().unwrap_or_default(),
            rendered.html,
        );
        if let Some(template) = rendered.front_matter.template {
            page.template = template;
        }
        page.meta_tags = rendered.front_matter.meta;
        page
    }

    pub fn with_meta(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.meta_tags.insert(name.into(), value.into());
        self
    }

    /// Build the public page a stored `Web Page` row describes.
    ///
    /// This is the only place the CMS row becomes a servable page, so the
    /// publication rule lives here and every caller inherits it: `None` for a
    /// draft (or a row without a route), which is what keeps a draft out of a
    /// direct load even when the caller read the row without a filter.
    ///
    /// `content_type = "Markdown"` renders the stored source with the project
    /// renderer; anything else is treated as HTML and passed through the shared
    /// allowlist ([`sanitize_html`]) before it can reach a response. Stored
    /// `script`/`css` columns are intentionally ignored: no allowlist makes
    /// arbitrary executable code safe, and a page cannot run site-wide code.
    pub fn from_value(value: &Value) -> Option<Self> {
        let route = value.get("route").and_then(Value::as_str)?.trim();
        if route.is_empty() || !stored_flag(value.get("published")) {
            return None;
        }
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(route);
        let source = value
            .get("main_section")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = if value.get("content_type").and_then(Value::as_str) == Some("Markdown") {
            crate::markdown::render(source)
        } else {
            sanitize_html(source)
        };
        let mut page = Self::new(route, title, content).published();
        if let Some(description) = value
            .get("meta_description")
            .and_then(Value::as_str)
            .filter(|description| !description.trim().is_empty())
        {
            page = page.with_meta("description", description);
        }
        Some(page)
    }
}

/// Whether a stored value is a truthy flag.
///
/// A `Check` column reaches Rust as a JSON bool, but a SQLite row that skipped
/// the engine's coercion arrives as `0`/`1`, and older rows may carry a string.
/// Reading all three here keeps "published" from silently reading as false (or,
/// worse, as true for the string `"0"`).
pub(super) fn stored_flag(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
        Some(Value::String(text)) => matches!(text.trim(), "1" | "true" | "True" | "yes"),
        _ => false,
    }
}

/// A fully rendered page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    pub title: String,
    pub html: String,
}

/// In-memory page registry and renderer.
#[derive(Default)]
pub struct WebsiteRouter {
    pages: RwLock<HashMap<String, WebPage>>,
}

impl WebsiteRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a page. The route is normalised to a leading slash.
    pub fn register(&self, page: WebPage) -> Result<()> {
        if page.route.trim().is_empty() {
            return Err(Error::Validation(ValidationError::Required(
                "page route".to_string(),
            )));
        }
        let route = normalize_route(&page.route);
        let normalized = WebPage {
            route: route.clone(),
            ..page
        };
        self.pages
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(route, normalized);
        Ok(())
    }

    /// Resolve a published page by route. Draft pages are never served.
    pub fn get(&self, route: &str) -> Option<WebPage> {
        let route = normalize_route(route);
        self.pages
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&route)
            .filter(|page| page.published)
            .cloned()
    }

    /// List published routes.
    pub fn published_routes(&self) -> Vec<String> {
        let mut routes: Vec<String> = self
            .pages
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|page| page.published)
            .map(|page| page.route.clone())
            .collect();
        routes.sort();
        routes
    }

    /// Render a page to a complete HTML document, or `None` when absent/unpublished.
    pub fn render(&self, route: &str) -> Option<RenderedPage> {
        let page = self.get(route)?;
        Some(self.render_page(&page))
    }

    /// Render a specific page.
    ///
    /// The body always goes through [`sanitize_html`]: a registry page can come
    /// from a stored document, and the allowlist is what makes "content is
    /// HTML" safe. `script`/`style` are part of the model's shape but are never
    /// emitted — a CMS row must not inject executable code into the public site
    /// (VIEW-22).
    pub fn render_page(&self, page: &WebPage) -> RenderedPage {
        let mut head = String::new();
        let _ = write!(head, "<title>{}</title>", escape_html(&page.title));
        for (name, value) in &page.meta_tags {
            let _ = write!(
                head,
                "<meta name=\"{}\" content=\"{}\">",
                escape_html(name),
                escape_html(value)
            );
        }

        let body = sanitize_html(&page.content);
        let html = format!("<!DOCTYPE html><html><head>{head}</head><body>{body}</body></html>");
        RenderedPage {
            title: page.title.clone(),
            html,
        }
    }

    /// Generate a `sitemap.xml` body for all published pages.
    pub fn sitemap(&self, base_url: &str) -> String {
        let base = base_url.trim_end_matches('/');
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
        for route in self.published_routes() {
            xml.push_str(&format!(
                "  <url><loc>{}{}</loc></url>\n",
                escape_xml(base),
                escape_xml(&route)
            ));
        }
        xml.push_str("</urlset>\n");
        xml
    }
}

/// Normalise a route to lowercase `/path` with no trailing slash (except root).
///
/// Lowercasing makes lookups case-insensitive, matching Frappe's website routing.
pub fn normalize_route(route: &str) -> String {
    let trimmed = route.trim().to_lowercase();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }
    let with_slash = if trimmed.starts_with('/') {
        trimmed
    } else {
        format!("/{trimmed}")
    };
    with_slash.trim_end_matches('/').to_string()
}

fn escape_xml(value: &str) -> String {
    escape_html(value).replace('\'', "&apos;")
}
