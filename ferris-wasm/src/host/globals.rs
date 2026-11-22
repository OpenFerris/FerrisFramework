//! Process-wide host facilities shared by every plugin instance.

use anyhow::Result;
use async_trait::async_trait;

// -- Global host facilities --------------------------------------------------
//
// Some framework services are process-wide singletons the running server owns
// (the realtime broadcaster, the notification backends). They are registered
// once at server boot and shared by every plugin. Default: absent, and the
// `RealHostApi` degrades gracefully (logs, keeps the request flowing).

/// The live (re)transmission backend for `pubsub` imports. Production wiring
/// is a thin adapter over the framework's realtime server.
pub trait RealtimeBroadcaster: Send + Sync + 'static {
    /// Fan `payload` (JSON) out to the room backing `channel`.
    ///
    /// # Errors
    ///
    /// Returns an error if the payload cannot be serialised or the transport
    /// fails to deliver the event.
    fn publish(&self, channel: &str, event_type: &str, payload: &str) -> anyhow::Result<()>;
}

/// Multi-channel delivery backend for `notification` imports (email, push,
/// webhook). Left to the host installation to provide.
pub trait NotificationSender: Send + Sync + 'static {
    /// Deliver `title`/`message` to `to` over `channel`.
    ///
    /// # Errors
    ///
    /// Returns an error when the channel cannot deliver the notification.
    fn send(&self, to: &str, channel: &str, title: &str, message: &str) -> anyhow::Result<()>;
}

/// The framework's method chain, behind `data::call-method` (the explicit
/// `super`). The chain itself lives in `ferris-api` (it must stay the single
/// resolution point); this crate only forwards the guest's call, so
/// `ferris-wasm` never depends on the API layer.
#[async_trait]
pub trait MethodResolver: Send + Sync + 'static {
    /// Run the entry below `app`'s in the chain of `name`, JSON in/out.
    ///
    /// # Errors
    ///
    /// `not-found` when the method does not exist, `permission-denied` when
    /// `app` does not override it, or the handler's own failure.
    async fn call_next(&self, app: &str, name: &str, params: &str) -> anyhow::Result<String>;
}

static REALTIME: std::sync::OnceLock<std::sync::Arc<dyn RealtimeBroadcaster>> =
    std::sync::OnceLock::new();
static METHOD_RESOLVER: parking_lot::RwLock<Option<std::sync::Arc<dyn MethodResolver>>> =
    parking_lot::RwLock::new(None);
static NOTIFY: std::sync::OnceLock<std::sync::Arc<dyn NotificationSender>> =
    std::sync::OnceLock::new();

/// Register the process-wide pubsub broadcaster (once). Returns `false` if a
/// broadcaster was already registered.
pub fn set_realtime_broadcaster(broadcaster: std::sync::Arc<dyn RealtimeBroadcaster>) -> bool {
    REALTIME.set(broadcaster).is_ok()
}

/// Install (or replace) the process-wide method resolver.
///
/// Replacing is deliberate: a hot reload or a second router must re-point the
/// bridge at its own registry, not leave the process bound to one that no
/// longer serves requests.
pub fn set_method_resolver(resolver: std::sync::Arc<dyn MethodResolver>) {
    *METHOD_RESOLVER.write() = Some(resolver);
}

/// The registered method resolver, if any.
pub fn method_resolver() -> Option<std::sync::Arc<dyn MethodResolver>> {
    METHOD_RESOLVER.read().clone()
}

/// The registered broadcaster, if any.
pub fn realtime_broadcaster() -> Option<std::sync::Arc<dyn RealtimeBroadcaster>> {
    REALTIME.get().cloned()
}

/// Register the process-wide notification sender (once). Returns `false` if a
/// sender was already registered.
pub fn set_notification_sender(sender: std::sync::Arc<dyn NotificationSender>) -> bool {
    NOTIFY.set(sender).is_ok()
}

/// The registered notification sender, if any.
pub fn notification_sender() -> Option<std::sync::Arc<dyn NotificationSender>> {
    NOTIFY.get().cloned()
}

/// The host's internal certificate authority behind the `pki` imports.
///
/// The implementation owns the CA key and the revocation store; it lives in the
/// running server (`ferris-server`), not in this crate, so the WASM host stays
/// free of X.509 dependencies and the guest never sees the CA private key.
/// Registered once at boot; while absent the `pki` imports report the capability
/// as unavailable rather than fabricating a certificate or a revocation answer.
#[async_trait]
pub trait PkiProvider: Send + Sync + 'static {
    /// Sign a base64 DER PKCS#10 certificate request for `app` and return the
    /// issued certificate as PEM. `ttl_seconds` of `0` means "use the CA's
    /// default". The provider records `app` against the certificate's
    /// fingerprint, which is what scopes later revocation.
    async fn pki_sign_csr(&self, app: &str, csr_der_b64: &str, ttl_seconds: u32) -> Result<String>;
    /// The internal CA certificate in PEM.
    async fn pki_ca_cert(&self) -> Result<String>;
    /// Revoke the certificate carrying this lowercase hex SHA-256 fingerprint.
    /// Only the app that was recorded as its issuer may revoke it.
    async fn pki_revoke(&self, app: &str, fingerprint: &str) -> Result<()>;
    /// Whether the certificate carrying this fingerprint is revoked.
    async fn pki_is_revoked(&self, fingerprint: &str) -> Result<bool>;
}

static PKI: std::sync::OnceLock<std::sync::Arc<dyn PkiProvider>> = std::sync::OnceLock::new();

/// Register the process-wide internal-CA provider (once). Returns `false` if a
/// provider was already registered.
pub fn set_pki_provider(provider: std::sync::Arc<dyn PkiProvider>) -> bool {
    PKI.set(provider).is_ok()
}

/// The registered internal-CA provider, if any.
pub fn pki_provider() -> Option<std::sync::Arc<dyn PkiProvider>> {
    PKI.get().cloned()
}

// -- stream transport --------------------------------------------------------

/// The sockets behind the `io` stream imports.
///
/// The WASM host owns no socket: `connect-stream`/`listen-stream`/`send-stream`
/// are performed by the running server, which has the tokio reactor, the TLS
/// stack and the listener budget. This trait is the seam. The runtime forwards
/// every stream import here and, when no transport is registered, answers the
/// typed `unimplemented` error instead of pretending a connection exists.
///
/// The implementation is responsible for pumping bytes into the store
/// (`handle_stream`/`handle_stream_event`); this trait only opens, writes and
/// closes. `app` scopes every operation to the calling plugin so one app can
/// never write to another app's stream.
#[async_trait]
pub trait StreamService: Send + Sync + 'static {
    /// Open an outbound connection and return its `stream-id`.
    ///
    /// # Errors
    ///
    /// Returns an error when the transport, the target or the policy refuses the
    /// connection.
    async fn connect(&self, app: &str, spec: &super::OutboundStreamSpec) -> Result<String>;

    /// Bind a listener the app owns and return its listener id.
    ///
    /// # Errors
    ///
    /// Returns an error when the address cannot be bound, TLS material is
    /// missing or invalid, or the listener budget is exhausted.
    async fn listen(&self, app: &str, spec: &super::StreamListenerSpec) -> Result<String>;

    /// Write `bytes` to the peer of a live stream.
    ///
    /// # Errors
    ///
    /// Returns an error when the stream is unknown, belongs to another app, or
    /// the write fails.
    async fn send(&self, app: &str, stream_id: &str, bytes: &[u8]) -> Result<()>;

    /// Close a stream, or stop accepting on a listener id.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is unknown or belongs to another app.
    async fn close(&self, app: &str, stream_id: &str) -> Result<()>;

    // -- datagram sockets ---------------------------------------------------
    //
    // The same seam covers `io::udp-*`, `io::raw-*` and `io::dns-*`: the guest
    // asks, the host opens the descriptor, and inbound datagrams are pumped
    // into `handle-datagram`. Every default refuses with the typed
    // `unsupported` failure, so a transport that only carries byte streams
    // still answers honestly instead of fabricating a socket.

    /// Open a connected UDP socket for `app` and return its id.
    ///
    /// # Errors
    ///
    /// Returns an error when the peer or bind policy is refused.
    async fn udp_connect(&self, app: &str, spec: &super::UdpSpec) -> Result<String> {
        let _ = (app, spec);
        Err(super::error::BridgeError::unsupported(
            "io::udp-connect (this stream transport carries no datagrams)",
        ))
    }

    /// Bind a UDP listener for `app` and return its id.
    ///
    /// # Errors
    ///
    /// Returns an error when the address cannot be bound or the budget is
    /// exhausted.
    async fn udp_listen(&self, app: &str, spec: &super::UdpListenerSpec) -> Result<String> {
        let _ = (app, spec);
        Err(super::error::BridgeError::unsupported(
            "io::udp-listen (this stream transport carries no datagrams)",
        ))
    }

    /// Send one datagram on a socket or listener `app` owns.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is unknown, belongs to another app, or the
    /// datagram is refused.
    async fn udp_send(
        &self,
        app: &str,
        stream_id: &str,
        datagram: &[u8],
        peer: Option<&str>,
    ) -> Result<()> {
        let _ = (app, stream_id, datagram, peer);
        Err(super::error::BridgeError::unsupported(
            "io::udp-send (this stream transport carries no datagrams)",
        ))
    }

    /// Close a UDP socket or stop accepting on a UDP listener.
    ///
    /// # Errors
    ///
    /// Returns an error when the id belongs to another app.
    async fn udp_close(&self, app: &str, stream_id: &str) -> Result<()> {
        let _ = (app, stream_id);
        Err(super::error::BridgeError::unsupported(
            "io::udp-close (this stream transport carries no datagrams)",
        ))
    }

    /// Open a raw/ICMP socket for `app` and return its id.
    ///
    /// # Errors
    ///
    /// Returns an error when the address or IP protocol is refused.
    async fn raw_open(&self, app: &str, spec: &super::RawSpec) -> Result<String> {
        let _ = (app, spec);
        Err(super::error::BridgeError::unsupported(
            "io::raw-open (this stream transport carries no raw sockets)",
        ))
    }

    /// Send one guest-built packet on a raw socket `app` owns.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is unknown, belongs to another app, or the
    /// packet is refused.
    async fn raw_send(
        &self,
        app: &str,
        stream_id: &str,
        packet: &[u8],
        destination: Option<&str>,
    ) -> Result<()> {
        let _ = (app, stream_id, packet, destination);
        Err(super::error::BridgeError::unsupported(
            "io::raw-send (this stream transport carries no raw sockets)",
        ))
    }

    /// Close a raw socket.
    ///
    /// # Errors
    ///
    /// Returns an error when the id belongs to another app.
    async fn raw_close(&self, app: &str, stream_id: &str) -> Result<()> {
        let _ = (app, stream_id);
        Err(super::error::BridgeError::unsupported(
            "io::raw-close (this stream transport carries no raw sockets)",
        ))
    }

    /// Serve `zone` authoritatively and return the listener id.
    ///
    /// # Errors
    ///
    /// Returns an error when the zone cannot be encoded or the port cannot be
    /// bound.
    async fn dns_serve(&self, app: &str, zone: &super::DnsZone) -> Result<String> {
        let _ = (app, zone);
        Err(super::error::BridgeError::unsupported(
            "io::dns-serve (this stream transport carries no DNS server)",
        ))
    }

    /// Replace the records of a live DNS listener atomically.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is unknown or belongs to another app.
    async fn dns_update(&self, app: &str, listener_id: &str, zone: &super::DnsZone) -> Result<()> {
        let _ = (app, listener_id, zone);
        Err(super::error::BridgeError::unsupported(
            "io::dns-update (this stream transport carries no DNS server)",
        ))
    }

    /// Stop answering for a DNS listener.
    ///
    /// # Errors
    ///
    /// Returns an error when the id belongs to another app.
    async fn dns_stop(&self, app: &str, listener_id: &str) -> Result<()> {
        let _ = (app, listener_id);
        Err(super::error::BridgeError::unsupported(
            "io::dns-stop (this stream transport carries no DNS server)",
        ))
    }
}

static STREAMS: std::sync::OnceLock<std::sync::Arc<dyn StreamService>> = std::sync::OnceLock::new();

/// Register the process-wide stream transport (once). Returns `false` if one
/// was already registered.
pub fn set_stream_service(transport: std::sync::Arc<dyn StreamService>) -> bool {
    STREAMS.set(transport).is_ok()
}

/// The registered stream transport, if any.
pub fn stream_service() -> Option<std::sync::Arc<dyn StreamService>> {
    STREAMS.get().cloned()
}

// -- host programs -----------------------------------------------------------

/// The process supervisor behind the `platform::process-*` imports.
///
/// The WASM host owns no child process: `process-run`/`spawn`/`wait`/`kill`
/// fork, pipe, enforce the deadline and reap inside the running server, which
/// is the only place that can attribute a process tree to one app and clean it
/// up with the instance. This trait is the seam. The runtime forwards every
/// process import here and, when no supervisor is registered, answers the
/// typed `unsupported` failure instead of pretending a program ran.
///
/// `app` scopes every operation to the calling plugin: a plugin may only wait
/// for or kill a process it started.
#[async_trait]
pub trait ProcessService: Send + Sync + 'static {
    /// Run a program to completion and return what it wrote.
    ///
    /// # Errors
    ///
    /// Returns an error when the program cannot be resolved inside the app's
    /// world, the deadline fires, the output cap is exceeded, or the spawn
    /// fails.
    async fn run(&self, app: &str, spec: &super::ProcessSpec) -> Result<super::ProcessOutput>;

    /// Start a program without waiting and return its id.
    ///
    /// # Errors
    ///
    /// Returns an error when the program cannot be resolved or the spawn
    /// fails.
    async fn spawn(&self, app: &str, spec: &super::ProcessSpec) -> Result<String>;

    /// Wait for a spawned program and hand its output over once.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is unknown, was already consumed, or
    /// belongs to another app.
    async fn wait(&self, app: &str, process_id: &str) -> Result<super::ProcessOutput>;

    /// Stop a spawned program and the children attributed to it.
    ///
    /// # Errors
    ///
    /// Returns an error when the id is unknown or belongs to another app.
    async fn kill(&self, app: &str, process_id: &str) -> Result<()>;
}

static PROCESS: std::sync::OnceLock<std::sync::Arc<dyn ProcessService>> =
    std::sync::OnceLock::new();

/// Register the process-wide supervisor (once). Returns `false` if one was
/// already registered.
pub fn set_process_service(service: std::sync::Arc<dyn ProcessService>) -> bool {
    PROCESS.set(service).is_ok()
}

/// The registered process supervisor, if any.
pub fn process_service() -> Option<std::sync::Arc<dyn ProcessService>> {
    PROCESS.get().cloned()
}
