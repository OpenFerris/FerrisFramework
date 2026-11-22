//! Turning a declared `tls-client`/`tls-server` into a rustls config.
//!
//! Resolving `tls-material.secret` is [`material`](crate::plugin_net::material)'s
//! job and is capability-checked there; this module only assembles the policy
//! the handshake runs with.

use std::sync::Arc;

use anyhow::Result;
use ferris_wasm::{OutboundTls, TlsClient, TlsProtocol, TlsServer};

use super::ServerStreams;
use crate::plugin_net::{material, tls_routes};

impl ServerStreams {
    /// Resolve a declared `tls-client` into the host TLS policy.
    pub(super) async fn client_tls(&self, app: &str, tls: &TlsClient) -> Result<OutboundTls> {
        let cert = match tls.cert.as_ref() {
            Some(item) => {
                Some(material::resolve(&self.core.store, app, item, "tls-client.cert").await?)
            }
            None => None,
        };
        let key = match tls.key.as_ref() {
            Some(item) => {
                Some(material::resolve(&self.core.store, app, item, "tls-client.key").await?)
            }
            None => None,
        };
        let ca = match tls.ca.as_ref() {
            Some(item) => {
                Some(material::resolve(&self.core.store, app, item, "tls-client.ca").await?)
            }
            None => None,
        };
        Ok(OutboundTls {
            cert,
            key,
            ca,
            server_name: tls.server_name.clone(),
            alpn: tls.alpn.clone(),
            min_version: match tls.min_version {
                ferris_wasm::TlsVersion::Tls12 => TlsProtocol::Tls12,
                ferris_wasm::TlsVersion::Tls13 => TlsProtocol::Tls13,
            },
            verify: tls.verify,
        })
    }

    /// Resolve a declared `tls-server` into a rustls server config.
    pub(super) async fn server_tls(
        &self,
        app: &str,
        tls: &TlsServer,
    ) -> Result<Arc<rustls::ServerConfig>> {
        let cert = material::resolve(&self.core.store, app, &tls.cert, "tls-server.cert").await?;
        let key = material::resolve(&self.core.store, app, &tls.key, "tls-server.key").await?;
        let ca = match tls.ca.as_ref() {
            Some(item) => {
                Some(material::resolve(&self.core.store, app, item, "tls-server.ca").await?)
            }
            None => None,
        };
        tls_routes::build_server_config(
            &cert,
            &key,
            ca.as_deref(),
            tls.client_auth,
            &tls.sni,
            &tls.alpn,
            &tls.min_version,
        )
    }
}
