//! The network surface an app declares: capabilities, routes, the default auth
//! and the HTTP/TLS policy, kept host-side in the shape the server mounts.
//!
//! Routes, certificates and listeners live in the descriptor while their
//! **execution** stays in the host: the guest declares, the host grants, the
//! server binds. This module is the seam between the two — it
//! parses the typed declaration once, when `describe()` runs, and stores it on
//! the [`Plugin`](crate::Plugin) so the router can mount routes without calling
//! back into the guest.
//!
//! Nothing here executes anything: no socket is bound, no certificate is
//! loaded. Binding is the server phase's job, and the material it needs is
//! carried verbatim.

use crate::ferris::framework::descriptor::{
    Descriptor, Route, RouteKind as WitRouteKind, TlsClient, TlsServer,
};
use crate::ferris::framework::types::{HttpPolicy, RouteAuth};
use crate::host::{Capability, StreamListenerSpec};

/// What one declared route asks the host to mount.
///
/// Not `PartialEq`: the TLS records it carries are generated bindings, which
/// derive `Debug`/`Clone` only.
#[derive(Debug, Clone)]
pub struct RouteSpec {
    /// The id the guest receives in `handle-request`.
    pub id: String,
    /// `GET`, `POST`, … or `*`; ignored for a stream route.
    pub method: String,
    /// A prefix matched segment-wise under `/plugins/{app}`.
    pub path: String,
    /// Plain HTTP (answered by `handle-request`) or a stream surface.
    pub kind: RouteKind,
    /// The auth the route requires; `None` inherits
    /// [`NetworkSpec::auth_mode`].
    pub auth: Option<RouteAuth>,
    /// Request body cap, already declared in bytes (the server clamps it to its
    /// own maximum before honouring it).
    pub body_limit: Option<u32>,
    /// The server certificate this route serves with, when it terminates TLS
    /// itself.
    pub tls_server: Option<TlsServer>,
    /// The listener a stream route owns.
    pub listener: Option<StreamListenerSpec>,
}

/// The transport of a declared route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    /// Plain HTTP, answered by `handle-request`.
    Http,
    /// An HTTP upgrade or a mounted listener, handled as a stream.
    Stream,
}

/// The app's whole network declaration, host-side.
#[derive(Debug, Clone)]
pub struct NetworkSpec {
    /// What the app **asked** for. The granted subset lives in
    /// [`HostState::grants`](crate::host::HostState::grants) and is what
    /// `platform::grants` reports.
    pub capabilities: Vec<Capability>,
    /// The surface to mount. Empty means `handle-request` is never called.
    pub routes: Vec<RouteSpec>,
    /// The default auth of routes that declare none.
    pub auth_mode: RouteAuth,
    /// The app-wide outbound policy, intersected with the host's own.
    pub http_policy: Option<HttpPolicy>,
    /// The app-wide outbound TLS defaults.
    pub tls_client: Option<TlsClient>,
    /// The app-wide inbound TLS defaults.
    pub tls_server: Option<TlsServer>,
}

impl NetworkSpec {
    /// The route declared under `id`, if any.
    #[must_use]
    pub fn route(&self, id: &str) -> Option<&RouteSpec> {
        self.routes.iter().find(|route| route.id == id)
    }
}

impl From<&Descriptor> for NetworkSpec {
    fn from(described: &Descriptor) -> Self {
        Self {
            capabilities: described
                .capabilities
                .iter()
                .map(|capability| Capability::from_wit(*capability))
                .collect(),
            routes: described.routes.iter().map(route_spec).collect(),
            auth_mode: described.auth_mode.clone(),
            http_policy: described.http_policy.clone(),
            tls_client: described.tls_client.clone(),
            tls_server: described.tls_server.clone(),
        }
    }
}

fn route_spec(route: &Route) -> RouteSpec {
    RouteSpec {
        id: route.id.clone(),
        method: route.method.clone(),
        path: route.path.clone(),
        kind: match route.kind {
            WitRouteKind::Http => RouteKind::Http,
            WitRouteKind::Stream => RouteKind::Stream,
        },
        auth: route.auth.clone(),
        body_limit: route.body_limit,
        tls_server: route.tls_server.clone(),
        listener: route.listener.clone().map(StreamListenerSpec::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_route_is_kept_with_its_own_auth() {
        let described = crate::ferris::framework::descriptor::Descriptor {
            capabilities: vec![
                crate::ferris::framework::types::Capability::Routes,
                crate::ferris::framework::types::Capability::StreamsListen,
            ],
            routes: vec![Route {
                id: "sensors".into(),
                method: "*".into(),
                path: "/sensors".into(),
                kind: WitRouteKind::Stream,
                auth: None,
                body_limit: None,
                tls_server: None,
                listener: Some(crate::ferris::framework::types::StreamListen {
                    transport: crate::ferris::framework::types::StreamTransport::Tcp,
                    address: Some("127.0.0.1:7000".into()),
                    port: None,
                    tls: None,
                    auth: Some(RouteAuth::Session),
                    max_frame: None,
                    idle_timeout_seconds: None,
                }),
            }],
            auth_mode: RouteAuth::Session,
            http_policy: None,
            tls_client: None,
            tls_server: None,
            doctypes: Vec::new(),
            themes: Vec::new(),
            folders: Vec::new(),
            workspaces: Vec::new(),
            methods: Vec::new(),
            ui_extensions: None,
            dashboards: None,
            widgets: None,
            website: None,
            assets: None,
            subscriptions: None,
            config: None,
            plugin_version: None,
            requires: None,
        };

        let network = NetworkSpec::from(&described);
        assert!(matches!(network.auth_mode, RouteAuth::Session));
        assert_eq!(
            network.capabilities,
            vec![Capability::Routes, Capability::StreamsListen]
        );
        let route = network.route("sensors").expect("route stored by id");
        assert_eq!(route.kind, RouteKind::Stream);
        // A route that declares no auth inherits the app default: the stored
        // spec keeps the "none declared" state so the server applies the
        // default itself, instead of the parser freezing one answer.
        assert!(route.auth.is_none());
        assert_eq!(
            route
                .listener
                .as_ref()
                .and_then(|listener| listener.address.as_deref()),
            Some("127.0.0.1:7000")
        );
    }
}
