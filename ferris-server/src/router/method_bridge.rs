//! The bridge between a guest's `data::call-method` and the API chain.
//!
//! The chain itself lives in [`ferris_api::MethodRegistry`] — one resolution
//! point for `/api/method`, JSON-RPC, batch and the file adapter. This module
//! only lets the WASM host run the entry *below* an app's own without making
//! `ferris-wasm` depend on the API layer: the server installs a process-wide
//! resolver at boot, exactly like the realtime broadcaster.
//!
//! A guest calls `data::call-method(name, params)` only through the WASM host,
//! which passes the calling app. A name that app does not override is refused
//! (403), so `super` can never be turned into "call any method".

use std::sync::Arc;

use async_trait::async_trait;
use ferris_api::MethodRegistry;
use ferris_wasm::MethodResolver;

/// The server's resolver: forwards to the live registry.
pub(super) struct ApiMethodResolver {
    methods: MethodRegistry,
}

impl ApiMethodResolver {
    pub(super) fn new(methods: MethodRegistry) -> Self {
        Self { methods }
    }
}

#[async_trait]
impl MethodResolver for ApiMethodResolver {
    async fn call_next(&self, app: &str, name: &str, params: &str) -> anyhow::Result<String> {
        let params: ferris_core::Value = serde_json::from_str(params)
            .map_err(|e| anyhow::anyhow!("method params are not JSON: {e}"))?;
        let out = self
            .methods
            .call_next(name, app, params)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        serde_json::to_string(&out).map_err(|e| anyhow::anyhow!("method result is not JSON: {e}"))
    }
}

/// Install (or replace) the process-wide resolver.
///
/// Replacing rather than setting once is deliberate: a test or a rebuild that
/// constructs a second router must re-point the bridge at *its* registry, not
/// leave the process bound to a registry that no longer serves requests.
pub(super) fn install(methods: MethodRegistry) {
    let _ = ferris_wasm::set_method_resolver(Arc::new(ApiMethodResolver::new(methods)));
}
