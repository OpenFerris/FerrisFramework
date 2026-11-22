//! `io` import bridge: everything the app does over the network.
//!
//! The host still owns every socket. What is real here is `http-send` (the
//! policy-checked outbound call) and the stream introspection/state calls the
//! server's transports feed. What is **not** wired yet — opening an outbound
//! stream, binding a listener, pushing bytes to a peer, closing it — fails with
//! the typed [`BridgeError::Unimplemented`], never with a fabricated `Ok`: the
//! server phase owns those sockets, and this bridge is the seam it plugs into.

use crate::ferris::framework::io as wit_io;
use crate::ferris::framework::types as wit_types;
use crate::host::conversions::{connect_spec_from_wit, raw_spec_from_wit};
use crate::host::{
    Capability, DnsZone, HostState, ListenerTransport, OutboundHttpRequest, OutboundTls,
    StreamInfo, StreamListenerSpec, TlsProtocol, UdpListenerSpec, UdpSpec,
};

use super::block_on;

/// Read a PEM file named by `tls-material.secret` from the app's world.
///
/// The path is relative to the world root and confined to it (`world_path`
/// refuses `..`, absolute paths and anything that resolves outside). The bytes
/// are never logged and never included in an error message: the error names the
/// path, not the material.
fn read_world_secret(world: &std::path::Path, path: &str) -> Result<String, wit_types::Error> {
    let relative = std::path::Path::new(path.trim());
    let file = ferris_core::plugins::world_path(world, relative).map_err(|e| {
        wit_types::Error::PermissionDenied(format!("tls-material.secret `{path}`: {e}"))
    })?;
    std::fs::read_to_string(&file)
        .map_err(|e| wit_types::Error::NotFound(format!("tls-material.secret `{path}`: {e}")))
}

impl crate::ferris::framework::io::Host for HostState {
    fn http_send(
        &mut self,
        request: wit_io::Request,
    ) -> Result<wit_io::Response, wit_types::Error> {
        // Outbound egress is a capability, not a default. `http` keeps the host
        // allow-list; `http-unrestricted` ignores it and only an app granted it
        // may widen the declaration, so a call asking to widen it is refused.
        self.require(Capability::Http)?;
        let unrestricted = self.grant_set().contains(Capability::HttpUnrestricted);
        if request.policy.as_ref().is_some_and(|p| p.allow_credentials) {
            // Forwarding `Authorization`/`Cookie` needs `http-unrestricted`,
            // exactly as `http-policy.allow-credentials` documents. The header
            // hygiene in the host service is what actually drops them.
            self.require(Capability::HttpUnrestricted)?;
        }
        if request.tls.is_some() {
            self.require(Capability::TlsClient)?;
        }
        let mut outbound = OutboundHttpRequest::from(request.clone());
        outbound.unrestricted = unrestricted;
        // Resolve the TLS material *here*: reading a `secret` key is a
        // capability check on the app's config store, and the transport that
        // builds the config has no database identity of its own.
        outbound.tls = self.resolve_outbound_tls(request.tls.as_ref())?;
        block_on(async { self.api().http_request(&outbound).await.map(Into::into) })
    }

    fn connect_stream(&mut self, spec: wit_io::ConnectSpec) -> Result<String, wit_types::Error> {
        self.require(Capability::Streams)?;
        if ListenerTransport::from(spec.transport).is_datagram() {
            // The contract is explicit: byte streams only, datagrams go through
            // `udp-connect`/`raw-open`. Refusing beats opening a stream socket
            // for a request that named UDP.
            return Err(wit_types::Error::ValidationFailed(
                "io::connect-stream carries byte streams only; use io::udp-connect".to_string(),
            ));
        }
        if spec.tls.is_some() {
            // A TLS stream is an outbound handshake with the app's material.
            self.require(Capability::TlsClient)?;
        }
        let outbound = connect_spec_from_wit(spec);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().streams_connect(&plugin, &outbound).await })
    }

    fn listen_stream(&mut self, spec: wit_types::StreamListen) -> Result<String, wit_types::Error> {
        // A listener is a specific capability by definition (`streams-listen`),
        // and a TLS listener additionally needs the app's `tls-server` material.
        self.require(Capability::StreamsListen)?;
        if ListenerTransport::from(spec.transport).is_datagram() {
            return Err(wit_types::Error::ValidationFailed(
                "io::listen-stream carries byte streams only; use io::udp-listen or io::raw-open"
                    .to_string(),
            ));
        }
        if spec.tls.is_some() {
            self.require(Capability::TlsServer)?;
        }
        let spec = StreamListenerSpec::from(spec);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().streams_listen(&plugin, &spec).await })
    }

    fn send_stream(&mut self, stream_id: String, bytes: Vec<u8>) -> Result<(), wit_types::Error> {
        self.require(Capability::Streams)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().streams_send(&plugin, &stream_id, &bytes).await })
    }

    fn close_stream(&mut self, stream_id: String) -> Result<(), wit_types::Error> {
        self.require(Capability::Streams)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().streams_close(&plugin, &stream_id).await })
    }

    fn stream_describe(
        &mut self,
        stream_id: String,
    ) -> Result<wit_io::StreamInfo, wit_types::Error> {
        self.require(Capability::Streams)?;
        block_on(async {
            self.api()
                .streams_info(&stream_id)
                .await
                .map(StreamInfo::into_wit)
        })
    }

    fn stream_read_state(&mut self, stream_id: String) -> Result<Vec<u8>, wit_types::Error> {
        self.require(Capability::Streams)?;
        block_on(async { self.api().streams_state(&stream_id).await })
    }

    fn stream_write_state(
        &mut self,
        stream_id: String,
        pending: Vec<u8>,
    ) -> Result<(), wit_types::Error> {
        self.require(Capability::Streams)?;
        block_on(async { self.api().streams_set_state(&stream_id, &pending).await })
    }

    // -- datagram sockets ---------------------------------------------------
    //
    // `udp`, `raw-sockets` and `dns-server` are specific capabilities: the
    // host opens the descriptor, so declaring one is both the request and the
    // grant. Nothing here binds a socket — the bridge forwards to
    // [`HostApi`], whose default answers the typed `unsupported` failure until
    // the server registers a datagram transport.

    fn udp_connect(&mut self, spec: wit_io::UdpSpec) -> Result<String, wit_types::Error> {
        self.require(Capability::Udp)?;
        let spec = UdpSpec::from(spec);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().udp_connect(&plugin, &spec).await })
    }

    fn udp_listen(&mut self, spec: wit_io::UdpListener) -> Result<String, wit_types::Error> {
        self.require(Capability::Udp)?;
        let spec = UdpListenerSpec::from(spec);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().udp_listen(&plugin, &spec).await })
    }

    fn udp_send(
        &mut self,
        stream_id: String,
        datagram: Vec<u8>,
        peer: Option<String>,
    ) -> Result<(), wit_types::Error> {
        self.require(Capability::Udp)?;
        let plugin = self.plugin_name.clone();
        block_on(async {
            self.api()
                .udp_send(&plugin, &stream_id, &datagram, peer.as_deref())
                .await
        })
    }

    fn udp_close(&mut self, stream_id: String) {
        // Idempotent by contract and infallible at the boundary (`udp-close`
        // has no `result`), but an ungranted capability is still not something
        // to ignore: the guest gets the typed failure through the next call it
        // makes. Closing a socket the app does not own must not succeed
        // silently, so the transport checks `app` scoping; here the capability
        // gate is the only decision the bridge can make.
        if self.require(Capability::Udp).is_err() {
            return;
        }
        let plugin = self.plugin_name.clone();
        let result = block_on(async { self.api().udp_close(&plugin, &stream_id).await });
        if let Err(error) = result {
            tracing::warn!(%error, %stream_id, "io::udp-close failed");
        }
    }

    fn raw_open(&mut self, spec: wit_io::RawSpec) -> Result<String, wit_types::Error> {
        self.require(Capability::RawSockets)?;
        let spec = raw_spec_from_wit(spec)?;
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().raw_open(&plugin, &spec).await })
    }

    fn raw_send(
        &mut self,
        stream_id: String,
        packet: Vec<u8>,
        destination: Option<String>,
    ) -> Result<(), wit_types::Error> {
        self.require(Capability::RawSockets)?;
        let plugin = self.plugin_name.clone();
        block_on(async {
            self.api()
                .raw_send(&plugin, &stream_id, &packet, destination.as_deref())
                .await
        })
    }

    fn raw_close(&mut self, stream_id: String) {
        // See `udp_close`: the wire call has no `result`, so the bridge logs
        // what the transport refused instead of claiming success.
        if self.require(Capability::RawSockets).is_err() {
            return;
        }
        let plugin = self.plugin_name.clone();
        let result = block_on(async { self.api().raw_close(&plugin, &stream_id).await });
        if let Err(error) = result {
            tracing::warn!(%error, %stream_id, "io::raw-close failed");
        }
    }

    fn dns_serve(&mut self, zone: wit_io::DnsZone) -> Result<String, wit_types::Error> {
        self.require(Capability::DnsServer)?;
        let zone = DnsZone::from(zone);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().dns_serve(&plugin, &zone).await })
    }

    fn dns_update(
        &mut self,
        listener_id: String,
        zone: wit_io::DnsZone,
    ) -> Result<(), wit_types::Error> {
        self.require(Capability::DnsServer)?;
        let zone = DnsZone::from(zone);
        let plugin = self.plugin_name.clone();
        block_on(async { self.api().dns_update(&plugin, &listener_id, &zone).await })
    }

    fn dns_stop(&mut self, listener_id: String) {
        // Idempotent and infallible at the wire; a refused stop is logged, not
        // silently swallowed.
        if self.require(Capability::DnsServer).is_err() {
            return;
        }
        let plugin = self.plugin_name.clone();
        let result = block_on(async { self.api().dns_stop(&plugin, &listener_id).await });
        if let Err(error) = result {
            tracing::warn!(%error, %listener_id, "io::dns-stop failed");
        }
    }

    fn publish(
        &mut self,
        channel: String,
        event_type: String,
        payload: String,
    ) -> Result<(), wit_types::Error> {
        self.require(Capability::Pubsub)?;
        // The declaration is enforcement, not documentation: once the app has
        // described itself, publishing to a channel it did not declare is
        // refused before the broadcaster sees it.
        if !self.may_publish(&channel) {
            return Err(wit_types::Error::PermissionDenied(format!(
                "channel `{channel}` is not declared in describe().subscriptions"
            )));
        }
        block_on(async {
            self.api()
                .pubsub_publish(&channel, &event_type, &payload)
                .await
        })
    }

    fn notify(
        &mut self,
        to: String,
        channel: String,
        title: String,
        message: String,
    ) -> Result<(), wit_types::Error> {
        self.require(Capability::Notifications)?;
        block_on(async {
            self.api()
                .notification_send(&to, &channel, &title, &message)
                .await
        })
    }
}

impl HostState {
    /// Resolve a declared `tls-client` policy into the host record the client
    /// builder consumes: every `tls-material.secret` becomes its stored PEM,
    /// which is what makes a rotating key a config change instead of a new
    /// `describe()`.
    ///
    /// # Errors
    ///
    /// `permission-denied` when the material lives behind `secrets` and the app
    /// does not hold it, `not-found` when the key was never provisioned, and
    /// `permission-denied` for `verify: false` without the specific
    /// `http-unrestricted` privilege (disabling verification is a deliberate
    /// widening, so it is declared and auditable).
    fn resolve_outbound_tls(
        &self,
        declared: Option<&wit_types::TlsClient>,
    ) -> Result<Option<OutboundTls>, wit_types::Error> {
        let Some(declared) = declared else {
            return Ok(None);
        };
        if !declared.verify && !self.grant_set().contains(Capability::HttpUnrestricted) {
            return Err(wit_types::Error::PermissionDenied(
                "tls-client `verify: false` needs the `http-unrestricted` capability".to_string(),
            ));
        }
        let cert = match declared.cert.as_ref() {
            Some(material) => Some(self.resolve_tls_material(material)?),
            None => None,
        };
        let key = match declared.key.as_ref() {
            Some(material) => Some(self.resolve_tls_material(material)?),
            None => None,
        };
        let ca = match declared.ca.as_ref() {
            Some(material) => Some(self.resolve_tls_material(material)?),
            None => None,
        };
        Ok(Some(OutboundTls {
            cert,
            key,
            ca,
            server_name: declared.server_name.clone(),
            alpn: declared.alpn.clone(),
            min_version: match declared.min_version {
                wit_types::TlsVersion::Tls12 => TlsProtocol::Tls12,
                wit_types::TlsVersion::Tls13 => TlsProtocol::Tls13,
            },
            verify: declared.verify,
        }))
    }

    /// One piece of TLS material as PEM: inline, or a file inside the app's
    /// world (`tls-material.secret` is a path such as `secrets/client.pem`).
    ///
    /// The app owns that directory, so the material rotates without a new
    /// `describe()` and never travels through a config row the host would have
    /// to make readable. The path is confined exactly like every other world
    /// access.
    fn resolve_tls_material(
        &self,
        material: &wit_types::TlsMaterial,
    ) -> Result<String, wit_types::Error> {
        match material {
            wit_types::TlsMaterial::Pem(pem) => Ok(pem.clone()),
            wit_types::TlsMaterial::Secret(path) => {
                let Some(world) = self.world() else {
                    return Err(wit_types::Error::PermissionDenied(
                        "tls-material.secret needs the `filesystem` capability".to_string(),
                    ));
                };
                read_world_secret(world, path)
            }
        }
    }
}
