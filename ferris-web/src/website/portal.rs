//! Stored public routes: the `Web Page`, `Web Form` and `Dashboard` rows that
//! answer a direct load, resolved from the database with a short-lived cache.
//!
//! The router talks to this module instead of querying the engine itself, so
//! the publication rule and the cache live with the models they serve. It is
//! deliberately HTTP-free: the caller decides how a [`PublicRoute`] becomes a
//! response, which is what keeps the lookup testable without a server.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use ferris_core::{Error, Value};
use ferris_db::Db;

use super::{WebForm, WebPage, normalize_route};

/// How long a stored route is trusted before the database is consulted again.
///
/// 30 seconds keeps a hot page from costing one query per request while still
/// making a Desk edit (publish, unpublish, rename) visible on the next page
/// load, not on the next restart. The cache is a lookup accelerator only: the
/// publication rule is applied by the model ([`WebPage::from_value`] /
/// [`WebForm::from_value`]) whether the row came from the cache or the engine.
const ROUTE_CACHE_TTL: Duration = Duration::from_secs(30);

/// Upper bound on cached routes. A site with more distinct public routes than
/// this is past the point where a process-wide map should keep growing.
const ROUTE_CACHE_MAX: usize = 512;

/// How many rows a case-insensitive route lookup may consider before it picks
/// the exact match. A CMS table is small, and a bounded scan keeps a mistaken
/// `like` from loading the whole table.
const ROUTE_MATCH_LIMIT: u32 = 25;

/// What a stored route serves.
#[derive(Clone)]
pub enum PublicRoute {
    /// A published `Web Page`, rendered server-side.
    Page(WebPage),
    /// A published `Web Form`, rendered on GET and submitted on POST.
    Form(WebForm),
    /// A Desk dashboard document: the client resolves it from the UI feed, so
    /// serving the SSR shell is enough to make a direct load work.
    Dashboard,
    /// Nothing public lives at the path.
    NotFound,
}

/// One cached resolution and the instant it was stored.
struct CachedRoute {
    route: PublicRoute,
    at: Instant,
}

static ROUTE_CACHE: LazyLock<RwLock<HashMap<String, CachedRoute>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// What the database stores at `path`, through the short-lived cache.
///
/// The cache only holds successful lookups: a storage failure must not pin the
/// route to "missing" for the whole TTL.
///
/// # Errors
///
/// The metadata/storage error of the lookup. A caller that answers 404 on it
/// must not cache that answer.
#[allow(clippy::result_large_err)] // `Error` is the crate-wide type; boxing it would leak into every caller.
pub async fn resolve_public_route(pool: &Db, path: &str) -> Result<PublicRoute, Error> {
    let key = normalize_route(path);
    if let Some(route) = cached_route(&key) {
        return Ok(route);
    }
    let route = lookup_stored(pool, &key).await?;
    remember_route(key, &route);
    Ok(route)
}

/// What the database stores at `route`, in precedence order.
#[allow(clippy::result_large_err)] // `Error` is the crate-wide type; boxing it would leak into every caller.
async fn lookup_stored(pool: &Db, route: &str) -> Result<PublicRoute, Error> {
    if let Some(row) = stored_row(pool, "Web Page", route, RouteMatch::Normalized).await?
        && let Some(page) = WebPage::from_value(&row)
    {
        return Ok(PublicRoute::Page(page));
    }
    if let Some(row) = stored_row(pool, "Web Form", route, RouteMatch::Normalized).await?
        && let Some(mut form) = WebForm::from_value(&row)
    {
        // The form's inputs come from the target `DocType`: `Web Form` has no
        // field list of its own, and the declared metadata is the only source
        // that stays correct when a field is customized.
        if let Some(meta) = ferris_api::effective_meta(pool, &form.doctype).await? {
            form = form.with_fields_from_meta(&meta);
        }
        return Ok(PublicRoute::Form(form));
    }
    // A Desk dashboard document: `Dashboard` rows are read as site-wide UI by
    // the feed, so their route only needs the shell to be server-rendered.
    if stored_row(
        pool,
        ferris_api::dashboards::DASHBOARD,
        route,
        RouteMatch::Exact,
    )
    .await?
    .is_some()
    {
        return Ok(PublicRoute::Dashboard);
    }
    Ok(PublicRoute::NotFound)
}

/// How a stored `route` is compared with the request path.
#[derive(Clone, Copy)]
enum RouteMatch {
    /// Byte-exact: a Desk dashboard route is matched by the client with `==`,
    /// so the server must not resolve a different path to it.
    Exact,
    /// Case-insensitive because [`normalize_route`] lowercases a CMS route: a
    /// page authored as `/About` is served at `/about` too. Only an exact match
    /// after normalisation counts, so `/about-us` never answers `/about`.
    Normalized,
}

/// The first row of `doctype` whose `route` matches the requested path, read as
/// the service principal.
///
/// `allow_anonymous(true)` is the explicit opt-in the singles store uses: the
/// row is public site configuration, not a caller's data, so the request must
/// not need a `Read` grant on a `Web Page`/`Web Form` (only System Manager has
/// one). The publication rule is *not* delegated to this flag — the model
/// checks `published` itself.
#[allow(clippy::result_large_err)] // `Error` is the crate-wide type; boxing it would leak into every caller.
async fn stored_row(
    pool: &Db,
    doctype: &str,
    route: &str,
    matching: RouteMatch,
) -> Result<Option<Value>, Error> {
    let meta = ferris_api::effective_meta(pool, doctype).await?;
    let series = meta
        .as_ref()
        .map(|meta| meta.naming_series.clone())
        .unwrap_or_default();
    let mut context = ferris_document::EngineContext::new(doctype, series).allow_anonymous(true);
    if let Some(meta) = &meta {
        context = context.with_meta(meta);
    }
    let mut query = ferris_document::ListQuery {
        limit: Some(ROUTE_MATCH_LIMIT),
        ..Default::default()
    };
    match matching {
        RouteMatch::Exact => {
            query.filters = HashMap::from([("route".to_string(), Value::string(route))]);
        }
        RouteMatch::Normalized => {
            // `like` is the case-insensitive substring operator; the exact
            // check below rejects a superset such as `/about-us` for `/about`.
            query.filter_ops = vec![ferris_document::ListFilter {
                field: "route".to_string(),
                op: ferris_document::FilterOp::Like,
                value: Value::string(route),
                or: false,
            }];
        }
    }
    let (rows, _) = ferris_document::DocumentEngine::new(pool.clone())
        .list(&context, &query)
        .await?;
    let rows: Vec<Value> = match matching {
        RouteMatch::Exact => rows,
        RouteMatch::Normalized => rows
            .into_iter()
            .filter(|row| {
                row.get("route")
                    .and_then(Value::as_str)
                    .is_some_and(|stored| normalized_route_matches(stored, route))
            })
            .collect(),
    };
    Ok(rows.into_iter().next())
}

/// Whether a stored CMS route and the normalised request path name the same
/// page. Only case and a trailing slash are insignificant: `/about-us` is not
/// `/about`.
fn normalized_route_matches(stored: &str, route: &str) -> bool {
    normalize_route(stored) == route
}

fn cached_route(key: &str) -> Option<PublicRoute> {
    let cache = ROUTE_CACHE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .get(key)
        .filter(|entry| entry.at.elapsed() < ROUTE_CACHE_TTL)
        .map(|entry| entry.route.clone())
}

fn remember_route(key: String, route: &PublicRoute) {
    let mut cache = ROUTE_CACHE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() >= ROUTE_CACHE_MAX {
        cache.retain(|_, entry| entry.at.elapsed() < ROUTE_CACHE_TTL);
        if cache.len() >= ROUTE_CACHE_MAX {
            cache.clear();
        }
    }
    cache.insert(
        key,
        CachedRoute {
            route: route.clone(),
            at: Instant::now(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_route_matches_case_insensitively_but_not_by_prefix() {
        assert!(normalized_route_matches("/About/", "/about"));
        assert!(normalized_route_matches("/about", "/about"));
        assert!(!normalized_route_matches("/about-us", "/about"));
        assert!(!normalized_route_matches("/blog", "/about"));
    }
}
