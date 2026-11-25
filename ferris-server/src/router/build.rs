//! The fused router constructor: mounts the roles the settings request, wires
//! the global layers and resolves the application state.
//!
//! # Compile-time gating
//!
//! Each mount is guarded by the feature that provides its handlers:
//!
//! * `web` — `/static/*`, `/pkg/*`, the Leptos SSR tree and the auth guard in
//!   front of it;
//! * `wasm` — `/events` (the pubsub bridge), `/plugins/*`, the reserved
//!   `/api/method/<name>` handlers and the routes an app declares;
//! * `realtime` — the socket.io engine and its outermost layer (the one
//!   exception: it needs both `realtime` and, for the pubsub bridge, `wasm`);
//!
//! The API role, the public session endpoints, OAuth, health and the global
//! layers are unconditional: they are the `api` base every build carries.

#[cfg(feature = "web")]
use std::collections::HashSet;
use std::sync::Arc;
#[cfg(feature = "web")]
use std::sync::Once;
use std::time::Duration;

#[cfg(feature = "wasm")]
use axum::routing::post;
use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    middleware::{Next, from_fn, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::get,
};
use ferris_api::{ApiState, OAuthState, SessionHandlers};
use ferris_auth::login::{Backend, DbSessionStore, auth_layer};
use ferris_auth::middleware::require_csrf;
use ferris_auth::{AuthState, OAuthManager};
use ferris_db::Db;
#[cfg(feature = "wasm")]
use ferris_wasm::set_realtime_broadcaster;
#[cfg(feature = "web")]
use leptos::view;
#[cfg(feature = "web")]
use leptos_axum::{AxumRouteListing, LeptosRoutes, generate_route_list};
#[cfg(feature = "web")]
use leptos_config::Env;
#[cfg(feature = "realtime")]
use socketioxide::layer::SocketIoLayer;
use tower_http::{limit::RequestBodyLimitLayer, timeout::TimeoutLayer, trace::TraceLayer};
use tracing::{info, warn};

use crate::client_identity::bind_client_identity;
use crate::roles::ServerRole;
use crate::server::ServerSettings;
#[cfg(feature = "wasm")]
use crate::sse::{FanoutBroadcaster, PubsubBus, sse_events};

#[cfg(feature = "web")]
use super::assets::{desk_css, pkg_no_store, preview_asset, vendor_asset};
use super::auth_layers::{bind_ambient_user, require_auth_or_device};
use super::cors::build_cors_layer;
use super::fallback::{health, not_found_fallback};
use super::oauth::oauth_config_from_settings;
#[cfg(feature = "web")]
use super::pages::{plugin_page_paths, public_page_guard, website_form_submit};
#[cfg(feature = "wasm")]
use super::plugin_http::{
    bind_plugin_user, plugin_describe, plugin_handle_request, plugin_ui_asset, plugin_ui_widget,
};
#[cfg(feature = "wasm")]
use super::plugin_routes::{self};
#[cfg(feature = "wasm")]
use super::plugin_stream::{plugin_stream_eof, plugin_stream_rest, plugin_stream_ws};
#[cfg(feature = "wasm")]
use super::plugins::register_plugin_methods;
#[cfg(feature = "realtime")]
use super::realtime::watch_realtime_shutdown;
use super::security::{SecurityPolicy, security_headers_with_policy};
use super::state::AppState;
#[cfg(feature = "web")]
use super::state::{LeptosConfig, LeptosState};
#[cfg(feature = "web")]
use super::web_auth::{bind_desk_user, provide_desk_context, web_auth_guard};

/// Ensures the Leptos futures executor is initialized at most once per process.
#[cfg(feature = "web")]
static LEPTOS_EXECUTOR_INIT: Once = Once::new();

/// The `Host` names this process serves.
///
/// `[server] allowed_hosts` is installed by `ferris serve`/`run` before boot
/// (the list is process-wide, mirroring the one-tenant-per-process model), and
/// `[server] external_url` is added here because that origin is what clients
/// are told to use — rejecting it would break OAuth callbacks and anything
/// else built from the configured public URL.
#[derive(Clone)]
struct AllowedHosts(Arc<[String]>);

fn configured_allowed_hosts(settings: &ServerSettings) -> AllowedHosts {
    let mut hosts = ferris_core::tenant::allowed_hosts().to_vec();
    if let Some(url) = settings.config.server.external_url.as_deref() {
        if let Some(host) = ferris_core::tenant::normalize_host(url) {
            hosts.push(host);
        }
    }
    AllowedHosts(hosts.into())
}

/// Reject a request addressed to a name this process does not serve.
///
/// `421 Misdirected Request` is the status for "the origin is not authoritative
/// for this name": answering with the Desk would leak this instance's session
/// cookie to whatever host a client typed. An empty list is the documented
/// default (no restriction), so the middleware is a no-op there. The check is
/// deliberately independent of auth: it runs before any credential is read.
async fn require_allowed_host(
    State(allowed): State<AllowedHosts>,
    request: Request,
    next: Next,
) -> Response {
    if allowed.0.is_empty() {
        return next.run(request).await;
    }
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(ToString::to_string));
    if host
        .as_deref()
        .is_some_and(|host| ferris_core::tenant::host_is_allowed(host, &allowed.0))
    {
        return next.run(request).await;
    }
    (
        StatusCode::MISDIRECTED_REQUEST,
        "421 Misdirected Request: this Host is not served by this instance",
    )
        .into_response()
}

/// OAuth providers persisted in `Social Login Key` / `Connected App` rows.
///
/// `[auth.oauth].providers` wins: this is only consulted when that table is
/// empty, so a TOML deployment keeps behaving exactly as before. Rows are read
/// as the integrations service user; a read failure is logged and leaves the
/// static (empty) table in place instead of failing boot (INT-01).
///
/// `build_router` is synchronous and runs on the multi-threaded server runtime,
/// so the database reads are driven by parking one worker. A current-thread
/// runtime (unit tests) keeps the static table instead of panicking inside
/// `block_in_place`.
#[cfg(feature = "integrations")]
fn oauth_config_from_doctypes(pool: &Db) -> Option<ferris_auth::OAuthConfig> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
        warn!(
            "OAuth providers in DocTypes need the multi-threaded server runtime; \
             using the static table only"
        );
        return None;
    }
    let pool = pool.clone();
    let loaded = tokio::task::block_in_place(|| {
        handle.block_on(async move {
            let context = ferris_integrations::runtime::IntegrationsContext::new(pool);
            let mut providers =
                ferris_integrations::runtime::oauth::load_social_login_keys(&context).await?;
            providers
                .extend(ferris_integrations::runtime::oauth::load_connected_apps(&context).await?);
            Ok::<_, ferris_core::Error>(providers)
        })
    });
    match loaded {
        Ok(providers) if !providers.is_empty() => Some(ferris_auth::OAuthConfig {
            providers,
            ..ferris_auth::OAuthConfig::default()
        }),
        Ok(_) => None,
        Err(error) => {
            warn!(
                error = %error,
                "could not read OAuth providers from DocTypes; using the static table"
            );
            None
        }
    }
}

/// No integrations runtime: the DocTypes cannot be read, so only TOML counts.
#[cfg(not(feature = "integrations"))]
const fn oauth_config_from_doctypes(_pool: &Db) -> Option<ferris_auth::OAuthConfig> {
    None
}

/// Fuse REST API, realtime socket.io and the desk web UI (with Leptos SSR + hydration)
/// into a single router.
pub fn build_router(
    settings: &ServerSettings,
    pool: Db,
    auth_state: AuthState,
    session_handlers: SessionHandlers,
    backend: Backend,
    session_store: DbSessionStore,
    modules: &ferris_api::ModuleRegistry,
) -> Router {
    // Framework-owned DocTypes are registered on every boot, independent of any
    // loaded app: each crate declares its own, the host calls them all in
    // `register_doctype_catalog()`. Their tables are created lazily by the
    // document engine on first use.
    crate::catalog::register_doctype_catalog(modules);

    // The Desk workspaces, declared by each native crate. This runs after the
    // DocType catalog so a workspace link can be resolved against it, and logs
    // any declaration that will not render (see `catalog::register_workspace_catalog`).
    crate::catalog::register_workspace_catalog(modules);

    // The host's own widget catalog (`host.stat`, `host.trend`, `host.table`)
    // joins the global registry on every boot — the parametrized replacements
    // for the dashboards that used to be hardcoded views. Idempotent, like the
    // core DocTypes above.
    let host_widgets = ferris_core::ui_extensions::register_host_widgets();
    for warning in &host_widgets.warnings {
        warn!("ui extensions (host): {warning}");
    }

    #[cfg(feature = "web")]
    let pkg_dir = resolve_pkg_dir();

    #[cfg(feature = "web")]
    let leptos_config = LeptosConfig {
        output_name: "ferris-web".into(),
        // The shell emits `/pkg/...`; only the directory ServeDir reads from
        // moves to the framework bundle when the project has none.
        site_root: "./".into(),
        site_pkg_dir: "pkg".into(),
        // `serve` is a production process: `Env::DEV` made Leptos inject the
        // `AutoReload` inline script, which the strict CSP then blocked. Only
        // `ferris dev` (settings.dev) needs the dev environment.
        env: if settings.dev { Env::DEV } else { Env::PROD },
        desk_css_dev: settings.desk_css_dev.clone().map(Arc::from),
    };

    // Process-wide pubsub bus: the registered broadcaster feeds it and the
    // `/events` SSE endpoint subscribes. Created before `AppState` so the route
    // and the broadcaster share one instance.
    #[cfg(feature = "wasm")]
    let pubsub_bus = PubsubBus::new();

    // Wire the notification publish hook into the live bus: a notification
    // stored by the API (login welcome, mark-read flows) or by a plugin host
    // then reaches every open Desk session over SSE instead of waiting for a
    // refetch. First install wins; a second mount must not replace the live bus.
    if !ferris_api::notifications::install() {
        warn!("notification publisher already installed");
    }

    // Document changes flow through `ferris_api::notify`; the realtime engine
    // is one possible sink. Installing it here keeps `ferris-api` free of the
    // optional vertical while preserving the live event stream.
    super::notifier::install_realtime_notifier();

    // SEC-08: the observability layer consults this list before trusting any
    // forwarding header. The composition root owns the settings, so it must
    // install them before the router can serve a request.
    crate::observability::set_trusted_proxies(settings.config.server.trusted_proxies.iter());

    let api_log_base_origin = settings
        .config
        .server
        .external_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned);

    // / H7: the OAuth2 **login** surface. Built here, before
    // `AppState`, because the routes resolve `OAuthState` through `FromRef` (the
    // axum embedding pattern) and because the callback opens the session on the
    // same `SessionHandlers` the rest of the server reads. Pending states live in
    // `tab_oauth_state` (the manager is built over the pool), so a callback can
    // land on any replica. A deployment with no providers gets an inert state and
    // no routes.
    //
    // An empty `[auth.oauth].providers` is not necessarily "no OAuth": the
    // `Social Login Key`/`Connected App` DocTypes can carry the clients. The
    // TOML table still wins — it is only when it is empty that the rows are
    // read (INT-01).
    let (oauth_state, oauth_ready) = if settings.config.auth.oauth.providers.is_empty() {
        match oauth_config_from_doctypes(&pool) {
            Some(config) => match OAuthManager::with_db(config, pool.clone()) {
                Ok(manager) => {
                    info!("OAuth providers loaded from Social Login Key/Connected App");
                    (OAuthState::new(manager, session_handlers.clone()), true)
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "OAuth providers found in DocTypes but the flow could not be built; \
                         /api/oauth/* endpoints are not mounted"
                    );
                    (
                        OAuthState::new(OAuthManager::default(), session_handlers.clone()),
                        false,
                    )
                }
            },
            None => (
                OAuthState::new(OAuthManager::default(), session_handlers.clone()),
                false,
            ),
        }
    } else {
        match oauth_config_from_settings(settings)
            .and_then(|config| OAuthManager::with_db(config, pool.clone()))
        {
            Ok(manager) => (OAuthState::new(manager, session_handlers.clone()), true),
            Err(e) => {
                warn!(
                    error = %e,
                    "OAuth providers configured but the flow could not be built; \
                     /api/oauth/* endpoints are not mounted"
                );
                (
                    OAuthState::new(OAuthManager::default(), session_handlers.clone()),
                    false,
                )
            }
        }
    };

    let app_state = AppState {
        #[cfg(feature = "web")]
        leptos: LeptosState {
            config: leptos_config,
        },
        api: ApiState::with_policy_path(pool.clone(), settings.policy_path.clone())
            // Same `AuthConfig` as the login gate, so the authenticated MFA
            // self-service routes agree with the login flow's settings.
            .with_auth_config(&settings.config.auth)
            // `[features]` decides which framework-owned routes exist at all.
            .with_features(settings.config.features.clone())
            // Native modules contribute their documented paths from the same
            // registry the router is built from.
            .with_openapi_fragments(modules.openapi_fragments())
            // Metadata endpoints project registry labels; they translate
            // against the same catalogue set the Desk shell negotiates.
            .with_i18n(settings.i18n.clone()),
        auth: auth_state.clone(),
        session: session_handlers.clone(),
        oauth: oauth_state,
        #[cfg(feature = "wasm")]
        pubsub: pubsub_bus.clone(),
        api_log_base_origin,
        #[cfg(feature = "wasm")]
        plugins: settings.plugins.clone(),
        i18n: settings.i18n.clone(),
    };

    // While wiring handlers we keep `Router<AppState>`; at the end we resolve
    // the state to `Router<()>` (the type `axum::serve` needs).
    let mut router: Router<AppState> = Router::new().route("/health", get(health));

    // Public module routes are mounted before the auth layers: a module that
    // exposes a signed URL owns its own authorization (the HMAC check) and must
    // not be wrapped by `require_auth`. `public_mount()` is a real prefix, never
    // `/`, so this cannot become a catch-all in front of the API.
    router = super::modules::mount_public(router, modules, &app_state);

    // Plugin pubsub → browser push without socket.io: a widget opens this
    // with `EventSource` and receives each publish on its channel. The bus
    // belongs to the plugin host, so it is compiled with `wasm`.
    #[cfg(feature = "wasm")]
    {
        router = router.route("/events", get(sse_events));
    }

    // Public session endpoints live OUTSIDE the `require_auth` layer (they are
    // the way in). `/api/auth/*` has no static prefix conflict with the guarded
    // API router below.
    //
    // logout is the one public auth route with a side effect on the
    // session store, so it carries the CSRF gate (`require_csrf`) as a route
    // layer — a cross-site form must not be able to log a user out. Login,
    // session and the MFA challenge-step complete it deliberately stay open:
    // there is no session yet to hold a token.
    // Public locale negotiation: the login page needs the catalogue before a
    // session exists, and the payload is the static catalog the binary embeds.
    router = router
        .route("/api/i18n", get(super::i18n::available))
        .route("/api/i18n/{locale}", get(super::i18n::catalog));

    router = router.merge(
        Router::new()
            .route("/api/auth/login", axum::routing::post(ferris_api::login))
            .route("/api/auth/session", get(ferris_api::session_me))
            .route(
                "/api/auth/mfa/verify",
                axum::routing::post(ferris_api::verify_mfa),
            )
            // Recovery and invitation acceptance are ways in for someone with
            // no session yet, so they stay outside `require_auth` like login.
            // They answer a generic `204`/`400` and never say whether an
            // address has an account.
            .route(
                "/api/auth/password-reset/request",
                axum::routing::post(ferris_api::request_password_reset),
            )
            .route(
                "/api/auth/password-reset/confirm",
                axum::routing::post(ferris_api::confirm_password_reset),
            )
            .route(
                "/api/auth/email/verify/confirm",
                axum::routing::post(ferris_api::confirm_email_verification),
            )
            .route(
                "/api/auth/invitations/accept",
                axum::routing::post(ferris_api::accept_invitation),
            )
            .merge(
                Router::new()
                    .route("/api/auth/logout", axum::routing::post(ferris_api::logout))
                    .route(
                        "/api/auth/logout-all",
                        axum::routing::post(ferris_api::logout_all),
                    )
                    // The account's locale is a self-service mutation: it is
                    // behind the same CSRF gate as logout.
                    .route(
                        "/api/auth/language",
                        axum::routing::put(ferris_api::set_language),
                    )
                    // API-key lifecycle: the caller's own keys only. The
                    // plaintext secret is returned once, by `POST`.
                    .route(
                        "/api/auth/api-keys",
                        get(ferris_api::list_api_keys).post(ferris_api::create_api_key),
                    )
                    .route(
                        "/api/auth/api-keys/{id}",
                        axum::routing::delete(ferris_api::delete_api_key),
                    )
                    // Verification is requested by the signed-in account about
                    // itself; the token goes to its mailbox, not to the caller.
                    .route(
                        "/api/auth/email/verify/request",
                        axum::routing::post(ferris_api::request_email_verification),
                    )
                    // Invitations are a `System Manager` surface (the handlers
                    // re-check the role); the CSRF gate is the same one every
                    // other mutating auth route uses.
                    .route(
                        "/api/auth/invitations",
                        get(ferris_api::list_invitations).post(ferris_api::create_invitation),
                    )
                    .route(
                        "/api/auth/invitations/{id}",
                        axum::routing::delete(ferris_api::delete_invitation),
                    )
                    .route_layer(from_fn_with_state(auth_state.clone(), require_csrf)),
            ),
    );

    if settings.roles.has(ServerRole::Api) {
        for module in modules.modules() {
            module.register_methods(&app_state.api.methods);
        }
        // Operational endpoints computed on demand, mounted with the rest of
        // the API so they share its session gate. `GET /health` stays the
        // public liveness probe; these three report schema-adjacent data.
        let mut system = Router::new()
            .route("/api/system/health", get(super::system::health))
            .route("/api/system/storage", get(super::system::storage));
        #[cfg(feature = "report")]
        {
            system = system.route("/api/report/prepared", get(super::system::prepared));
        }
        let api_router = ferris_api::api_router_with_modules(app_state.clone(), modules.modules())
            .merge(system)
            // `bind_client_identity` is outermost so the verified mTLS facts are
            // in request extensions *before* `require_auth_or_device` decides:
            // a device certificate is accepted as the credential for
            // `/api/method/*`, and `require_csrf` (inside) exempts it. Then
            // `bind_ambient_user` scopes the resolved session user for the
            // metadata engine (fails closed when there is none).
            .layer(from_fn_with_state(auth_state.clone(), bind_ambient_user))
            .layer(from_fn_with_state(
                auth_state.clone(),
                require_auth_or_device,
            ))
            .layer(from_fn(bind_client_identity));
        router = router.merge(api_router);
        info!("api role mounted (with auth middleware)");
    }

    // The socket.io role's layer is applied as the **outermost** layer at the
    // end of this function, after `fallback(..)`. Axum dispatches unmatched
    // paths straight to the catch-all fallback, so a socket layer applied here
    // — before `router.fallback(not_found_fallback)` — would never see
    // `/socket.io` and every handshake would 404 (the pre-fix symptom).
    #[cfg(feature = "realtime")]
    let realtime_socket_layer: Option<SocketIoLayer> = if settings.roles.has(ServerRole::Realtime) {
        let realtime = ferris_realtime::RealtimeServer::new(ferris_realtime::RealtimeConfig {
            host: settings.realtime.host.clone(),
            port: settings.realtime.port,
            ping_interval_secs: settings.realtime.ping_interval_secs,
            ping_timeout_secs: settings.realtime.ping_timeout_secs,
            cors_origins: settings.cors_origins.clone(),
            max_message_size: 1024 * 1024,
        })
        .with_auth(auth_state.clone());
        // Presence maintenance / revalidation must run in production too; the
        // server used to create the engine without ever starting it.
        let _ = realtime.spawn_maintenance(std::time::Duration::from_secs(60));
        // Make the live handler the process-wide document-notification sink, so
        // `ferris-api`'s `DocStore` can publish committed writes to the DocType
        // rooms instead of letting the server value drop (RT2). This is the
        // realtime engine's own seam — no guest runtime involved. The pubsub
        // *broadcaster* bridge below is `wasm`.
        if ferris_realtime::install_handler(realtime.handler().clone()) {
            info!("realtime document notifications wired");
        } else {
            warn!(
                "a realtime handler is already installed; document notifications stay on the first one"
            );
        }
        // A socket.io connection is an upgraded HTTP connection: `axum`
        // counts it as in-flight forever, so graceful shutdown would wait
        // for a browser tab that is still open. Close the engine's sockets
        // when the process signal fires; the handler then drains immediately
        // instead of hitting the timeout fallback.
        watch_realtime_shutdown(realtime.socket_io().clone());
        // Wire the process-wide pubsub broadcaster so plugin `pubsub::publish`
        // calls fan out over socket.io **and** the `/events` SSE bus.
        #[cfg(feature = "wasm")]
        if set_realtime_broadcaster(Arc::new(FanoutBroadcaster::new(
            pubsub_bus.clone(),
            Some(realtime.socket_io().clone()),
        ))) {
            info!("pubsub broadcaster registered (socket.io + SSE bridge)");
        }
        info!("realtime role mounted (socket.io on /socket.io/) with auth");
        Some(realtime.socket_io_layer())
    } else {
        None
    };

    // The pubsub broadcaster is registered **once**, after the realtime role had
    // its chance: with socket.io when the role is mounted, SSE-only otherwise
    // (the realtime feature compiled out included). The `/events` bridge is the
    // reason this must exist even without socket.io: the Desk ships no socket.io
    // client, so a plugin widget would otherwise receive nothing.
    #[cfg(feature = "wasm")]
    {
        // Whether the socket.io engine is mounted *and* compiled. Two `let`s so
        // the feature test stays a `cfg` (not a `cfg!` folded into a boolean
        // clippy would ask to simplify).
        #[cfg(feature = "realtime")]
        let realtime_mounted = settings.roles.has(ServerRole::Realtime);
        #[cfg(not(feature = "realtime"))]
        let realtime_mounted = false;
        if !realtime_mounted {
            // The constructor differs by feature, not by role: with the engine
            // compiled the broadcaster still carries the (empty) socket.io slot.
            #[cfg(feature = "realtime")]
            let broadcaster = FanoutBroadcaster::new(pubsub_bus.clone(), None);
            #[cfg(not(feature = "realtime"))]
            let broadcaster = FanoutBroadcaster::new(pubsub_bus.clone());
            if set_realtime_broadcaster(Arc::new(broadcaster)) {
                info!("pubsub broadcaster registered (SSE bridge only)");
            }
        }
    }

    #[cfg(feature = "web")]
    if settings.roles.has(ServerRole::Web) {
        LEPTOS_EXECUTOR_INIT.call_once(|| {
            if let Err(e) = any_spawner::Executor::init_futures_executor() {
                warn!("leptos futures executor init returned an error: {e}");
            }
        });

        // Single source of truth for the route list: reused for SSR routing.
        let mut route_list = generate_route_list(|| view! { <ferris_web::DeskShell /> });

        // Fase 2: dashboards and website pages the loaded plugins declare
        // become real SSR routes, registered as concrete paths with the same
        // shape (SSR mode, HTTP methods) the generator used for the desk's
        // own routes: cheap, and exactly what the boot-time snapshot gives.
        // The desk's trailing catch-all `<Route>` (the only static tree
        // position that can cover arbitrary plugin paths) comes out of this
        // list and goes back in below behind the Fase 3 guard — registered
        // bare, `/{*rest}` would swallow every unknown path — API 404s
        // included — and SSR the whole desk shell instead.
        let catch_all = route_list
            .iter()
            .position(|listing| listing.path() == "/{*rest}")
            .map(|index| route_list.remove(index));
        if let Some(sample) = route_list.first() {
            let mode = sample.mode().clone();
            let methods: Vec<_> = sample.methods().collect();
            let mut seen: HashSet<String> = route_list
                .iter()
                .map(|listing| listing.path().to_string())
                .collect();
            for path in plugin_page_paths() {
                if seen.insert(path.clone()) {
                    route_list.push(AxumRouteListing::new(
                        path,
                        mode.clone(),
                        methods.clone(),
                        Vec::new(),
                    ));
                }
            }
        }

        // `web_auth_guard` wraps the whole desk router: it is what turns a
        // sessionless `/desk/…` into a `302 /login?next=…` (instead of SSR-ing
        // the chrome as a guest) and an already-signed-in `/login` into a
        // `302` back to the desk. It also resolves the caller so the SSR pass
        // paints the real identity. Assets and plugin/website pages registered
        // on this router pass through untouched.
        let leptos_router: Router<AppState> = Router::new()
            .route("/static/desk.css", get(desk_css))
            // T2-F4: committed third-party libraries (leaflet, jsbarcode, quill,
            // marked), compiled into the binary like `desk.css`. Same
            // auth treatment as every other asset: `web_auth_guard` passes
            // `/static/…` straight through (it is not a desk path).
            .route("/static/vendor/{*path}", get(vendor_asset))
            // D3: widget-catalog preview images, same embedded-table
            // treatment and same auth pass-through as `/static/vendor`.
            .route("/static/previews/{*path}", get(preview_asset))
            .leptos_routes_with_context(
                &app_state,
                route_list,
                {
                    let pool = app_state.api.pool.clone();
                    let i18n = app_state.i18n.clone();
                    move || provide_desk_context(pool.clone(), &i18n)
                },
                || view! { <ferris_web::DeskShell /> },
            )
            // The engine's ambient user for the SSR bridge. `DocStore::Server`
            // authorises through `ferris_document::current_user()`; without it
            // the pre-render read as an anonymous caller. Applied before the
            // guard in code so the guard stays the outer layer: it resolves the
            // identity once and inserts `DeskIdentity` into the request
            // extensions this layer reads.
            .route_layer(from_fn(bind_desk_user))
            .route_layer(from_fn_with_state(auth_state.clone(), web_auth_guard));

        router = router.merge(leptos_router);

        // Fase 3: the catch-all returns as a dynamic guard against the LIVE
        // registry *and* the stored website rows — a page a plugin declares
        // after boot (dev hot-reload) or a `Web Page`/`Web Form` an
        // administrator publishes SSRs without a restart, and everything else
        // still takes `not_found_fallback`, which is what keeps API 404s JSON
        // and the outermost socket.io layer untouched (riesgo R2, pinned by the
        // `dynamic_page_guard_tests` below). The concrete boot routes above
        // keep winning for the paths they know — matchit prefers their
        // static/param segments over the splat — so the guard only runs for
        // paths nothing else claimed.
        if let Some(catch_all) = catch_all {
            let pool = app_state.api.pool.clone();
            let i18n = app_state.i18n.clone();
            let dynamic_pages: Router<AppState> = Router::new()
                .leptos_routes_with_context(
                    &app_state,
                    vec![catch_all],
                    move || provide_desk_context(pool.clone(), &i18n),
                    || view! { <ferris_web::DeskShell /> },
                )
                // A published `Web Form` also answers POST. The GET splat is
                // the Leptos catch-all; POST is the one method it does not
                // register, so the same path takes a second method route
                // instead of a second router. An unknown POST still reaches
                // `not_found_fallback` from the handler and stays JSON for API
                // callers.
                .route("/{*rest}", axum::routing::post(website_form_submit))
                .route_layer(from_fn_with_state(app_state.clone(), public_page_guard));
            router = router.merge(dynamic_pages);
            info!(
                "dynamic page guard mounted (/{{*rest}} consults the live registry and stored pages)"
            );
        }

        // Fase 7: serve the hydrate wasm bundle generated by wasm-bindgen.
        // `HydrationScripts` in the SSR shell references `/pkg/{output_name}.js`.
        //
        // In dev the bundle is rewritten in place on every rebuild, so the
        // browser must not hold on to a previous `ferris-web.js`: a glue from an
        // older build paired with the new wasm fails to link (see
        // `pkg_no_store`). Production keeps the default caching.
        let mut pkg_service =
            Router::new().fallback_service(tower_http::services::ServeDir::new(pkg_dir));
        if settings.dev {
            pkg_service = pkg_service.layer(from_fn(pkg_no_store));
        }
        router = router.nest_service("/pkg", pkg_service);
        info!("web role mounted (Leptos SSR + hydration)");
    }

    // Plugin routes. Two surfaces with different trust levels share the
    // `/plugins` namespace:
    //
    // * the trusted, always-on asset channel (`/{name}` descriptor,
    //   `/{name}/asset/*`, `/{name}/widget/*`), same-origin by design: a
    //   plugin's scripts run with the Desk's origin;
    // * the request-forwarding and raw-stream endpoints (`/{name}/{*rest}`,
    //   `/{name}/ws`, `/{name}/stream/*`), which are a **development** surface
    //   with an optional session. Mounting them in production would expose the
    //   guest with none of the per-route auth the app declares
    //   (`resolve_policy` lives in the declared-route and `/api/plugins`
    //   dispatchers, not here), so production answers 404 for those paths.
    #[cfg(feature = "wasm")]
    if let Some(plugins) = &settings.plugins {
        let mut plugin_router = Router::new()
            .route("/{name}", get(plugin_describe))
            // Fase 1 asset channel: the Desk loads the plugin's UI assets
            // (scripts/stylesheets) through `ui-asset` and widget SSR markup
            // through `ui-widget`. Registered before the dev catch-all below so
            // matchit matches their static segments first.
            .route("/{name}/asset/{*path}", get(plugin_ui_asset))
            .route("/{name}/widget/{widget}", get(plugin_ui_widget));
        if settings.dev {
            plugin_router = plugin_router
                // Raw stream endpoints exercising `handle-stream`: a live
                // WebSocket and a REST body-in/body-out pair.
                .route("/{name}/ws", get(plugin_stream_ws))
                .route(
                    "/{name}/stream/{stream_id}",
                    post(plugin_stream_rest).delete(plugin_stream_eof),
                )
                // Dev forwarding surface: the guest owns everything below its
                // name. `handle_request_as` uses the fixed `plugins` route id,
                // which is why this cannot be the production ingress.
                .route(
                    "/{name}/{*rest}",
                    get(plugin_handle_request).post(plugin_handle_request),
                );
        }
        // Resolve the optional desk session cookie so plugin
        // `session` imports see the real caller; anonymous stays a guest.
        // `bind_client_identity` exposes the verified mTLS certificate facts
        // to the plugin REST/stream handlers and the RPC path.
        let plugin_router = plugin_router
            .layer(from_fn_with_state(auth_state.clone(), bind_plugin_user))
            .layer(from_fn(bind_client_identity))
            .with_state(plugins.clone());
        router = router.nest("/plugins", plugin_router);
        info!(
            dev = settings.dev,
            "plugin routes mounted at /plugins/ (asset channel always; \
             forwarding/stream endpoints only in dev)"
        );
        // H15: the asset channel is same-origin by design, so a plugin's JS runs
        // with the Desk's origin — it is trusted code of the app, not sandboxed
        // guest code. Said once at boot so an operator reading the log knows the
        // trust boundary before installing an app.
        warn!(
            "plugin UI assets are served same-origin (/plugins/{{name}}/asset/…): \
             their scripts run with the Desk's origin and are trusted app code, \
             not sandboxed wasm"
        );

        // App-owned namespaces. `/api/plugins/<app>/*` is one catch-all (the
        // app owns everything below it); `/desk/<app>/*` is one static pattern
        // per loaded app, because the Desk already owns
        // `/desk/{module}/{doctype}` and matchit allows a param or a wildcard
        // at a position, not both. The API auth policy is per declared route
        // (`none`/`session`/`capability`) and is enforced inside the handler,
        // so a public webhook and a session API can share the namespace. Only
        // the caller/identity resolution is a layer; the layer cannot decide
        // per path.
        let plugin_api = super::plugin_app::api_router()
            .layer(from_fn_with_state(auth_state.clone(), bind_plugin_user))
            .layer(from_fn(bind_client_identity));
        router = router.merge(plugin_api);
        #[cfg(feature = "web")]
        {
            let desk_pages = super::plugin_app::desk_router(plugins)
                .layer(from_fn_with_state(auth_state.clone(), bind_plugin_user))
                .layer(from_fn_with_state(auth_state.clone(), web_auth_guard));
            router = router.merge(desk_pages);
        }
        info!("app namespaces mounted at /api/plugins/ and /desk/<app>/");

        // Register one `/api/method/<name>` handler per API method the plugin
        // declares in its descriptor, so desk/the API whitelist can call into
        // the app like any other whitelisted method. Handlers re-check the
        // descriptor on every call, so a hot reload that drops a method answers
        // 404 instead of forwarding to a stale export.
        register_plugin_methods(&app_state.api, plugins);
        // `data::call-method` (the guest's `super`) resolves against this
        // registry; installing it next to the registration keeps both views of
        // the chain pointing at the same object.
        super::method_bridge::install(app_state.api.methods.clone());

        // The routes the apps declare in `describe()` are mounted at the
        // root, each with its own method, auth and body cap. A route that
        // collides with the framework or with another app is refused (logged)
        // rather than silently shadowing `/api/*`. The auth layers reach these
        // routes only: `bind_plugin_user` resolves the optional desk session and
        // `bind_client_identity` exposes the verified mTLS facts, exactly as on
        // `/plugins/*`.
        let declared = plugin_routes::collect(plugins);
        if !declared.is_empty() {
            let declared_router = plugin_routes::build(&declared, plugin_routes::RouteMode::Plain)
                .layer(from_fn_with_state(auth_state.clone(), bind_plugin_user))
                .layer(from_fn(bind_client_identity));
            router = router.merge(declared_router);
        }
    }

    // / H7: OAuth2 login. Mounted only when the operator declared
    // providers **and** the flow could be built; the callback opens a real
    // session (cookie + CSRF) and redirects to `next`, it no longer answers with
    // provider tokens. The routes are public on purpose: they are a way in, and
    // the callback is bound to the browser by the one-time state cookie.
    if oauth_ready {
        router = router.merge(ferris_api::oauth_router_with_state::<AppState>());
        info!(
            providers = settings.config.auth.oauth.providers.len(),
            "oauth login mounted (/api/oauth/*)"
        );
    }

    // Global layers. Applied while the state is still `AppState` so we can
    // keep building; `.layer` preserves the state type.
    let mut router = router
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(settings.request_timeout_secs),
        ))
        .layer(RequestBodyLimitLayer::new(
            settings.body_limit_mb * 1024 * 1024,
        ));

    // 404 for unmatched routes (JSON for API clients, small HTML otherwise).
    router = router.fallback(not_found_fallback);

    // Security headers (M8) wrap the fallback too, so a stray 404 is still
    // delivered with the full header set. The policy carries the only fact the
    // header set cannot infer: whether this process terminates TLS, which is
    // what gates `Strict-Transport-Security` (SRV-005).
    router = router.layer(from_fn_with_state(
        SecurityPolicy::new(settings.config.server.tls.is_some()),
        security_headers_with_policy,
    ));

    // SRV7: the request-path producers sit below CORS and outside `require_auth`
    // so every finished request — the 404 fallback and a timeout 504 included —
    // is recorded with its final status.
    router = router.layer(from_fn_with_state(
        app_state.clone(),
        crate::observability::record_request,
    ));

    // The one session stack, mounted outside every auth gate and inside the
    // socket.io layer: request and response cookies pass through it for every
    // HTTP route, and `record_request` sits inside it so the access log sees
    // the same `AuthSession`/`Session` extensions instead of parsing a
    // credential twice. The socket engine is mounted *above* it: an upgrade is
    // handled by socketioxide itself and resolves the session from the shared
    // store (see `ferris-realtime`), so it neither needs nor disturbs cookie
    // handling.
    router = router.layer(auth_layer(&auth_state.config, session_store, backend));

    // The socket.io layer must be the **last** layer applied before CORS: in
    // axum the last `.layer()` call is the outermost, and `Router::layer` also
    // wraps the catch-all fallback — the only path an unmatched `/socket.io`
    // request flows through. Applied earlier, the engine would sit inside a
    // routing table that never dispatches to it for a path with no matching
    // route.
    #[cfg(feature = "realtime")]
    if let Some(layer) = realtime_socket_layer {
        router = router.layer(layer);
    }

    // CORS outermost so `/socket.io` polling preflights and cross-origin API
    // calls are answered before the request reaches the router.
    if settings.enable_cors {
        router = router.layer(build_cors_layer(settings));
    }

    // Outermost: a request for a name this instance does not serve is refused
    // before CORS, auth or any handler reads a credential. Empty by default,
    // which keeps the historical single-name/no-check deployment working.
    router = router.layer(from_fn_with_state(
        configured_allowed_hosts(settings),
        require_allowed_host,
    ));

    // ✅ Aquí se resuelve el estado: `Router<AppState>` → `Router<()>`.
    // Esta es la conversión que `axum::serve` + `into_make_service()` necesitan.
    router.with_state(app_state)
}

/// Locate the hydrate bundle the SSR shell references.
///
/// `./pkg` is the normal deployment layout, but a site created in an otherwise
/// empty project directory has no bundle next to its `sites/`. The CLI binary
/// built from this workspace can still serve the framework's own
/// `ferris-web/pkg`; without this fallback `/pkg/ferris-web.js` is a 404 with an
/// empty MIME type and the Desk never hydrates.
#[cfg(feature = "web")]
fn resolve_pkg_dir() -> std::path::PathBuf {
    let usable = |dir: &std::path::Path| dir.join("ferris-web.js").is_file();

    let local = std::path::PathBuf::from("pkg");
    if usable(&local) {
        return local;
    }
    if let Ok(home) = std::env::var("FERRIS_HOME") {
        for candidate in [
            std::path::PathBuf::from(&home).join("ferris-web/pkg"),
            std::path::PathBuf::from(&home).join("pkg"),
        ] {
            if usable(&candidate) {
                return candidate;
            }
        }
    }
    if let Ok(data) = std::env::var("FERRIS_DATA_DIR") {
        for candidate in [
            std::path::PathBuf::from(&data).join("ferris-web/pkg"),
            std::path::PathBuf::from(&data).join("pkg"),
        ] {
            if usable(&candidate) {
                return candidate;
            }
        }
    }
    // Only meaningful when the CLI was built from the workspace; that is the
    // same fallback `framework_root` uses for the WIT.
    if let Some(root) = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        let candidate = root.join("ferris-web/pkg");
        if usable(&candidate) {
            return candidate;
        }
    }
    local
}

#[cfg(test)]
mod allowed_hosts_tests {
    use super::{AllowedHosts, require_allowed_host};
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        middleware::from_fn_with_state,
        routing::get,
    };
    use tower::ServiceExt;

    fn app(hosts: &[&str]) -> Router {
        let allowed: Vec<String> = hosts.iter().map(|host| (*host).to_string()).collect();
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn_with_state(
                AllowedHosts(allowed.into()),
                require_allowed_host,
            ))
    }

    async fn status_for(hosts: &[&str], host: Option<&str>) -> StatusCode {
        let mut request = Request::builder().uri("/");
        if let Some(host) = host {
            request = request.header(header::HOST, host);
        }
        app(hosts)
            .oneshot(request.body(Body::empty()).expect("the request builds"))
            .await
            .expect("the router answers")
            .status()
    }

    /// SEC-15: only the configured names reach a handler; a port is part of the
    /// authority, not of the name.
    #[tokio::test]
    async fn a_listed_host_is_served_and_an_unlisted_one_is_misdirected() {
        assert_eq!(
            status_for(&["erp.example.com"], Some("erp.example.com:8000")).await,
            StatusCode::OK
        );
        assert_eq!(
            status_for(&["erp.example.com"], Some("evil.example.com")).await,
            StatusCode::MISDIRECTED_REQUEST
        );
        assert_eq!(
            status_for(&["erp.example.com"], None).await,
            StatusCode::MISDIRECTED_REQUEST,
            "an HTTP/1.1 request with no Host is not addressed to a served name"
        );
    }

    /// An empty list is the documented default: it must not turn every request
    /// into a 421.
    #[tokio::test]
    async fn an_empty_list_does_not_restrict() {
        assert_eq!(status_for(&[], Some("anything.test")).await, StatusCode::OK);
    }

    /// The `DocType` loader refuses to park a current-thread runtime: the static
    /// table is the honest fallback rather than a panic at boot (INT-01).
    #[cfg(feature = "integrations")]
    #[tokio::test]
    async fn oauth_doctypes_are_skipped_outside_a_multi_thread_runtime() {
        let db = ferris_db::memory_db().await.expect("in-memory db");
        assert!(super::oauth_config_from_doctypes(&db).is_none());
    }
}
