//! The compile-time feature set of this `ferris` binary, and the checks that
//! keep the runtime `[features]` switches honest.
//!
//! Cargo features are **compile-time**. A `ferris` built without `print` cannot
//! gain printing at runtime, and a config that asks for it on that binary must
//! not be served as if the engine existed: the operator would be watching a
//! process that answers nothing on those routes. The same holds for every
//! `[features]` switch whose subsystem was compiled out.
//!
//! There is **one** source of truth for "what did this binary compile":
//! [`ferris_server::host_features::compiled`], built from `cfg!(feature = …)`
//! inside the server crate. The CLI must not read its own `cfg!` — it links the
//! server, and the server's flags are the ones that decide what the process can
//! actually serve. This module is the CLI-side *view* of that set plus the
//! operator-facing wording, and it exists for three callers:
//!
//! * `ferris serve`/`run`/`dev` (through `serve::settings_for`) refuse to boot
//!   and name the flags to rebuild with;
//! * `ferris doctor` prints the compiled set next to the configured one and
//!   fails on a mismatch;
//! * `ferris info` and `ferris config show` report the compiled set (no secrets
//!   involved).
//!
//! Deliberate non-goals:
//!
//! * **Roles are not validated here.** `ferris-server` owns the role gate
//!   (`gate_roles` plus its compile-time gates), and a second answer to the same
//!   question is how two layers end up disagreeing. The CLI validates the
//!   `[features]` matrix only.
//! * **`features.search` and `features.webhooks` are not checked.** They are
//!   runtime switches over code that ships inside the always-linked API and
//!   integrations paths, so there is no Cargo feature to compare them against.
//!   The server's boot check draws the same line
//!   (`ferris_server::server::validate_compiled_features`); when that list
//!   changes, this one changes with it.
//!
//! The mirror rule still holds in the server: `true` never adds a subsystem that
//! was not compiled, `false` removes one that was.

use ferris_core::host_features::HostFeatures;
use ferris_core::{ConfigError, Error, FeatureFlags, Result};
use ferris_server::host_features as server_features;

use crate::config::CliConfig;

/// One subsystem the server can report as compiled.
///
/// The names are exactly `ferris_server::host_features::FEATURES` (minus
/// `full`, which is a reported union rather than an independent switch): `api`
/// (the always-on base), `web`, `realtime`, `jobs`, `scheduler`, `email`,
/// `files`, `print`, `report`, `integrations`, `wasm`. A typo here would make a
/// check silently pass, which is why the sync is pinned by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    Api,
    Web,
    Realtime,
    Jobs,
    Scheduler,
    Email,
    Files,
    Print,
    Report,
    Integrations,
    Wasm,
}

impl Feature {
    /// Every subsystem, in the order the reports present them.
    pub const ALL: [Self; 11] = [
        Self::Api,
        Self::Web,
        Self::Realtime,
        Self::Jobs,
        Self::Scheduler,
        Self::Email,
        Self::Files,
        Self::Print,
        Self::Report,
        Self::Integrations,
        Self::Wasm,
    ];

    /// The Cargo feature name, as the server reports it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Web => "web",
            Self::Realtime => "realtime",
            Self::Jobs => "jobs",
            Self::Scheduler => "scheduler",
            Self::Email => "email",
            Self::Files => "files",
            Self::Print => "print",
            Self::Report => "report",
            Self::Integrations => "integrations",
            Self::Wasm => "wasm",
        }
    }
}

/// The feature set of the running binary, as the server reports it.
///
/// The value type is [`ferris_core::host_features::HostFeatures`]: the same type
/// `ferris-server` installs at boot and `ferris-wasm` serves through
/// `platform::features()`, so the doctor report, the boot pre-flight and the WIT
/// answer cannot use three different vocabularies.
#[must_use]
pub fn compiled() -> HostFeatures {
    server_features::compiled()
}

/// `api, web, print` — or `none` for a set with nothing at all (only reachable
/// in tests, since `api` is always compiled).
#[must_use]
pub fn describe(compiled: &HostFeatures) -> String {
    if compiled.is_empty() {
        "none".to_string()
    } else {
        compiled.to_vec().join(", ")
    }
}

/// A subsystem the config asks for and this binary cannot serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureMismatch {
    pub feature: Feature,
    /// The config key that asked for it (`features.realtime`, …).
    pub asked_by: String,
}

impl FeatureMismatch {
    /// One sentence an operator can act on: the config key, the missing Cargo
    /// feature and the command that fixes it.
    #[must_use]
    pub fn reason(&self) -> String {
        format!(
            "`{}` asks for the `{}` subsystem, but this `ferris` binary was built \
             without it; rebuild ferris with `--features {}` (or `--features full` \
             for every subsystem), e.g. `cargo build -p ferris-cli --features {}`",
            self.asked_by,
            self.feature.name(),
            self.feature.name(),
            self.feature.name()
        )
    }
}

/// Everything the config asks for that this binary cannot deliver.
///
/// The checked set is the server's `validate_compiled_features` set: the
/// verticals that own a Cargo feature (`features.realtime` … `features.report`).
/// `search`/`webhooks` are reported by [`configured_summary`] but never checked
/// — see the module docs.
///
/// Empty means the config and the build agree. The order is deterministic (the
/// `[features]` order) so `doctor` and the boot error read the same way twice.
#[must_use]
pub fn mismatches(config: &CliConfig, compiled: &HostFeatures) -> Vec<FeatureMismatch> {
    let gates = [
        (
            Feature::Realtime,
            "features.realtime",
            config.features.realtime,
        ),
        (Feature::Jobs, "features.jobs", config.features.jobs),
        (
            Feature::Scheduler,
            "features.scheduler",
            config.features.scheduler,
        ),
        (Feature::Email, "features.email", config.features.email),
        (Feature::Files, "features.files", config.features.files),
        (Feature::Print, "features.print", config.features.print),
        (Feature::Report, "features.report", config.features.report),
    ];

    let mut found = Vec::new();
    for (feature, key, enabled) in gates {
        if enabled && !compiled.contains(feature.name()) {
            found.push(FeatureMismatch {
                feature,
                asked_by: key.to_string(),
            });
        }
    }
    found
}

/// Fail closed when the config asks for a subsystem this binary does not have.
///
/// The error is the config taxonomy's `Invalid` (not an ad-hoc `anyhow`), so a
/// caller can still react to a variant, and its message contains every
/// mismatch plus the rebuild hint — never a silent downgrade.
///
/// `ferris-server` repeats this check at boot on the merged `Config`
/// (`validate_compiled_features`), because a server embedded by something that
/// is not this CLI deserves the same answer. This one runs earlier and speaks
/// the CLI's dialect; the rule set is the same on purpose.
///
/// # Errors
///
/// [`ConfigError::Invalid`] with the first offending key and one sentence per
/// mismatch, including the `--features <name>` command that fixes it.
pub fn validate(config: &CliConfig, compiled: &HostFeatures) -> Result<()> {
    let found = mismatches(config, compiled);
    if found.is_empty() {
        return Ok(());
    }

    let key = found
        .first()
        .map_or_else(|| "features".to_string(), |m| m.asked_by.clone());
    let msg = found
        .iter()
        .map(FeatureMismatch::reason)
        .collect::<Vec<_>>()
        .join("; ");
    Err(Error::Config(ConfigError::Invalid { key, msg }))
}

/// `realtime=on jobs=on …` for `[features]`, one line, printable.
///
/// Every switch in the table, including the two that are not checked against a
/// Cargo feature: this line is the operator's view of the file, not the
/// validator's view of the binary.
#[must_use]
pub fn configured_summary(flags: &FeatureFlags) -> String {
    let switches = [
        ("realtime", flags.realtime),
        ("jobs", flags.jobs),
        ("scheduler", flags.scheduler),
        ("email", flags.email),
        ("files", flags.files),
        ("print", flags.print),
        ("report", flags.report),
        ("search", flags.search),
        ("webhooks", flags.webhooks),
    ];
    switches
        .into_iter()
        .map(|(name, enabled)| format!("{name}={}", if enabled { "on" } else { "off" }))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
