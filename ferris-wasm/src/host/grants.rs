//! Capability grants: what an app may reach, without operator configuration.
//!
//! A grant is computed from two lists:
//!
//! * **Primordial** — always granted, not configurable: the app's world
//!   (`filesystem`, which *is* its files, blobs and `secrets/.env`), its own
//!   HTTP routes (`routes`, including HTTPS served by the host), outbound HTTP
//!   under policy (`http`), the document store, config, schema, scheduler,
//!   notifications and pubsub.
//! * **Specific** — granted when the app declares them: the egress privileges
//!   that widen a host boundary (`http-unrestricted`, `tls-client`,
//!   `tls-server`, `streams`, `streams-listen`, `udp`, `raw-sockets`,
//!   `dns-server`, `process`, `threads`, `pki`) and `overrides` (replacing a
//!   framework API). `threads` is specific because a thread pool multiplies the
//!   guest's memory and CPU footprint; it has no import and is reported only
//!   (`platform::grants`, `platform::features`).
//!
//! There is no `[apps] trusted/capabilities/grants` table: declaring a specific
//! capability is the request *and* the grant, and every refusal is the host's
//! own enforcement (the capability gates the import). The operator config is
//! not a second policy system.

use std::collections::BTreeSet;

use crate::ferris::framework::types as wit;

/// One WIT `types.capability`, mirrored host-side so [`HostApi`](super::HostApi)
/// and the bridges never depend on the generated bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// Declare routes and receive them in `handle-request`.
    Routes,
    /// Outbound HTTP under the host's policy.
    Http,
    /// Outbound HTTP ignoring the allow-list.
    HttpUnrestricted,
    /// Outbound TLS with client material.
    TlsClient,
    /// Serve TLS/mTLS with the app's own material.
    TlsServer,
    /// Open outbound streams and handle the inbound ones.
    Streams,
    /// Bind a listener the app owns.
    StreamsListen,
    /// Read documents, list/count/aggregate, read schema metadata.
    DocstoreRead,
    /// Write documents and allocate naming-series numbers.
    DocstoreWrite,
    /// Read and write the app's own world directory (`plugins/<app>/`).
    Filesystem,
    /// Declare a method that overrides an existing one.
    Overrides,
    /// Sign CSRs with the host CA and revoke what the app issued.
    Pki,
    /// Schedules and one-shot jobs.
    Scheduler,
    /// The app's non-secret config keys.
    Config,
    /// `DocType` metadata introspection.
    Schema,
    /// Alerts and messages.
    Notifications,
    /// The app's declared pubsub channels.
    Pubsub,
    /// UDP datagrams through the host (`io::udp-connect`/`udp-listen`/…).
    Udp,
    /// One authoritative DNS zone the host binds and answers.
    DnsServer,
    /// Raw/ICMP sockets the host opens for guest-built packets.
    RawSockets,
    /// Programs the host runs on the app's behalf.
    Process,
    /// The guest's own threads (the wasm threads proposal); reported, no import.
    Threads,
}

impl Capability {
    /// Every capability, in the WIT declaration order.
    pub const ALL: [Self; 22] = [
        Self::Routes,
        Self::Http,
        Self::HttpUnrestricted,
        Self::TlsClient,
        Self::TlsServer,
        Self::Streams,
        Self::StreamsListen,
        Self::DocstoreRead,
        Self::DocstoreWrite,
        Self::Filesystem,
        Self::Overrides,
        Self::Pki,
        Self::Scheduler,
        Self::Config,
        Self::Schema,
        Self::Notifications,
        Self::Pubsub,
        Self::Udp,
        Self::DnsServer,
        Self::RawSockets,
        Self::Process,
        Self::Threads,
    ];

    /// The WIT kebab-case name, as it appears in `permission-denied` messages
    /// and the `/plugins/{name}` projection.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Routes => "routes",
            Self::Http => "http",
            Self::HttpUnrestricted => "http-unrestricted",
            Self::TlsClient => "tls-client",
            Self::TlsServer => "tls-server",
            Self::Streams => "streams",
            Self::StreamsListen => "streams-listen",
            Self::DocstoreRead => "docstore-read",
            Self::DocstoreWrite => "docstore-write",
            Self::Filesystem => "filesystem",
            Self::Overrides => "overrides",
            Self::Pki => "pki",
            Self::Scheduler => "scheduler",
            Self::Config => "config",
            Self::Schema => "schema",
            Self::Notifications => "notifications",
            Self::Pubsub => "pubsub",
            Self::Udp => "udp",
            Self::DnsServer => "dns-server",
            Self::RawSockets => "raw-sockets",
            Self::Process => "process",
            Self::Threads => "threads",
        }
    }

    /// Parse a capability name. Exact, case-sensitive kebab-case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.name() == name)
    }

    /// Always granted, never configurable: the app's own world and the
    /// framework services that never leave the process, plus the routes and
    /// outbound HTTP it needs to be an app at all.
    ///
    /// `filesystem` is primordial and not just a convenience: the app owns its
    /// directory (files, blobs, `secrets/.env`) and the preopen is the boundary.
    #[must_use]
    pub const fn primordial(self) -> bool {
        matches!(
            self,
            Self::Routes
                | Self::Http
                | Self::DocstoreRead
                | Self::DocstoreWrite
                | Self::Filesystem
                | Self::Config
                | Self::Schema
                | Self::Scheduler
                | Self::Notifications
                | Self::Pubsub
        )
    }

    /// Granted only when the app declares it: egress that widens a host
    /// boundary (sockets, DNS, programs, threads) and replacing a framework
    /// API.
    #[must_use]
    pub const fn specific(self) -> bool {
        !self.primordial()
    }

    /// The host-side mirror of a declared WIT capability.
    #[must_use]
    pub const fn from_wit(capability: wit::Capability) -> Self {
        match capability {
            wit::Capability::Routes => Self::Routes,
            wit::Capability::Http => Self::Http,
            wit::Capability::HttpUnrestricted => Self::HttpUnrestricted,
            wit::Capability::TlsClient => Self::TlsClient,
            wit::Capability::TlsServer => Self::TlsServer,
            wit::Capability::Streams => Self::Streams,
            wit::Capability::StreamsListen => Self::StreamsListen,
            wit::Capability::DocstoreRead => Self::DocstoreRead,
            wit::Capability::DocstoreWrite => Self::DocstoreWrite,
            wit::Capability::Filesystem => Self::Filesystem,
            wit::Capability::Overrides => Self::Overrides,
            wit::Capability::Pki => Self::Pki,
            wit::Capability::Scheduler => Self::Scheduler,
            wit::Capability::Config => Self::Config,
            wit::Capability::Schema => Self::Schema,
            wit::Capability::Notifications => Self::Notifications,
            wit::Capability::Pubsub => Self::Pubsub,
            wit::Capability::Udp => Self::Udp,
            wit::Capability::DnsServer => Self::DnsServer,
            wit::Capability::RawSockets => Self::RawSockets,
            wit::Capability::Process => Self::Process,
            wit::Capability::Threads => Self::Threads,
        }
    }

    /// The WIT capability this one mirrors, for `platform::grants`.
    #[must_use]
    pub const fn to_wit(self) -> wit::Capability {
        match self {
            Self::Routes => wit::Capability::Routes,
            Self::Http => wit::Capability::Http,
            Self::HttpUnrestricted => wit::Capability::HttpUnrestricted,
            Self::TlsClient => wit::Capability::TlsClient,
            Self::TlsServer => wit::Capability::TlsServer,
            Self::Streams => wit::Capability::Streams,
            Self::StreamsListen => wit::Capability::StreamsListen,
            Self::DocstoreRead => wit::Capability::DocstoreRead,
            Self::DocstoreWrite => wit::Capability::DocstoreWrite,
            Self::Filesystem => wit::Capability::Filesystem,
            Self::Overrides => wit::Capability::Overrides,
            Self::Pki => wit::Capability::Pki,
            Self::Scheduler => wit::Capability::Scheduler,
            Self::Config => wit::Capability::Config,
            Self::Schema => wit::Capability::Schema,
            Self::Notifications => wit::Capability::Notifications,
            Self::Pubsub => wit::Capability::Pubsub,
            Self::Udp => wit::Capability::Udp,
            Self::DnsServer => wit::Capability::DnsServer,
            Self::RawSockets => wit::Capability::RawSockets,
            Self::Process => wit::Capability::Process,
            Self::Threads => wit::Capability::Threads,
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The effective grant of one app: the primordial set plus the specific
/// capabilities it declared.
///
/// Built once per instantiation and re-installed on every `describe()`, so a
/// reload re-computes it from the component that is actually running. There is
/// no operator input and no profile: `for_declaration` is the whole policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantSet {
    capabilities: BTreeSet<Capability>,
}

impl GrantSet {
    /// The primordial set (an app that has declared nothing yet).
    #[must_use]
    pub fn primordial() -> Self {
        let capabilities = Capability::ALL
            .into_iter()
            .filter(|capability| capability.primordial())
            .collect();
        Self { capabilities }
    }

    /// Keep the primordial set and add the specific capabilities the app
    /// declared.
    #[must_use]
    pub fn for_declaration(declared: impl IntoIterator<Item = Capability>) -> Self {
        let mut grants = Self::primordial();
        for capability in declared {
            grants.capabilities.insert(capability);
        }
        grants
    }

    /// Whether the app has `capability`.
    #[must_use]
    pub fn contains(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// The effective set, for `platform::grants`.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.capabilities.iter().copied()
    }

    /// Replace the set (used by the descriptor re-install and by tests).
    pub fn set(&mut self, capabilities: impl IntoIterator<Item = Capability>) {
        self.capabilities = capabilities.into_iter().collect();
    }

    /// The effective grant as the WIT record `platform::grants` returns.
    #[must_use]
    pub fn to_wit(&self) -> wit::GrantSet {
        wit::GrantSet {
            capabilities: self
                .capabilities
                .iter()
                .map(|capability| capability.to_wit())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests;
