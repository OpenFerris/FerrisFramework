//! Native module composition.
//!
//! This is the only place that knows which verticals are compiled and enabled.
//! `ferris-api` owns the contract and the generic REST surface; a vertical owns
//! its routes and implements [`ferris_api::Module`]. The server builds the list
//! from the compile-time feature and the operator's `[features]` switch, then
//! hands it to the router builder.
//!
//! A vertical that is not compiled or is disabled by configuration is absent
//! from the registry, so its routes do not exist (404) and its `OpenAPI` paths are
//! not advertised. That is the honest projection: no handler pretends to work.

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use ferris_api::{ApiState, Module, ModuleRegistry, ModuleState};
use ferris_auth::AuthState;
use ferris_core::FeatureFlags;

/// Build the ordered module registry for this process.
pub fn native_modules(features: &FeatureFlags) -> ModuleRegistry {
    let mut modules: Vec<Arc<dyn Module>> = Vec::new();

    #[cfg(feature = "files")]
    if features.files {
        modules.push(Arc::new(ferris_files::FilesModule::default()));
    }

    #[cfg(feature = "print")]
    if features.print {
        modules.push(Arc::new(ferris_print::PrintModule::default()));
    }

    #[cfg(feature = "report")]
    if features.report {
        modules.push(Arc::new(ferris_report::ReportModule));
    }

    #[cfg(feature = "email")]
    if features.email {
        modules.push(Arc::new(ferris_email::EmailModule));
    }

    #[cfg(feature = "integrations")]
    modules.push(Arc::new(ferris_integrations::IntegrationsModule));

    ModuleRegistry::new(modules)
}

/// Mount every module's public router, outside the authenticated API.
///
/// A module opts in by returning a real prefix from `public_mount()`; the
/// default `/` is reserved so a module cannot accidentally register a
/// catch-all in front of the whole server. Routers that share a prefix merge
/// first, because axum allows one wildcard service per prefix.
pub(super) fn mount_public<S>(
    mut router: Router<S>,
    modules: &ModuleRegistry,
    state: &S,
) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    Arc<ApiState>: FromRef<S>,
    AuthState: FromRef<S>,
{
    let module_state = ModuleState::from_state(state);
    let mut mounts: Vec<(&'static str, Router<ModuleState>)> = Vec::new();
    for module in modules.modules() {
        let prefix = module.public_mount();
        if prefix == "/" {
            continue;
        }
        let public_routes = module.public_routes();
        if let Some((_, mounted)) = mounts.iter_mut().find(|(p, _)| *p == prefix) {
            let existing = std::mem::replace(mounted, Router::new());
            *mounted = existing.merge(public_routes);
        } else {
            mounts.push((prefix, public_routes));
        }
    }
    for (prefix, public_routes) in mounts {
        router = router.nest_service(prefix, public_routes.with_state(module_state.clone()));
    }
    router
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is the projection of the runtime switches: a compiled
    /// vertical appears only when the operator enables it. Integrations has no
    /// runtime flag (the compiled feature is the switch), so it stays.
    #[test]
    fn registry_follows_the_feature_switches() {
        let mut features = FeatureFlags::default();
        let enabled: Vec<&str> = native_modules(&features)
            .modules()
            .iter()
            .map(|module| module.name())
            .collect();
        #[cfg(feature = "files")]
        assert!(enabled.contains(&"files"), "{enabled:?}");
        #[cfg(feature = "print")]
        assert!(enabled.contains(&"print"), "{enabled:?}");
        #[cfg(feature = "report")]
        assert!(enabled.contains(&"report"), "{enabled:?}");
        #[cfg(feature = "email")]
        assert!(enabled.contains(&"email"), "{enabled:?}");
        #[cfg(feature = "integrations")]
        assert!(enabled.contains(&"integrations"), "{enabled:?}");

        features.files = false;
        features.print = false;
        features.report = false;
        features.email = false;
        let remaining: Vec<&str> = native_modules(&features)
            .modules()
            .iter()
            .map(|module| module.name())
            .collect();
        for disabled in ["files", "print", "report", "email"] {
            assert!(
                !remaining.contains(&disabled),
                "a disabled vertical must not be registered: {remaining:?}"
            );
        }
        #[cfg(feature = "integrations")]
        assert!(
            remaining.contains(&"integrations"),
            "integrations has no runtime switch: {remaining:?}"
        );
    }

    /// The modules that mount HTTP routes document them.
    #[test]
    fn the_routed_modules_document_their_paths() {
        let features = FeatureFlags::default();
        for module in native_modules(&features).modules() {
            if matches!(module.name(), "print" | "report") {
                assert!(
                    !module.openapi().paths.is_empty(),
                    "module `{}` mounts routes without documenting them",
                    module.name()
                );
            }
        }
    }
}
