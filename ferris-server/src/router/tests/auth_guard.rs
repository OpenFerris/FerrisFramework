use super::super::web_auth::{encode_query_value, is_core_desk_path, is_local_path};

/// `is_core_desk_path` must cover every concrete desk route and nothing
/// else, or the redirect would either send authenticated content away or
/// leave a desk deep link publicly readable. Plugin *dashboards* are added
/// to this by `is_dashboard_page`, which reads the live registry (empty in a
/// unit test); plugin *website* pages stay public.
#[test]
fn is_core_desk_path_covers_desk_routes_and_nothing_else() {
    for path in [
        "/",
        "/desk",
        "/desk/",
        "/desk/core/Task",
        "/desk/core/Task/ACME",
        "/report/sales",
        "/kanban/Task",
        "/calendar",
        "/settings",
        "/settings/profile",
    ] {
        assert!(is_core_desk_path(path), "{path} should require auth");
    }

    for path in [
        "/login",
        "/static/desk.css",
        "/api/auth/login",
        "/api/desk/layout",
        "/website",
        "/website/about",
        "/pages/landing",
        "/app/ops",
        "/favicon.ico",
    ] {
        assert!(!is_core_desk_path(path), "{path} should be public");
    }
}

/// Only same-origin absolute paths may be bounced back to; scheme- and
/// authority-relative targets are the open-redirect hole this closes.
#[test]
fn is_local_path_rejects_offsite_and_scheme_relative_targets() {
    assert!(is_local_path("/"));
    assert!(is_local_path("/desk/core/Task?filter=open"));
    assert!(!is_local_path(""));
    assert!(!is_local_path("//evil.example/steal"));
    assert!(!is_local_path("https://evil.example/steal"));
    assert!(!is_local_path("http://evil.example/steal"));
    assert!(!is_local_path("/\\evil.example/steal"));
    assert!(!is_local_path("/ok\nInjected: header"));
}

#[test]
fn encode_query_value_percent_encodes_reserved_bytes() {
    // Reserved delimiters (including `/`, `?`, `&`) are encoded, so a whole
    // path fits safely in one query value; a percent-decoding reader gets it
    // back verbatim.
    assert_eq!(
        encode_query_value("/desk/core/Task"),
        "%2Fdesk%2Fcore%2FTask"
    );
    assert_eq!(encode_query_value("/a b"), "%2Fa%20b");
    assert_eq!(encode_query_value("/a?b=c&d=e"), "%2Fa%3Fb%3Dc%26d%3De");
    assert_eq!(encode_query_value("/café"), "%2Fcaf%C3%A9");
}
