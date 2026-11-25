//! Process start-up: build the dev state, the router and the listener.

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    Router, middleware,
    routing::{get, post},
};
use ferris_api::{
    ApiState, SessionHandlers, api_router_with_app_state, login, logout, logout_all, session_me,
    set_language, verify_mfa,
};
use ferris_auth::login::{Backend, DbSessionStore, SessionConfig};
use ferris_auth::{ApiKeyManager, AuthState, UserStore, require_auth, require_csrf};
use ferris_wasm::{PluginStore, RuntimeConfig, SharedRuntime};
use leptos::config::get_configuration;
use leptos::prelude::*;
use leptos_axum::{LeptosRoutes, generate_route_list};
use tower_http::services::ServeDir;
use tracing::info;

use super::assets::{desk_css, pkg_no_store, preview_asset, vendor_asset};
use super::auth::{
    bind_ambient_user, database_config_from_env, health, provide_desk_context, seed_dev_admin,
};
use super::plugin::{
    bind_plugin_user, load_app_plugin, plugin_describe, plugin_handle_request,
    register_plugin_methods, reload_app,
};
use super::state::{DevState, ReloadState};

pub(super) async fn run() -> Result<()> {
    let conf = get_configuration(None)?;
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;
    let site_root = PathBuf::from(leptos_options.site_root.as_ref().to_string());
    let pkg_dir = site_root.join(leptos_options.site_pkg_dir.as_ref().to_string());

    // Config comes from the `ferris dev --hot-reload front` supervisor, which
    // resolves the project's `config/ferris.toml` and forwards it here.
    let database = database_config_from_env()?;

    let pool = ferris_db::create_pool(&database).await?;
    seed_dev_admin(&pool).await;

    // The one session stack, identical to `ferris-server` (plain HTTP in dev,
    // so the cookie must not be `Secure` or browsers would drop it).
    //
    // This is the SSR **dev** binary launched by `ferris dev --hot-reload
    // front`; production traffic is served by `ferris-server`, which derives
    // `Secure` from TLS or an `https://` external URL. Keep the dev profile
    // insecure on purpose.
    let session_config = SessionConfig::default().with_secure(false);
    let session_store = DbSessionStore::new(pool.clone());
    session_store.ensure_table().await?;
    let _ = session_store.spawn_cleanup_task(std::time::Duration::from_secs(600));
    let backend = Backend::new(pool.clone());
    backend.init().await?;
    let api_key_manager = ApiKeyManager::with_db(pool.clone());
    api_key_manager.init().await?;
    let auth_state = AuthState::new(session_config.clone())
        .with_session_store(session_store.clone())
        // Resolve API-key owners (roles included) from the users table so a key
        // authenticates as the real caller.
        .with_user_validator(Arc::new(UserStore::new(pool.clone())))
        // Accept the Frappe `Authorization: token <key>:<secret>` scheme (AUTH2).
        .with_api_keys(Arc::new(api_key_manager));
    let session_handlers =
        SessionHandlers::new(session_config.clone(), session_store.clone(), pool.clone())
            .with_auth_config(&ferris_core::AuthConfig::default());

    // Framework-owned DocTypes are available on every boot, not only when an
    // app declares them. Each crate declares its own; `ferris-web`'s ssr binary
    // wires the ones it can reach (the API shell's, via `ferris-api`).
    ferris_api::doctypes::register_doctypes();

    // App plugin store: hosts the project's DocTypes so the Desk can list them.
    let runtime = Arc::new(SharedRuntime::new(RuntimeConfig {
        enable_epoch_interrupts: false, // dev mode: no epoch clock
        // The reload endpoint swaps the same component repeatedly; without the
        // cache each swap re-ran cranelift over the whole artifact.
        compile_cache: true,
        ..Default::default()
    })?);
    let plugins = Arc::new(
        PluginStore::new(runtime, pool.clone()).with_worlds(ferris_core::plugins::plugins_root()),
    );

    let state = DevState {
        leptos: leptos_options.clone(),
        api: ApiState::new(pool.clone()).with_auth_config(&ferris_core::AuthConfig::default()),
        auth: auth_state.clone(),
        session: session_handlers,
        plugins: plugins.clone(),
        reload: ReloadState {
            app: load_app_plugin(&plugins).await?,
            token: env::var("FERRIS_DEV_RELOAD_TOKEN").unwrap_or_default(),
        },
    };

    // One `/api/method/<name>` handler per method the loaded plugin declares
    // in `describe()` — the `ferris-server` twin. The route itself already
    // comes from `api_router_with_app_state`; the registry is what makes the
    // call reach the plugin. Handlers re-verify the descriptor per call, so a
    // reload that drops a method answers 404 instead of hitting a stale one.
    register_plugin_methods(&state.api, &plugins);

    // Public session endpoints live OUTSIDE the `require_auth` layer. Logout
    // carries the CSRF gate: a cross-site form must not be able to
    // log a user out; login/session/verify-mfa stay open because there is no
    // session yet to hold a token.
    let auth_routes: Router<DevState> = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/session", get(session_me))
        .route("/api/auth/mfa/verify", post(verify_mfa))
        .merge(
            Router::new()
                .route("/api/auth/logout", post(logout))
                .route("/api/auth/logout-all", post(logout_all))
                .route("/api/auth/language", axum::routing::put(set_language))
                .route_layer(middleware::from_fn_with_state(
                    auth_state.clone(),
                    require_csrf,
                )),
        );

    // REST document store: `require_auth` (outermost) authenticates; then
    // `bind_ambient_user` scopes the resolved user for the metadata engine so
    // document writes authorise as the caller (the engine fails closed).
    let api_routes: Router<DevState> = api_router_with_app_state(state.clone())
        .layer(middleware::from_fn_with_state(
            auth_state.clone(),
            bind_ambient_user,
        ))
        .layer(middleware::from_fn_with_state(
            auth_state.clone(),
            require_auth,
        ));

    let route_list = generate_route_list(|| view! { <ferris_web::DeskShell /> });

    // Plugin routes (dev): same shape as `ferris-server` — a nested router so
    // `bind_plugin_user` scopes the optional desk session to plugin traffic
    // only. Anonymous callers stay guests instead of being rejected, while a
    // logged-in caller gets `session::current` reporting the real user.
    let plugin_routes: Router<DevState> = Router::new()
        .route("/{name}", get(plugin_describe))
        .route(
            "/{name}/{*rest}",
            get(plugin_handle_request).post(plugin_handle_request),
        )
        .layer(middleware::from_fn_with_state(
            auth_state.clone(),
            bind_plugin_user,
        ));

    // Dev-only server: the hydrate bundle is rebuilt in place on every change,
    // so the browser must re-fetch it instead of reusing the previous
    // `ferris-web.js` (which would be paired with the new wasm and abort with
    // `LinkError: import object field ... is not a Function`). See
    // `pkg_no_store` in `ferris-server` for the full reasoning.
    let pkg_service = Router::new()
        .fallback_service(ServeDir::new(&pkg_dir))
        .layer(middleware::from_fn(pkg_no_store));

    let app = Router::new()
        .route("/health", get(health))
        .merge(auth_routes)
        .merge(api_routes)
        .route("/__dev/reload-app", post(reload_app))
        .nest("/plugins", plugin_routes)
        .route("/static/desk.css", get(desk_css))
        // Committed third-party libraries, compiled into this binary
        // (`include_bytes!` — cargo rebuilds on change, so `npm run vendor`
        // hot-reloads like an edited Rust file).
        .route("/static/vendor/{*path}", get(vendor_asset))
        // Widget-catalog previews, same embedded-table treatment.
        .route("/static/previews/{*path}", get(preview_asset))
        .leptos_routes_with_context(
            &state,
            route_list,
            {
                let pool = pool.clone();
                move || provide_desk_context(pool.clone())
            },
            || view! { <ferris_web::DeskShell /> },
        )
        .nest_service("/pkg", pkg_service)
        .fallback_service(ServeDir::new(&site_root))
        // The one session stack, outside every gate: the login route needs the
        // session record, and `require_auth` reads `AuthSession` from the
        // request extensions it installs.
        .layer(ferris_auth::login::auth_layer(
            &session_config,
            session_store,
            backend,
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("ferris-web SSR dev server listening on http://{addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
