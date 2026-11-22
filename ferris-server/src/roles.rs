//! Server roles: which subsystems a running `ferris` instance serves.
//!
//! The enum lists **every** role the framework defines, not only the ones this
//! binary compiled. That is deliberate: `roles = ["all"]`, `--realtime` and the
//! error message for a typo must read the same on every build, so a config file
//! is not silently reinterpreted by a different binary. A role whose subsystem
//! was not compiled is removed at boot by
//! [`gate_roles`](crate::server::gate_roles) with a log line naming the missing
//! Cargo feature — the compile-time counterpart of `[features]` being `false`.

use anyhow::{Result, anyhow};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServerRole {
    Api,
    Web,
    Realtime,
    Worker,
    Scheduler,
}

impl ServerRole {
    pub const ALL: [ServerRole; 5] = [
        ServerRole::Api,
        ServerRole::Web,
        ServerRole::Realtime,
        ServerRole::Worker,
        ServerRole::Scheduler,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ServerRole::Api => "api",
            ServerRole::Web => "web",
            ServerRole::Realtime => "realtime",
            ServerRole::Worker => "worker",
            ServerRole::Scheduler => "scheduler",
        }
    }

    /// The Cargo feature that compiles this role's subsystem.
    ///
    /// This is the join between the two layers: the role name an operator
    /// writes and the `cfg!(feature = …)` check `gate_roles` uses. `api` has no
    /// gate — it is the base every build carries.
    pub fn cargo_feature(&self) -> &'static str {
        match self {
            ServerRole::Api => "api",
            ServerRole::Web => "web",
            ServerRole::Realtime => "realtime",
            ServerRole::Worker => "jobs",
            ServerRole::Scheduler => "scheduler",
        }
    }
}

impl fmt::Display for ServerRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ServerRole {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        ServerRole::ALL
            .iter()
            .find(|r| r.as_str().eq_ignore_ascii_case(s))
            .copied()
            .ok_or_else(|| {
                anyhow!(
                    "unknown server role `{s}` (expected: all, api, web, \
                     realtime, worker, scheduler)"
                )
            })
    }
}

/// Roles selected for a running server instance.
///
/// Backed by `BTreeSet` so iteration order matches `ServerRole::ALL`
/// (deterministic logs, snapshots, and CLI output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRoles {
    enabled: BTreeSet<ServerRole>,
}

impl Default for ServerRoles {
    fn default() -> Self {
        Self {
            enabled: ServerRole::ALL.into_iter().collect(),
        }
    }
}

impl ServerRoles {
    /// All roles enabled.
    pub fn all() -> Self {
        Self::default()
    }

    /// Parse role names from configuration. `all` expands to every role.
    ///
    /// Unknown names are always rejected, even when `all` is also present —
    /// this catches typos like `["all", "raeltime"]` that would otherwise be
    /// silently masked.
    pub fn parse(list: &[String]) -> Result<Self> {
        let mut enabled = BTreeSet::new();
        let mut all = false;

        for item in list {
            let item = item.trim();
            if item.eq_ignore_ascii_case("all") {
                all = true;
                continue;
            }
            enabled.insert(item.parse::<ServerRole>()?);
        }

        if all {
            enabled = ServerRole::ALL.into_iter().collect();
        }

        Ok(Self { enabled })
    }

    pub fn has(&self, role: ServerRole) -> bool {
        self.enabled.contains(&role)
    }

    pub fn is_empty(&self) -> bool {
        self.enabled.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = ServerRole> + '_ {
        self.enabled.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_individual_roles() {
        let roles = ServerRoles::parse(&["api".into(), "web".into()]).unwrap();
        assert!(roles.has(ServerRole::Api));
        assert!(roles.has(ServerRole::Web));
        assert!(!roles.has(ServerRole::Realtime));
    }

    #[test]
    fn all_expands_to_every_role() {
        let roles = ServerRoles::parse(&["api".into(), "all".into()]).unwrap();
        for role in ServerRole::ALL {
            assert!(roles.has(role));
        }
    }

    #[test]
    fn empty_list_means_no_roles() {
        let roles = ServerRoles::parse(&[]).unwrap();
        assert!(roles.is_empty());
    }

    #[test]
    fn rejects_unknown_roles() {
        assert!(ServerRoles::parse(&["wat".into()]).is_err());
    }

    #[test]
    fn rejects_unknown_roles_even_with_all() {
        // Pinning down the "strict" behavior above.
        assert!(ServerRoles::parse(&["all".into(), "wat".into()]).is_err());
    }

    #[test]
    fn ignores_whitespace_and_case() {
        let roles = ServerRoles::parse(&[" API ".into(), "Realtime".into()]).unwrap();
        assert!(roles.has(ServerRole::Api));
        assert!(roles.has(ServerRole::Realtime));
    }

    #[test]
    fn default_enables_all() {
        assert_eq!(ServerRoles::default(), ServerRoles::all());
        for role in ServerRole::ALL {
            assert!(ServerRoles::default().has(role));
        }
    }

    #[test]
    fn iteration_order_is_deterministic() {
        let roles = ServerRoles::parse(&["web".into(), "api".into(), "scheduler".into()]).unwrap();
        let order: Vec<_> = roles.iter().collect();
        assert_eq!(
            order,
            vec![ServerRole::Api, ServerRole::Web, ServerRole::Scheduler]
        );
    }

    /// Every role names a real Cargo feature, and `api` is the only one that is
    /// not a vertical (it is the base).
    #[test]
    fn every_role_names_its_cargo_feature() {
        for role in ServerRole::ALL {
            let feature = role.cargo_feature();
            assert!(!feature.is_empty());
            assert!(
                crate::host_features::FEATURES.contains(&feature),
                "role {role} names `{feature}`, which is not a declared feature"
            );
        }
    }
}
