//! Unmatched-route fallback and the health probe.
//!
//! Both are part of every build. The signed-download handler lives with the
//! files module, which owns the route and the service it needs.

use axum::{
    Json,
    extract::Request,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};
/// Fallback for any unregistered route. JSON for API clients (`Accept:
/// application/json`, as the Desk's `fetch` calls send) and a small HTML page
/// for everyone else. Plain axum answers a 404 in `text/plain`, which is useless
/// to both a browser and the API client that expects `{ "error": … }` (SRV13).
pub(super) async fn not_found_fallback(req: Request) -> Response {
    let wants_json = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|values| {
            values
                .split(',')
                .any(|t| t.trim().starts_with("application/json"))
        })
        .unwrap_or(false);
    let path = req.uri().path();
    if wants_json {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "not_found",
                "message": format!("Route not found: {path}"),
            })),
        )
            .into_response();
    }
    let escaped = escape_html(path);
    (
        StatusCode::NOT_FOUND,
        Html(format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
             <title>404 · Not Found</title>\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <style>body{{font-family:system-ui,sans-serif;max-width:36rem;margin:4rem auto;padding:0 1rem;color:CanvasText;background:Canvas}}\
             h1{{font-size:1.6rem;margin-bottom:.5rem}}\
             code{{border:1px solid;padding:.15rem .4rem;border-radius:.25rem}}</style>\
             </head><body><h1>404 · Not Found</h1>\
             <p>The route <code>{escaped}</code> does not exist on this server.</p>\
             <p><a href=\"/\">Back to the home page</a></p></body></html>"
        )),
    )
        .into_response()
}

/// Escape the characters that could break out of the 404 page's text context.
/// The request path is attacker-controlled, so it is never interpolated raw.
fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

pub(super) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}
