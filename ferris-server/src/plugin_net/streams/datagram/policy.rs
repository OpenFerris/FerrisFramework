//! Admission rules shared by every datagram call.

use anyhow::Result;
use ferris_wasm::Capability;
use ferris_wasm::host::BridgeError;

use super::ServerStreams;
use crate::plugin_net::egress::{EgressPolicy, is_wildcard_bind};

impl ServerStreams {
    /// The shared checks for `udp-connect`/`udp-listen`.
    pub(super) fn admit_udp(&self, app: &str, call: &str) -> Result<()> {
        if !self.granted(app, Capability::Udp) {
            return Err(BridgeError::unsupported(format!(
                "{call}: a datagram socket needs the `udp` capability"
            )));
        }
        if !self.granted(app, Capability::Streams) {
            return Err(BridgeError::unsupported(format!(
                "{call}: the socket surface needs the `streams` capability"
            )));
        }
        Ok(())
    }

    pub(super) fn admit_raw(&self, app: &str, call: &str) -> Result<()> {
        if !self.granted(app, Capability::RawSockets) {
            return Err(BridgeError::unsupported(format!(
                "{call}: a raw socket needs the `raw-sockets` capability"
            )));
        }
        if !self.granted(app, Capability::Streams) {
            return Err(BridgeError::unsupported(format!(
                "{call}: the socket surface needs the `streams` capability"
            )));
        }
        Ok(())
    }

    /// The egress policy of one datagram call, from the app's grants.
    ///
    /// A datagram spec has no per-call `allow-private` flag (the WIT does not
    /// offer one), so the operator grant `http-unrestricted` is both the
    /// allow-list bypass and the permission to reach loopback/RFC 1918. Without
    /// it the default allow-list applies and a private target is refused, which
    /// is the rule the byte streams and the HTTP client already enforce
    /// (WASM-005).
    pub(super) fn datagram_policy(&self, app: &str) -> EgressPolicy {
        let unrestricted = self.granted(app, Capability::HttpUnrestricted);
        EgressPolicy::from_env(unrestricted, unrestricted)
    }

    /// A wildcard bind (`0.0.0.0`/`::`) publishes the socket on every
    /// interface. That is an operator decision, so it needs
    /// `http-unrestricted`; loopback (the default) and an explicitly declared
    /// address need nothing extra.
    pub(super) fn admit_bind(&self, app: &str, host: &str, call: &str) -> Result<()> {
        if is_wildcard_bind(host) && !self.granted(app, Capability::HttpUnrestricted) {
            return Err(BridgeError::unsupported(format!(
                "{call}: binding `{host}` publishes the socket on every interface and needs \
                 `http-unrestricted`; bind `127.0.0.1` (the default) or declare one address"
            )));
        }
        Ok(())
    }
}
