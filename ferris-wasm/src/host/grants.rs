//! Capability grants: what an app may reach, and who approved it.
//!
//! A grant has two halves:
//!
//! * **The manifest** ([`GrantSet`]) is the app's *request*. It is computed
//!   from the WIT declaration and has two tiers:
//!   - **Primordial** — always granted, not configurable: the app's world
//!     (`filesystem`, which *is* its files, blobs and `secrets/.env`), its own
//!     HTTP routes (`routes`, including HTTPS served by the host), outbound HTTP
//!     under policy (`http`), the document store, config, schema, scheduler,
//!     notifications and pubsub. Primordial capabilities are gated only by the
//!     manifest: they keep the pre-trust rule and need no operator approval.
//!   - **Specific** — requested when the app declares them: the egress
//!     privileges that widen a host boundary (`http-unrestricted`,
//!     `tls-client`, `tls-server`, `streams`, `streams-listen`, `udp`,
//!     `raw-sockets`, `dns-server`, `process`, `threads`, `pki`) and
//!     `overrides` (replacing a framework API). `threads` is specific because a
//!     thread pool multiplies the guest's memory and CPU footprint; it has no
//!     import and is reported only (`platform::grants`, `platform::features`).
//! * **The approval** ([`ApprovedGrants`]) is the operator's decision, written
//!   by `ferris app install` to `grants.toml`. Every specific capability must
//!   appear in both halves; a manifest request alone is not a grant.
//!
//! Primordial capabilities deliberately skip the approval half. They are the
//! services an app needs to exist at all (world, routes, docstore, config,
//! schema, scheduler, notifications, pubsub) plus outbound HTTP under the
//! host's policy; making an operator approve them per app would only duplicate
//! the host's own policy. A missing `grants.toml` therefore fails **closed for
//! specific capabilities only**, naming the app and the files the host looked
//! for.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::ferris::framework::types as wit;
use crate::trust::{self, TrustError};

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
    /// Primordial capabilities are gated by the manifest alone; they are the
    /// one exception to the operator approval in [`ApprovedGrants`], because
    /// the host's own policy already bounds them; [`HostState::require`]
    /// documents and enforces the split.
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
/// reload re-computes it from the component that is actually running. This is
/// the app's **request**; a specific capability is not usable until
/// [`ApprovedGrants`] names it too.
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

/// The operator-approved grant set for one app, loaded from `grants.toml`.
///
/// The manifest is the app's request ([`GrantSet`]); this is the operator's
/// decision. Every non-primordial capability must appear in both, and outbound
/// HTTP hosts must be approved here in addition to the manifest's host list.
///
/// `grants: None` means no file was found: nothing is approved. That is the
/// fail-closed default for an app installed before trust existed, or one whose
/// approval was removed; it is never an implicit grant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovedGrants {
    grants: Option<trust::GrantSet>,
    /// Files consulted, in preference order (root, then `world/` fallback).
    candidates: Vec<PathBuf>,
    /// The file the approval was read from, when one was found.
    loaded_from: Option<PathBuf>,
}

impl ApprovedGrants {
    /// Load the approval for the app whose directory is `app_dir`.
    ///
    /// Prefers `<app dir>/grants.toml` and falls back to
    /// `<app dir>/world/grants.toml`, the two layouts `ferris app install` and
    /// `ferris site install-app` produce. A missing file is an empty approval
    /// (the [`trust::GrantSet::load`] semantics), never an error and never a
    /// grant; a file that exists but cannot be read or parsed is an error, so a
    /// broken approval fails the load instead of being silently ignored.
    ///
    /// # Errors
    ///
    /// [`TrustError`] from [`trust::GrantSet::load`].
    pub fn load(app_dir: &Path) -> Result<Self, TrustError> {
        let candidates = vec![
            app_dir.join(trust::GRANTS_FILE),
            app_dir.join("world").join(trust::GRANTS_FILE),
        ];
        for path in &candidates {
            // `exists`, not `is_file`: a directory or special file named
            // `grants.toml` must surface as a read error, not look absent.
            if path.exists() {
                let grants = trust::GrantSet::load(path)?;
                let loaded_from = path.clone();
                return Ok(Self {
                    grants: Some(grants),
                    candidates,
                    loaded_from: Some(loaded_from),
                });
            }
        }
        Ok(Self {
            grants: None,
            candidates,
            loaded_from: None,
        })
    }

    /// An approval backed by `grants` with no file behind it (a tool or a test
    /// that installed the decision elsewhere).
    #[must_use]
    pub fn from_set(grants: trust::GrantSet) -> Self {
        Self {
            grants: Some(grants),
            ..Self::default()
        }
    }

    /// The approved set, or `None` when no grants file was found.
    #[must_use]
    pub fn grant_set(&self) -> Option<&trust::GrantSet> {
        self.grants.as_ref()
    }

    /// Whether the operator approved `capability`.
    #[must_use]
    pub fn allows_capability(&self, capability: Capability) -> bool {
        self.grants
            .as_ref()
            .is_some_and(|grants| grants.allows_capability(capability.name()))
    }

    /// Whether the operator approved outbound HTTP to `host`. Matching is
    /// exact, exactly like [`trust::GrantSet::allows_http_host`].
    #[must_use]
    pub fn allows_http_host(&self, host: &str) -> bool {
        self.grants
            .as_ref()
            .is_some_and(|grants| grants.allows_http_host(host))
    }

    /// The file the approval was read from, when one was found.
    #[must_use]
    pub fn loaded_from(&self) -> Option<&Path> {
        self.loaded_from.as_deref()
    }

    /// The files the host looked at, for the fail-closed message.
    #[must_use]
    pub fn candidates_label(&self) -> String {
        if self.candidates.is_empty() {
            return trust::GRANTS_FILE.to_string();
        }
        self.candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" or ")
    }
}

#[cfg(test)]
mod tests;
