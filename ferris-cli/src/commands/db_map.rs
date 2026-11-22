//! Shared mapping from the CLI (string-typed) database config onto the core
//! config types. Used by `serve`, `migrate`, `tenant`, `doctor` and
//! `benchmark`.
//!
//! One mapping for every command. A value this module does not recognise is an
//! error, never a silent fallback: the one dangerous mistake in this section is
//! asking for certificate verification and getting a different mode.

use crate::config::DatabaseConfig as CliDatabaseConfig;
use anyhow::Result;
use ferris_core::{DatabaseConfig, DatabaseType, config::SslMode};
use std::path::Path;

/// The database types a config may name, and their spellings.
const DATABASE_TYPES: [(&str, DatabaseType); 7] = [
    ("postgres", DatabaseType::Postgres),
    ("postgresql", DatabaseType::Postgres),
    ("pg", DatabaseType::Postgres),
    ("mysql", DatabaseType::MySQL),
    ("mariadb", DatabaseType::MySQL),
    ("sqlite", DatabaseType::SQLite),
    ("sqlite3", DatabaseType::SQLite),
];

/// The `ssl_mode` values a config may name. Compared with the separators removed,
/// so `verifyfull`, `verify_full` and `verify-full` are one mode, as they are in
/// libpq.
const SSL_MODES: [(&str, SslMode); 7] = [
    ("disable", SslMode::Disable),
    ("allow", SslMode::Allow),
    ("prefer", SslMode::Prefer),
    ("require", SslMode::Require),
    ("verifyca", SslMode::VerifyCa),
    ("verifyfull", SslMode::VerifyFull),
    ("true", SslMode::Require),
];

/// Every accepted spelling of a database type, for an error message.
pub(crate) fn database_types() -> Vec<&'static str> {
    DATABASE_TYPES.iter().map(|(name, _)| *name).collect()
}

/// Every accepted spelling of an `ssl_mode`, for an error message.
pub(crate) fn ssl_modes() -> Vec<&'static str> {
    SSL_MODES.iter().map(|(name, _)| *name).collect()
}

fn normalise(raw: &str) -> String {
    raw.to_ascii_lowercase().replace(['-', '_'], "")
}

/// The configured database type, or an error naming what was accepted.
///
/// This used to answer `Postgres` for anything it did not recognise, so
/// `type = "postgress"` quietly connected to Postgres instead of telling the
/// truth about the config.
pub(crate) fn parse_database_type(cli: &CliDatabaseConfig) -> Result<DatabaseType> {
    let wanted = normalise(&cli.r#type);
    DATABASE_TYPES
        .iter()
        .find(|(name, _)| *name == wanted)
        .map(|(_, ty)| *ty)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unsupported database type `{}` (expected one of: {})",
                cli.r#type,
                database_types().join(", ")
            )
        })
}

/// The configured `ssl_mode`, or an error. Every spelling of a mode is accepted;
/// anything else is refused rather than replaced with `Prefer`.
pub(crate) fn parse_ssl_mode(cli: &CliDatabaseConfig) -> Result<SslMode> {
    let wanted = normalise(&cli.ssl_mode);
    SSL_MODES
        .iter()
        .find(|(name, _)| *name == wanted)
        .map(|(_, mode)| *mode)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown ssl_mode `{}` (expected one of: {})",
                cli.ssl_mode,
                ssl_modes().join(", ")
            )
        })
}

/// The core database config the CLI describes.
///
/// `name` overrides the configured database name, which is how `tenant` moves a
/// command onto another database. What the file does *not* say — how the pool
/// behaves — is left to [`DatabaseConfig::default`], so a dev command and a
/// server do not quietly disagree about connection counts.
pub(crate) fn core_database(cli: &CliDatabaseConfig, name: String) -> Result<DatabaseConfig> {
    let mut config = DatabaseConfig {
        r#type: parse_database_type(cli)?,
        host: cli.host.clone(),
        port: cli.port,
        name,
        user: cli.user.clone(),
        password: cli.password.clone(),
        max_connections: cli.max_connections.max(1),
        min_connections: 0,
        ssl_mode: parse_ssl_mode(cli)?,
        ..DatabaseConfig::default()
    };
    // The one default that would be an invention: Postgres already searches
    // `public` on its own, and for SQLite or MySQL a schema name means nothing.
    // `serve` used to pass `None` here and the rest of the CLI inherited
    // `Some("public")`.
    config.schema = None;
    Ok(config)
}

/// Whether the project runs on SQLite, which needs neither a server nor an SSL
/// mode. An unrecognised type is not SQLite, and `core_database` will say so.
pub(crate) fn is_sqlite(cli: &CliDatabaseConfig) -> bool {
    matches!(parse_database_type(cli), Ok(DatabaseType::SQLite))
}

/// SQLite keeps its database in a file, and a bare relative name is resolved
/// against the process working directory, which is the project directory only
/// because `main` moves there for `--dir`.
pub(crate) fn sqlite_file(cli: &CliDatabaseConfig) -> std::path::PathBuf {
    Path::new(&cli.name).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseConfig as Cli;

    fn cli(type_: &str, ssl_mode: &str) -> Cli {
        Cli {
            r#type: type_.into(),
            host: "localhost".into(),
            port: 5432,
            name: "test.db".into(),
            user: "ferris".into(),
            password: "ferris".into(),
            max_connections: 20,
            ssl_mode: ssl_mode.into(),
        }
    }

    /// The bug this module exists for: the server read `verifyfull` as `Prefer`
    /// while `migrate` read it as `VerifyFull`, so the same config file verified
    /// certificates in one command and not in the other.
    #[test]
    fn every_spelling_of_verify_full_verifies_the_certificate() {
        for spelling in ["verifyfull", "verify_full", "verify-full", "VerifyFull"] {
            let mode = parse_ssl_mode(&cli("postgres", spelling))
                .unwrap_or_else(|e| panic!("`{spelling}` must be understood: {e}"));
            assert_eq!(
                mode,
                SslMode::VerifyFull,
                "`{spelling}` must not fall back to Prefer"
            );
        }
    }

    #[test]
    fn an_unknown_ssl_mode_is_an_error_not_a_default() {
        // A typo must not quietly turn verification off.
        let err = parse_ssl_mode(&cli("postgres", "verifyful"))
            .expect_err("a misspelled mode is not a mode");
        assert!(
            err.to_string().contains("unknown ssl_mode `verifyful`"),
            "{err}"
        );
        assert!(
            err.to_string().contains("verifyfull"),
            "the error lists: {err}"
        );

        assert_eq!(
            parse_ssl_mode(&cli("postgres", "disable")).unwrap(),
            SslMode::Disable
        );
        assert_eq!(
            parse_ssl_mode(&cli("postgres", "REQUIRE")).unwrap(),
            SslMode::Require
        );
    }

    #[test]
    fn an_unknown_database_type_is_an_error_not_postgres() {
        let err = parse_database_type(&cli("postgress", "disable"))
            .expect_err("a typo is not a database");
        assert!(
            err.to_string()
                .contains("unsupported database type `postgress`"),
            "{err}"
        );
        assert!(
            err.to_string().contains("postgres"),
            "the error lists: {err}"
        );
    }

    #[test]
    fn the_documented_spellings_are_accepted() {
        for (name, expected) in [
            ("postgres", DatabaseType::Postgres),
            ("PostgreSQL", DatabaseType::Postgres),
            ("pg", DatabaseType::Postgres),
            ("mysql", DatabaseType::MySQL),
            ("MariaDB", DatabaseType::MySQL),
            ("sqlite3", DatabaseType::SQLite),
        ] {
            assert_eq!(
                parse_database_type(&cli(name, "disable")).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn a_bad_mode_stops_the_config_from_being_built_at_all() {
        assert!(
            core_database(&cli("postgres", "verifyful"), "db".into()).is_err(),
            "a server must not be configured from a config it does not understand"
        );
        assert!(core_database(&cli("postgress", "disable"), "db".into()).is_err());
        assert!(
            core_database(&cli("postgres", "verifyfull"), "db".into()).is_ok(),
            "the spelling that used to be dropped is the one in libpq"
        );
    }

    #[test]
    fn sqlite_is_told_apart_without_guessing() {
        assert!(is_sqlite(&cli("sqlite", "disable")));
        assert!(!is_sqlite(&cli("postgres", "disable")));
        assert!(
            !is_sqlite(&cli("postgress", "disable")),
            "a typo is not sqlite"
        );
    }
}
