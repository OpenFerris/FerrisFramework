//! MIME parsing for one fetched message.
//!
//! `mail-parser` does the RFC 5322/2047/2231 work (encoded words, charsets,
//! multipart, transfer encodings); this module only maps its views onto the
//! `Inbound Email` shape and applies the limits that keep a hostile message
//! from turning into a multi-megabyte Desk row.
//!
//! The HTML part is sanitised at this boundary (`sanitize_html`) because the
//! Desk renders that field as markup: a sender must not be able to run script
//! in the reader's session (MAIL-02/SEC-17). The sanitizer is a small
//! allow-list, not a full HTML parser — see its documentation for what it does
//! and does not promise. The raw RFC 822 source is stored untouched, so
//! forwarding and evidence keep the original bytes.

use chrono::Utc;
use mail_parser::{AddressList, MessageParser};

use super::{InboundAttachment, InboundEmail, InboundError};

/// Attachments listed per message. A message with thousands of parts is
/// pathological; the document records the first ones and the raw source keeps
/// the rest.
const MAX_ATTACHMENTS: usize = 100;
/// Identifiers kept from `References`.
const MAX_REFERENCES: usize = 50;

/// Elements removed together with their content. None of them has a place in a
/// mail body, and each can execute or embed something the reader did not ask
/// for.
const CONTAINER_ELEMENTS: &[&str] = &["script", "iframe", "object", "applet"];

/// Void elements removed without looking for a closing tag (looking would
/// swallow the rest of the document).
const VOID_ELEMENTS: &[&str] = &["base", "meta", "link", "embed"];

/// Parse `raw` into an [`InboundEmail`].
///
/// # Errors
///
/// [`InboundError::Malformed`] when the parser rejects the bytes (no header
/// block at all). A message that parses but is missing fields is valid: mail in
/// the wild is.
pub fn parse(
    account: String,
    mailbox: String,
    uid: u32,
    raw: &[u8],
) -> Result<InboundEmail, InboundError> {
    let message = MessageParser::new().parse(raw).ok_or_else(|| {
        InboundError::Malformed(format!(
            "message {uid} in {mailbox} has no parseable header block"
        ))
    })?;

    let attachments = message
        .attachments()
        .take(MAX_ATTACHMENTS)
        .map(|part| InboundAttachment {
            name: part.attachment_name().unwrap_or("attachment").to_string(),
            content_type: part.content_type().map_or_else(
                || "application/octet-stream".to_string(),
                |content_type| {
                    content_type.subtype().map_or_else(
                        || content_type.ctype().to_string(),
                        |subtype| format!("{}/{subtype}", content_type.ctype()),
                    )
                },
            ),
            size: part.decoded_len(),
        })
        .collect();

    let references = message
        .references()
        .map(|list| {
            list.iter()
                .take(MAX_REFERENCES)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Ok(InboundEmail {
        account,
        uid,
        mailbox,
        message_id: message.message_id().map(str::to_string),
        in_reply_to: message
            .in_reply_to()
            .and_then(|list| list.first())
            .map(str::to_string),
        references,
        from: address_list(message.from()),
        to: address_list(message.to()),
        cc: address_list(message.cc()),
        subject: message.subject().unwrap_or_default().to_string(),
        text: message
            .body_text(0)
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default(),
        html: sanitize_html(
            &message
                .body_html(0)
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default(),
        ),
        raw: raw.to_vec(),
        attachments,
        received_at: Utc::now(),
        notify_user: None,
    })
}

/// `Name <address>, …`; groups are flattened by `mailboxes()`.
fn address_list(list: Option<AddressList<'_>>) -> String {
    let Some(list) = list else {
        return String::new();
    };
    list.mailboxes()
        .filter_map(|mailbox| {
            let address = mailbox.address()?;
            Some(match mailbox.name() {
                Some(name) if !name.trim().is_empty() => format!("{name} <{address}>"),
                _ => address.to_string(),
            })
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Strip the parts of a mail's HTML that can execute in the Desk.
///
/// This is a **minimal allow-list**, not a full sanitizer: it removes
/// `script`/`iframe`/`object`/`applet` (with their content), `base`/`meta`/
/// `link`/`embed`, every `on*` event-handler attribute, `srcdoc`/`formaction`
/// and `javascript:`/`vbscript:`/`data:text/html` URLs. Everything else — the
/// tables, images, links and inline styles a newsletter uses — is kept so a
/// forwarded mail still reads correctly.
///
/// What it does not promise: entity-encoded URLs (`jav&#x61;script:`), CSS
/// `url()` tricks and markup a browser repairs differently from this scanner
/// can still get through. The stored field therefore remains untrusted HTML;
/// the reader should still isolate it (CSP / sandboxed frame). The raw source
/// is preserved separately, so this never loses evidence.
fn sanitize_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let tail = &rest[lt..];
        if let Some(comment) = tail.strip_prefix("<!--") {
            match comment.find("-->") {
                Some(close) => rest = &comment[close + 3..],
                None => rest = "",
            }
            continue;
        }
        let after = &tail[1..];
        let looks_like_tag = after.starts_with(['!', '?', '/'])
            || after
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic());
        if !looks_like_tag {
            // `a < b` is text, not a tag.
            out.push('<');
            rest = after;
            continue;
        }
        let Some(end) = tag_end(tail) else {
            // An unterminated tag is dropped: a browser may still interpret it.
            rest = "";
            break;
        };
        let inner = &tail[1..end];
        rest = &tail[end + 1..];

        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
        }
        let closing = inner.starts_with('/');
        let body = inner.trim_start_matches('/');
        let name: String = body
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':'))
            .collect::<String>()
            .to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if CONTAINER_ELEMENTS.contains(&name.as_str()) {
            if !closing {
                rest = skip_element(rest, &name);
            }
            continue;
        }
        if VOID_ELEMENTS.contains(&name.as_str()) {
            continue;
        }
        let name_len = body.len()
            - body
                .trim_start_matches(|c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | ':'))
                .len();
        out.push('<');
        if closing {
            out.push('/');
        }
        out.push_str(&body[..name_len]);
        out.push_str(&filter_attributes(&body[name_len..]));
        out.push('>');
    }
    out.push_str(rest);
    out
}

/// Index of the `>` that closes the tag starting at `tail` (index 0 is `<`),
/// ignoring `>` inside a quoted attribute value.
fn tag_end(tail: &str) -> Option<usize> {
    let mut quote = None;
    for (index, c) in tail.char_indices() {
        match c {
            '"' | '\'' => match quote {
                Some(open) if open == c => quote = None,
                None => quote = Some(c),
                Some(_) => {}
            },
            '>' if quote.is_none() => return Some(index),
            _ => {}
        }
    }
    None
}

/// Skip the content and closing tag of `name`, returning what follows.
fn skip_element<'a>(haystack: &'a str, name: &str) -> &'a str {
    let needle = format!("</{name}");
    let Some(start) = haystack.to_ascii_lowercase().find(&needle) else {
        return "";
    };
    match haystack[start..].find('>') {
        Some(end) => &haystack[start + end + 1..],
        None => "",
    }
}

/// Rebuild a tag's attributes minus the dangerous ones.
fn filter_attributes(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::new();
    let mut i = 0_usize;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'/' {
            i += 1;
            continue;
        }
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let name = &raw[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = None;
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
                let quote = bytes[i];
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = Some(&raw[start..i]);
                if i < bytes.len() {
                    i += 1;
                }
            } else {
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = Some(&raw[start..i]);
            }
        }
        let lower = name.to_ascii_lowercase();
        if name.is_empty() || lower.starts_with("on") || lower == "srcdoc" || lower == "formaction"
        {
            continue;
        }
        if let Some(value) = value
            && is_dangerous_url(value)
        {
            continue;
        }
        out.push(' ');
        out.push_str(name);
        if let Some(value) = value {
            out.push_str("=\"");
            out.push_str(&value.replace('"', "&quot;"));
            out.push('"');
        }
    }
    out
}

/// A URL an attribute may not carry, ignoring the whitespace/control characters
/// a browser would strip before resolving it.
fn is_dangerous_url(value: &str) -> bool {
    let normalized: String = value
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized.starts_with("javascript:")
        || normalized.starts_with("vbscript:")
        || normalized.starts_with("data:text/html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_event_handlers_and_javascript_urls_are_removed() {
        let dirty = "<p onclick=\"steal()\">Hi \
                     <script>alert(1)</script>\
                     <img src=\"x\" onerror=\"alert(2)\">\
                     <a href=\"javascript:alert(3)\">bad</a>\
                     <a href=\"https://example.com\">good</a></p>";
        let clean = sanitize_html(dirty);
        let lower = clean.to_ascii_lowercase();

        assert!(!lower.contains("<script"), "{clean}");
        assert!(!lower.contains("alert(1)"), "{clean}");
        assert!(!lower.contains("onerror"), "{clean}");
        assert!(!lower.contains("onclick"), "{clean}");
        assert!(!lower.contains("javascript:"), "{clean}");
        assert!(clean.contains("https://example.com"), "{clean}");
        assert!(clean.contains("Hi "), "{clean}");
    }

    #[test]
    fn ordinary_markup_survives() {
        let html = "<table style=\"width:100%\"><tr><td>Total</td></tr></table>\
                    <a href=\"https://ferris.dev/x?y=1\">link</a>";
        let clean = sanitize_html(html);
        assert!(clean.contains("<table style=\"width:100%\">"), "{clean}");
        assert!(clean.contains("<td>Total</td>"), "{clean}");
        assert!(
            clean.contains("href=\"https://ferris.dev/x?y=1\""),
            "{clean}"
        );
    }

    #[test]
    fn an_unclosed_container_does_not_leak_its_body() {
        let clean = sanitize_html("<p>before</p><iframe src=\"x\">payload");
        assert!(clean.contains("before"), "{clean}");
        assert!(!clean.contains("payload"), "{clean}");
    }

    #[test]
    fn text_that_merely_contains_an_angle_bracket_is_kept() {
        assert_eq!(sanitize_html("a < b and 3 < 4"), "a < b and 3 < 4");
    }

    #[test]
    fn a_parsed_message_stores_sanitised_html_and_the_original_raw_source() {
        let raw = b"From: ann@example.com\r\nSubject: hi\r\n\
Content-Type: text/html\r\n\r\n\
<p onmouseover=\"x()\">hello</p><script>alert(1)</script>";
        let email = parse("soporte".to_string(), "INBOX".to_string(), 1, raw).expect("parsed");
        assert!(email.html.contains("hello"), "{}", email.html);
        assert!(!email.html.contains("onmouseover"), "{}", email.html);
        assert!(!email.html.contains("alert(1)"), "{}", email.html);
        assert!(
            String::from_utf8_lossy(&email.raw).contains("alert(1)"),
            "the raw source is untouched for forwarding/evidence"
        );
    }
}
