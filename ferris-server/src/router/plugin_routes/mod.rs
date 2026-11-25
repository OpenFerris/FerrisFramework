//! The routes an app declares in `describe()`, mounted at the root.
//!
//! Routes live outside the `/plugins/{app}` namespace: an app declares
//! `route { id, method, path, auth, body-limit }` and the host mounts it where
//! the path says. Two properties matter more than the mechanism:
//!
//! * **Fail closed at load.** A declared path that collides with the framework's
//!   own surface (`/api`, `/plugins`, `/desk`, …), with another app, or with a
//!   second route of the same app is **not mounted** and is logged with the
//!   reason. A plugin must never be able to shadow `/api/*` or take over the
//!   catch-all; axum would panic on an overlapping method route, and a panic at
//!   boot is a worse answer than refusing one route.
//! * **One dispatch path.** The handler forwards to
//!   [`PluginStore::handle_request_for_route`] with the declared `route.id`, so
//!   the guest learns which route the host matched instead of re-deriving it
//!   from the path — the same contract `/plugins/*` uses with its reserved id.
//!
//! # Route auth
//!
//! * `none` — dispatched as an anonymous guest, whatever session the caller
//!   happens to carry. The plugin decides what to do with the request.
//! * `session` — a desk session cookie is required; the host answers
//!   `401` before dispatch and binds the resolved user to `platform::current-user`.
//! * `capability(name)` — a machine-to-machine credential. The caller sends
//!   `x-ferris-capability: <token>` and the host compares it in constant time
//!   against the app's secret config key `route.<name>.token` (the variant
//!   carries the *name*, not the secret). `401` when the header is absent or
//!   wrong, `403` when the app never provisioned the key — an operator reading
//!   the two statuses can tell "bad token" from "not configured". The header is
//!   **never forwarded** to the guest, so a plugin's own credential policy stays
//!   its own and the token cannot leak into a log the plugin writes.
//!
//! # `%stream` routes
//!
//! A `%stream` route is a byte stream, not a request/response pair. Its
//! `listener` decides who owns the socket:
//!
//! * `transport: websocket` — an HTTP upgrade, so [`mount`] mounts it in axum at
//!   the declared path (`WebSocketUpgrade`), with the route's `auth` enforced
//!   *before* the upgrade, and forwards frames to `handle-stream` /
//!   `handle-stream-event` ([`stream`]). A `tls-server` record on the same route
//!   makes it `wss`: it is then mounted on the app's dedicated TLS listener
//!   instead of the plain one.
//! * `transport: tcp` / `tls` — a raw socket the boot phase binds
//!   ([`bind_declared_listeners`](crate::plugin_net::bind_declared_listeners));
//!   it is not part of any HTTP router.
//! * no `listener` — a declaration with no address to serve: reported at boot,
//!   never mounted silently.
//!
//! # Hot reload
//!
//! The table is built **once, at boot**. A `ferris dev` reload re-installs the
//! app's descriptor (capabilities, routes, auth) in the store, and every route
//! already mounted keeps dispatching through it, so editing a handler or
//! changing a route's behaviour needs no restart. A route that is **new** in the
//! reloaded component is *not* mounted until the server restarts: mounting a
//! pattern at runtime would mean validating collisions against the live axum
//! table, and a failure there is unrecoverable (axum panics on an overlapping
//! method route). Fail-closed means the new path answers what an unknown path
//! answers — the 404 fallback — instead of risking the whole router.

mod mount;
mod stream;
mod table;

pub use mount::{DeclaredRouteState, RouteMode, declared_route_router};
pub(crate) use mount::{build, collect};

#[cfg(test)]
mod tests;
