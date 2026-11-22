//! Markdown renderer

use crate::markdown::front_matter::{FrontMatter, parse_front_matter};
use ferris_web_shared::html::escape_html;

/// A rendered document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMarkdown {
    pub front_matter: FrontMatter,
    pub html: String,
}

/// Parse front matter and render the body to HTML.
pub fn render_page(source: &str) -> RenderedMarkdown {
    let (front_matter, body) = parse_front_matter(source);
    RenderedMarkdown {
        front_matter,
        html: render(&body),
    }
}

/// Render a Markdown subset to HTML.
pub fn render(source: &str) -> String {
    let mut html = String::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Fenced code block.
        if let Some(lang) = trimmed.strip_prefix("```") {
            let lang = lang.trim();
            let mut code = String::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != "```" {
                code.push_str(lines[i]);
                code.push('\n');
                i += 1;
            }
            i += 1; // consume closing fence (or EOF)
            let lang_attr = if lang.is_empty() {
                String::new()
            } else {
                format!(" class=\"language-{}\"", escape_html(lang))
            };
            html.push_str(&format!(
                "<pre><code{lang_attr}>{}</code></pre>\n",
                escape_html(&code)
            ));
            continue;
        }

        // Headings.
        if let Some(heading) = heading(line) {
            let (level, text) = heading;
            html.push_str(&format!("<h{level}>{}</h{level}>\n", inline(text)));
            i += 1;
            continue;
        }

        // Blockquote.
        if let Some(quote) = trimmed.strip_prefix('>') {
            let mut text = quote.trim().to_string();
            i += 1;
            while i < lines.len() && lines[i].trim().starts_with('>') {
                text.push(' ');
                text.push_str(lines[i].trim().trim_start_matches('>').trim());
                i += 1;
            }
            html.push_str(&format!("<blockquote>{}</blockquote>\n", inline(&text)));
            continue;
        }

        // Lists.
        if is_list_item(trimmed) {
            let ordered = is_ordered_item(trimmed);
            let tag = if ordered { "ol" } else { "ul" };
            html.push_str(&format!("<{tag}>\n"));
            while i < lines.len() && is_list_item(lines[i].trim()) {
                html.push_str(&format!(
                    "<li>{}</li>\n",
                    inline(strip_list_marker(lines[i].trim()))
                ));
                i += 1;
            }
            html.push_str(&format!("</{tag}>\n"));
            continue;
        }

        // Paragraph: join consecutive plain lines.
        let mut paragraph = String::new();
        while i < lines.len() {
            let current = lines[i].trim();
            if current.is_empty()
                || current.starts_with("```")
                || heading(lines[i]).is_some()
                || current.starts_with('>')
                || is_list_item(current)
            {
                break;
            }
            if !paragraph.is_empty() {
                paragraph.push(' ');
            }
            paragraph.push_str(current);
            i += 1;
        }
        html.push_str(&format!("<p>{}</p>\n", inline(&paragraph)));
    }

    html
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
        Some((hashes, trimmed[hashes..].trim()))
    } else {
        None
    }
}

fn is_list_item(line: &str) -> bool {
    line.starts_with("- ") || line.starts_with("* ") || is_ordered_item(line)
}

fn is_ordered_item(line: &str) -> bool {
    let digits: String = line.chars().take_while(|c| c.is_ascii_digit()).collect();
    !digits.is_empty() && line[digits.len()..].starts_with(". ")
}

fn strip_list_marker(line: &str) -> &str {
    if is_ordered_item(line) {
        let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
        line[digits + 2..].trim()
    } else {
        line[2..].trim()
    }
}

/// Render inline Markdown (emphasis, code, links) into HTML.
fn inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Inline code.
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '`') {
                let code: String = chars[i + 1..i + 1 + end].iter().collect();
                out.push_str(&format!("<code>{}</code>", escape_html(&code)));
                i += end + 2;
                continue;
            }
        }

        // Bold (** or __).
        if i + 1 < chars.len() && matches!(chars[i], '*' | '_') && chars[i + 1] == chars[i] {
            let marker = chars[i];
            if let Some(end) = find_double(&chars, i + 2, marker) {
                let content: String = chars[i + 2..end].iter().collect();
                out.push_str(&format!("<strong>{}</strong>", inline(&content)));
                i = end + 2;
                continue;
            }
        }

        // Italic (* or _).
        if matches!(chars[i], '*' | '_') {
            let marker = chars[i];
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == marker) {
                let end = i + 1 + end;
                if end > i + 1 {
                    let content: String = chars[i + 1..end].iter().collect();
                    out.push_str(&format!("<em>{}</em>", inline(&content)));
                    i = end + 1;
                    continue;
                }
            }
        }

        // Link [text](url).
        if chars[i] == '[' {
            if let Some(close) = chars[i + 1..].iter().position(|c| *c == ']') {
                let close = i + 1 + close;
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(paren) = chars[close + 2..].iter().position(|c| *c == ')') {
                        let text: String = chars[i + 1..close].iter().collect();
                        let url: String = chars[close + 2..close + 2 + paren].iter().collect();
                        if is_safe_href(&url) {
                            out.push_str(&format!(
                                "<a href=\"{}\">{}</a>",
                                escape_html(&url),
                                inline(&text)
                            ));
                        } else {
                            // The URL uses a scheme outside the allow-list
                            // (`javascript:`, `data:`, …). Emitting the anchor
                            // would be an XSS sink, so keep only the text.
                            out.push_str(&inline(&text));
                        }
                        i = close + 2 + paren + 1;
                        continue;
                    }
                }
            }
        }

        out.push_str(&escape_html_char(chars[i]));
        i += 1;
    }

    out
}

fn find_double(chars: &[char], from: usize, marker: char) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == marker && chars[i + 1] == marker {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Whether `href` may be emitted into the DOM. Absolute URLs must use one of
/// the allow-listed schemes (`http`, `https`, `mailto`); anything without a
/// scheme is treated as a relative URL and passes. Everything else
/// (`javascript:`, `data:`, `vbscript:`, entity-obfuscated schemes, …) is
/// rejected so markdown can never hand the browser an executable URL.
///
/// The check runs on the raw URL, *before* `escape_html`, so `&#106;avascript:`
/// tricks are caught too: the entity text is part of the scheme test.
fn is_safe_href(url: &str) -> bool {
    match url.split_once(':') {
        None => true,
        Some((scheme, _)) => {
            matches!(
                scheme.trim_start().to_ascii_lowercase().as_str(),
                "http" | "https" | "mailto"
            )
        }
    }
}

fn escape_html_char(c: char) -> String {
    match c {
        '&' => "&amp;".to_string(),
        '<' => "&lt;".to_string(),
        '>' => "&gt;".to_string(),
        '"' => "&quot;".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_blocks_and_inline() {
        let html = render("# Title\n\nSome **bold** and *italic* and `code`.\n\n- one\n- two\n");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
        assert!(html.contains("<code>code</code>"));
        assert!(html.contains("<ul>\n<li>one</li>\n<li>two</li>\n</ul>"));
    }

    #[test]
    fn test_render_links_and_code_fence_escape() {
        let html = render("See [docs](https://example.com/x).\n\n```rust\nlet a = 1 < 2;\n```\n");
        assert!(html.contains("<a href=\"https://example.com/x\">docs</a>"));
        assert!(html.contains("class=\"language-rust\""));
        assert!(html.contains("1 &lt; 2;"));
    }

    #[test]
    fn test_render_escapes_raw_html() {
        let html = render("<script>alert(1)</script>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_render_drops_unsafe_url_schemes_from_links() {
        let html = render("[clic](javascript:alert(document.cookie))");
        assert!(!html.contains("<a "), "{html}");
        assert!(!html.to_lowercase().contains("javascript"), "{html}");
        assert!(html.contains("clic"), "link text must stay: {html}");

        for unsafe_url in [
            "JAVASCRIPT:alert(1)",
            " javas\\tcript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
            "&#106;avascript:alert(1)",
        ] {
            let html = render(&format!("[x]({unsafe_url})"));
            assert!(!html.contains("<a "), "must not emit {unsafe_url}: {html}");
        }
    }

    #[test]
    fn test_render_keeps_allow_listed_and_relative_urls() {
        for safe in [
            "https://example.com/x?a=1&b=2",
            "http://example.com",
            "mailto:team@example.com",
            "/wiki/Foo",
            "./local.html",
            "../up.md",
            "#anchor",
            "plain/path",
        ] {
            let html = render(&format!("[t]({safe})"));
            assert!(
                html.contains("<a href="),
                "expected a link for {safe}: {html}"
            );
        }
    }

    #[test]
    fn test_render_page_combines_front_matter() {
        let page = render_page("---\ntitle: Home\n---\n## Hi\n");
        assert_eq!(page.front_matter.title.as_deref(), Some("Home"));
        assert!(page.html.contains("<h2>Hi</h2>"));
    }
}
