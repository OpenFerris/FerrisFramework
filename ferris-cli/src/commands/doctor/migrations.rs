//! What is in `migrations/`, and how the check reports it.

use std::path::Path;

use super::health::HealthCheck;

/// What is in `migrations/`, as far as could be read.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Migrations {
    /// No `migrations/` directory, which a fresh project is expected not to have.
    Missing,
    /// This many migration files. Subdirectories are namespaces, not migrations,
    /// and the check used to count every entry, so a lone `migrations/0001_init/`
    /// read as "1 migration file(s)". The second field is the entries whose type
    /// could not be read, which used to be counted as migrations too.
    Found {
        files: usize,
        unreadable: Vec<String>,
    },
    /// The directory is there and cannot be opened, which is not the same as it
    /// being empty or absent.
    Unreadable(String),
}

pub(super) fn inspect_migrations(dir: &Path) -> Migrations {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Migrations::Missing,
        Err(e) => return Migrations::Unreadable(format!("cannot read {}: {e}", dir.display())),
    };

    let mut files = 0usize;
    let mut unreadable = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => match entry.file_type() {
                Ok(kind) if kind.is_dir() => {}
                Ok(_) => files += 1,
                Err(e) => unreadable.push(format!("{}: {e}", entry.path().display())),
            },
            Err(e) => unreadable.push(e.to_string()),
        }
    }
    Migrations::Found { files, unreadable }
}

pub(super) fn check_migrations(cwd: &Path) -> HealthCheck {
    match inspect_migrations(&cwd.join("migrations")) {
        Migrations::Missing => HealthCheck {
            name: "Migrations".to_string(),
            passed: true,
            message: "no migrations/ dir; schema migrations are generated from \
                       doctypes by `ferris migrate`"
                .to_string(),
            critical: false,
        },
        Migrations::Unreadable(why) => HealthCheck {
            name: "Migrations".to_string(),
            // Not critical — a project builds fine without reading this — but a
            // directory nobody could look in is not a directory that is fine.
            passed: false,
            message: why,
            critical: false,
        },
        Migrations::Found { files, unreadable } if unreadable.is_empty() => HealthCheck {
            name: "Migrations".to_string(),
            passed: true,
            message: format!("{files} migration file(s) in migrations/"),
            critical: false,
        },
        Migrations::Found { files, unreadable } => HealthCheck {
            name: "Migrations".to_string(),
            passed: false,
            message: format!(
                "{files} migration file(s) in migrations/, {} could not be read: {}",
                unreadable.len(),
                unreadable[0]
            ),
            critical: false,
        },
    }
}
