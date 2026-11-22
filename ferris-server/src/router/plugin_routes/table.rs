//! Validation of the declared routes: paths, methods, collisions and the auth
//! each one inherits.
//!
//! The table is built from the descriptor alone — no socket, no guest call — so
//! a declaration that cannot be served is refused before the router is touched.
//! That is what keeps `axum` from panicking at boot on an overlapping route: the
//! overlap is detected here, logged, and the second route simply does not exist.

use std::collections::BTreeMap;
use std::fmt;

use ferris_wasm::descriptor::{NetworkSpec, RouteKind, RouteSpec};
use ferris_wasm::{ListenerTransport, RouteAuth, StreamListenerSpec};

/// Path prefixes the framework owns. A declared route whose first segment is one
/// of these is refused: `/api` and `/plugins` are the contract with the Desk, and
/// a plugin asking for them is asking to impersonate the framework.
///
/// The list covers every static first segment `build_router` mounts **and** every
/// literal first segment the Desk's SSR table declares. Missing one is not a
/// cosmetic gap: a declared route that overlaps an existing SSR path makes
/// `Router::merge` panic at boot (SRV-001). The
/// `every_literal_desk_route_is_a_framework_prefix` test below reads the real
/// route table so this list cannot silently drift again.
pub(super) const FRAMEWORK_PREFIXES: &[&str] = &[
    "api",
    "app",
    "assets",
    "calendar",
    "desk",
    "events",
    "favicon.ico",
    "files",
    "gantt",
    "health",
    "kanban",
    "login",
    "logout",
    "oauth",
    "pkg",
    "plugins",
    "report",
    "settings",
    "socket.io",
    "static",
    "tree",
];

/// Why one declared route was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RouteConflict {
    /// The path is not a literal URL path the router can mount.
    Invalid { path: String, reason: String },
    /// The first segment belongs to the framework.
    FrameworkPrefix { path: String },
    /// The path claims the whole root namespace.
    RootClaim,
    /// Another route (same app or another) already owns this method + path.
    Duplicate {
        pattern: String,
        method: String,
        other: String,
    },
}

impl fmt::Display for RouteConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { path, reason } => {
                write!(f, "route path `{path}` is not mountable: {reason}")
            }
            Self::FrameworkPrefix { path } => {
                write!(f, "route path `{path}` uses a framework-owned prefix")
            }
            Self::RootClaim => f.write_str(
                "route path `/` claims the whole namespace and would shadow the framework",
            ),
            Self::Duplicate {
                pattern,
                method,
                other,
            } => write!(f, "`{method} {pattern}` is already declared by `{other}`"),
        }
    }
}

/// One route the router mounts: the declared spec plus the app it belongs to and
/// the auth the host enforces (the declaration's, or the app default).
#[derive(Debug, Clone)]
pub(super) struct MountedRoute {
    pub(super) app: String,
    pub(super) id: String,
    /// Upper-case method, or `*`. Ignored by a stream route, which only upgrades
    /// a `GET`.
    pub(super) method: String,
    /// The declared prefix, normalized: leading `/`, no trailing `/`.
    pub(super) path: String,
    pub(super) auth: RouteAuth,
    pub(super) body_limit: usize,
    /// Whether this is a `%stream` route (an HTTP upgrade) instead of a
    /// request/response one.
    pub(super) stream: bool,
    /// The declared listener of a stream route; `None` for an HTTP route.
    pub(super) listener: Option<StreamListenerSpec>,
}

impl MountedRoute {
    /// The axum patterns that serve this route.
    ///
    /// An HTTP route's path is a prefix: `/x` claims `/x` and `/x/…`. A stream
    /// route claims exactly its path — the frames carry no path, so a subtree
    /// would accept traffic the app never declared.
    pub(super) fn patterns(&self) -> Vec<String> {
        if self.stream {
            return vec![self.path.clone()];
        }
        if self.path == "/" {
            vec!["/".to_string()]
        } else {
            vec![self.path.clone(), format!("{}/{{*rest}}", self.path)]
        }
    }

    /// The max chunk a stream route forwards, when it declares one.
    pub(super) fn max_frame(&self) -> Option<usize> {
        self.listener
            .as_ref()
            .and_then(|listener| listener.max_frame)
            .and_then(|frame| usize::try_from(frame).ok())
    }

    /// The idle timeout a stream route declares, if any.
    pub(super) fn idle(&self) -> Option<std::time::Duration> {
        self.listener
            .as_ref()
            .and_then(|listener| listener.idle_timeout_seconds)
            .map(|seconds| std::time::Duration::from_secs(u64::from(seconds)))
    }
}

/// The validated table: what to mount and what was refused.
#[derive(Debug, Default)]
pub(super) struct RouteTable {
    pub(super) routes: Vec<MountedRoute>,
    /// `(app, route id, reason)` per refused route.
    pub(super) refused: Vec<(String, String, RouteConflict)>,
}

impl RouteTable {
    /// Validate every declared route this mode owns.
    ///
    /// The order is the app order (`PluginStore::plugins` is name-sorted), so a
    /// collision is reported the same way on every boot instead of depending on
    /// a hash map's iteration order.
    pub(super) fn build_for(declared: &[(String, NetworkSpec)], mode: super::RouteMode) -> Self {
        let mut table = Self::default();
        // `(pattern, method) -> "app/route"` of the route that owns it.
        let mut taken: BTreeMap<(String, String), String> = BTreeMap::new();
        for (app, network) in declared {
            for spec in &network.routes {
                // A route this table does not own is not a conflict: a `%stream`
                // route with a raw listener belongs to the stream transport, and
                // a `tls-server` route to the dedicated TLS listener. Only a
                // route that *is* mine and cannot be mounted is refused.
                if !owns(mode, spec) {
                    continue;
                }
                let route = match normalize(app, spec, &network.auth_mode, mode) {
                    Ok(route) => route,
                    Err(conflict) => {
                        table.refused.push((app.clone(), spec.id.clone(), conflict));
                        continue;
                    }
                };
                let patterns = route.patterns();
                let methods = methods_of(&route.method);
                let mut conflict = None;
                'scan: for pattern in &patterns {
                    for method in &methods {
                        if let Some(owner) = taken.get(&(pattern.clone(), collision_key(method))) {
                            conflict = Some(RouteConflict::Duplicate {
                                pattern: pattern.clone(),
                                method: method.clone(),
                                other: owner.clone(),
                            });
                            break 'scan;
                        }
                    }
                }
                if let Some(conflict) = conflict {
                    table.refused.push((app.clone(), spec.id.clone(), conflict));
                } else {
                    for pattern in &patterns {
                        for method in &methods {
                            taken.insert(
                                (pattern.clone(), collision_key(method)),
                                format!("{app}/{}", spec.id),
                            );
                        }
                    }
                    table.routes.push(route);
                }
            }
        }
        table
    }
}

/// Whether `mode` is the table that serves `spec`.
///
/// A `%stream` route is served by a router only when its listener is a WebSocket
/// upgrade; `tcp`/`tls` listeners are raw sockets the boot phase binds and a
/// route with no listener has no address at all. The `tls-server` record decides
/// *which* router: `wss` belongs to the app's dedicated TLS listener, exactly
/// like an `https` route.
fn owns(mode: super::RouteMode, spec: &RouteSpec) -> bool {
    match spec.kind {
        RouteKind::Http => match mode {
            super::RouteMode::Plain => spec.tls_server.is_none(),
            super::RouteMode::Tls => spec.tls_server.is_some(),
        },
        RouteKind::Stream => {
            let websocket = spec
                .listener
                .as_ref()
                .is_some_and(|listener| listener.transport == ListenerTransport::Websocket);
            websocket
                && match mode {
                    super::RouteMode::Plain => spec.tls_server.is_none(),
                    super::RouteMode::Tls => spec.tls_server.is_some(),
                }
        }
    }
}

/// The key a method occupies in the collision table.
///
/// axum's `get()` also answers `HEAD`, and its `head()` refuses an already-set
/// HEAD handler. Two declared routes (`GET` in one app, `HEAD` in another) would
/// therefore panic the router at mount time, so they share one key here and the
/// second is refused with the others.
fn collision_key(method: &str) -> String {
    if method == "HEAD" {
        "GET".to_string()
    } else {
        method.to_string()
    }
}

/// The methods one declared `method` becomes.
fn methods_of(method: &str) -> Vec<String> {
    if method == "*" {
        ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
            .iter()
            .map(|method| (*method).to_string())
            .collect()
    } else {
        vec![method.to_string()]
    }
}

/// Validate one declared route and flatten it into what the router mounts.
fn normalize(
    app: &str,
    spec: &RouteSpec,
    default_auth: &RouteAuth,
    mode: super::RouteMode,
) -> Result<MountedRoute, RouteConflict> {
    let stream = spec.kind == RouteKind::Stream;
    match (mode, spec.tls_server.is_some(), stream) {
        (super::RouteMode::Plain, true, _) => {
            return Err(RouteConflict::Invalid {
                path: spec.path.clone(),
                reason: "declares `tls-server`: served by the dedicated TLS listener".to_string(),
            });
        }
        (super::RouteMode::Tls, false, _) => {
            return Err(RouteConflict::Invalid {
                path: spec.path.clone(),
                reason: "does not declare `tls-server`: served by the plain listener".to_string(),
            });
        }
        _ => {}
    }
    let path = normalize_path(&spec.path).ok_or_else(|| RouteConflict::Invalid {
        path: spec.path.clone(),
        reason: "a declared path is literal: only URL path characters are allowed, \
                 no `{}`, `*` or `:` placeholders"
            .to_string(),
    })?;
    if path == "/" {
        return Err(RouteConflict::RootClaim);
    }
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    if FRAMEWORK_PREFIXES.contains(&first) {
        return Err(RouteConflict::FrameworkPrefix { path });
    }
    // A stream route is an upgrade: the only method axum can offer it is `GET`,
    // whatever the declaration says (and `*` would collide with itself).
    let method = if stream {
        "GET".to_string()
    } else {
        let method = spec.method.trim().to_ascii_uppercase();
        if method.is_empty() || method == "*" {
            "*".to_string()
        } else if is_method(&method) {
            method
        } else {
            return Err(RouteConflict::Invalid {
                path,
                reason: format!("`{method}` is not an HTTP method this host mounts"),
            });
        }
    };
    Ok(MountedRoute {
        app: app.to_string(),
        id: spec.id.clone(),
        method,
        path,
        auth: spec.auth.clone().unwrap_or_else(|| default_auth.clone()),
        // No framework-side ceiling here: a declared route is bounded by the
        // host's global body limit (`[server] body_limit_mb`), the same one
        // every other route has. The old `.min(1 MiB)` silently truncated
        // whatever the app declared.
        body_limit: spec.body_limit.map_or(usize::MAX, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        }),
        stream,
        listener: spec.listener.clone(),
    })
}

/// Validate and normalize a declared path.
///
/// The host interprets no placeholders (`types.route`), so anything axum would
/// read as one is refused instead of silently becoming a wildcard: `{}`, `*`
/// and `:` are pattern syntax, `?`/`#` never appear in a path, and `..` would
/// climb out of the namespace the operator believes it is mounting.
pub(super) fn normalize_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if !raw.starts_with('/') {
        return None;
    }
    let mut path = raw.trim_end_matches('/');
    if path.is_empty() {
        path = "/";
    }
    if path
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~' | '/')))
    {
        return None;
    }
    if path.split('/').any(|segment| segment == "..") {
        return None;
    }
    Some(path.to_string())
}

/// Whether `method` is one this router mounts.
fn is_method(method: &str) -> bool {
    matches!(
        method,
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    )
}

#[cfg(test)]
mod tests {
    use super::FRAMEWORK_PREFIXES;

    /// The Desk's route table, embedded so the prefix guard is checked against
    /// the source of truth instead of a second hand-maintained copy (SRV-001).
    const DESK_ROUTES: &str = include_str!("../../../../ferris-web/src/app/routes.rs");

    /// First path segments of every literal `<Route path=path!("/…")>` in the
    /// Desk table. Parametric and catch-all routes are skipped: they are not
    /// literal prefixes and cannot collide with a declared route's first
    /// segment.
    fn desk_prefixes() -> Vec<String> {
        let mut prefixes = Vec::new();
        for line in DESK_ROUTES.lines() {
            let Some(start) = line.find("path!(\"/") else {
                continue;
            };
            let rest = &line[start + r#"path!("/"#.len()..];
            let Some(end) = rest.find('"') else {
                continue;
            };
            let path = &rest[..end];
            let Some(first) = path.split('/').next() else {
                continue;
            };
            if first.is_empty() || first.starts_with(':') || first.starts_with('*') {
                continue;
            }
            prefixes.push(first.to_string());
        }
        prefixes
    }

    #[test]
    fn every_literal_desk_route_is_a_framework_prefix() {
        let prefixes = desk_prefixes();
        assert!(
            !prefixes.is_empty(),
            "the Desk route table moved or changed shape; the guard read nothing"
        );
        for prefix in prefixes {
            assert!(
                FRAMEWORK_PREFIXES.contains(&prefix.as_str()),
                "`/{prefix}` is a Desk route but not in FRAMEWORK_PREFIXES: a plugin \
                 declaring it would panic the merged router at boot"
            );
        }
    }

    #[test]
    fn framework_prefixes_are_sorted_and_unique() {
        let mut sorted = FRAMEWORK_PREFIXES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.as_slice(),
            FRAMEWORK_PREFIXES,
            "keep FRAMEWORK_PREFIXES sorted and free of duplicates"
        );
    }
}
