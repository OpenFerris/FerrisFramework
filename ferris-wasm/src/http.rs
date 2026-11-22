//! The plugin → host HTTP response boundary (P1, WIT 2.0.0).
//!
//! [`Plugin::handle_request_as`] now returns a typed `http.response` instead
//! of a bare `String`; these helpers decide which of its headers reach the
//! wire. Both hosts (`ferris-server`'s router and `ferris-web`'s SSR twin)
//! share this policy so a header one drops cannot slip through the other.

use reqwest::header::{HeaderName, HeaderValue};
use tracing::warn;

/// Filters a plugin's response headers before they reach the wire.
///
/// Drops the hop-by-hop set (RFC 9110 §7.6.1) plus the two the framework
/// owns — `Content-Length` (computed from the body the host actually sends)
/// and `Set-Cookie` (session cookies belong to the framework, not to a
/// plugin).
///
/// Entries whose name or value is not valid header syntax are dropped as
/// well: one malformed pair from a guest must not fail the whole response.
///
/// Returns the surviving headers with lowercased names, ready to test for
/// `content-type` and to feed an `http` response builder.
#[must_use]
pub fn sanitize_response_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    const FORBIDDEN: [&str; 10] = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "content-length",
        "set-cookie",
    ];
    headers
        .iter()
        .filter_map(|(raw_name, value)| {
            let name = raw_name.to_ascii_lowercase();
            if FORBIDDEN.contains(&name.as_str()) {
                warn!(header = %name, "dropping forbidden response header");
                return None;
            }
            if HeaderName::from_bytes(name.as_bytes()).is_err() {
                warn!(header = %raw_name, "dropping invalid response header name");
                return None;
            }
            if HeaderValue::from_bytes(value.as_bytes()).is_err() {
                warn!(header = %name, "dropping invalid response header value");
                return None;
            }
            Some((name, value.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::sanitize_response_headers;

    fn pairs(headers: &[(&str, &str)]) -> Vec<(String, String)> {
        headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Hop-by-hop (RFC 9110 §7.6.1) and the framework-owned pair never reach
    /// the wire, no matter how the guest cases them.
    #[test]
    fn forbidden_headers_are_dropped() {
        let kept = sanitize_response_headers(&pairs(&[
            ("Connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("proxy-authenticate", "Basic"),
            ("proxy-authorization", "Basic Zm9v"),
            ("te", "trailers"),
            ("trailer", "x-checksum"),
            ("transfer-encoding", "chunked"),
            ("Upgrade", "h2c"),
            ("Content-Length", "999"),
            ("SET-COOKIE", "sid=1"),
            ("x-allowed", "yes"),
        ]));
        assert_eq!(kept, vec![("x-allowed".to_string(), "yes".to_string())]);
    }

    /// Names are lowercased once, so `content-type` detection downstream is
    /// a plain equality check; valid custom headers survive verbatim.
    #[test]
    fn names_are_normalized_and_valid_headers_survive() {
        let kept = sanitize_response_headers(&pairs(&[
            ("Content-Type", "application/json"),
            ("X-Custom", "value"),
        ]));
        assert_eq!(
            kept,
            vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("x-custom".to_string(), "value".to_string()),
            ]
        );
    }

    /// Malformed names/values are skipped (one bad pair must not fail the
    /// whole response builder).
    #[test]
    fn invalid_header_syntax_is_dropped() {
        let kept = sanitize_response_headers(&pairs(&[
            ("bad header", "value"),
            ("x-good", "value"),
            ("x-bad-value", "line\nbreak"),
        ]));
        assert_eq!(kept, vec![("x-good".to_string(), "value".to_string())]);
    }
}
