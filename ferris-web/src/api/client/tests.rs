use super::*;
use ferris_web_shared::route::path_segment;

#[test]
fn plain_segments_are_left_alone() {
    assert_eq!(path_segment("Sales Invoice"), "Sales%20Invoice");
    assert_eq!(path_segment("a-b_c.d~e"), "a-b_c.d~e");
}

#[test]
fn multi_byte_names_round_trip() {
    // Encoded per byte, so the server decodes back to the same string.
    assert_eq!(path_segment("Facturación"), "Facturaci%C3%B3n");
    assert_eq!(path_segment("/"), "%2F");
    assert_eq!(path_segment("a?b#c"), "a%3Fb%23c");
}

#[test]
fn path_encodes_every_segment_but_not_the_prefix() {
    assert_eq!(
        path("/api/resource", &["Sales Invoice"]),
        "/api/resource/Sales%20Invoice"
    );
    assert_eq!(
        path("/api/resource", &["Sales Invoice", "SO-0001", "submit"]),
        "/api/resource/Sales%20Invoice/SO-0001/submit"
    );
}

/// The CSRF echo rules mirror the server's `require_csrf` gate exactly:
/// mutating verbs carry the token, safe methods never do.
#[test]
fn mutating_methods_carry_csrf() {
    assert!(is_mutating("POST"));
    assert!(is_mutating("PUT"));
    assert!(is_mutating("PATCH"));
    assert!(is_mutating("DELETE"));
    assert!(!is_mutating("GET"));
    assert!(!is_mutating("HEAD"));
    assert!(!is_mutating("OPTIONS"));
}

/// The stored token is refreshed (never evicted) by a response header.
#[test]
fn capture_refreshes_the_stored_token() {
    capture_csrf_token(Some("first"));
    assert_eq!(csrf_token().as_deref(), Some("first"));
    capture_csrf_token(Some("second"));
    assert_eq!(csrf_token().as_deref(), Some("second"));
    // An absent header leaves the current token untouched.
    capture_csrf_token(None);
    assert_eq!(csrf_token().as_deref(), Some("second"));
}
