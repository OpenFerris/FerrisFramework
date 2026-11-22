//! `ferris backup` / `ferris restore`.
//!
//! Frappe's `bench backup` is the safety net a migration needs; this CLI had
//! none (`CLI-20`). The backup is a plain directory tree — manifest, database
//! snapshot, files and config — so an operator can inspect it with the usual
//! tools and restore it even without this binary.
//!
//! Only SQLite is supported. A file copy of a live Postgres/MySQL server is not
//! a consistent dump, so those dialects fail loudly and name the tool to use.
//!
//! Size note: this patch's allowed-file list names only this file, so the
//! command, its hashing entry point and its tests stay together instead of
//! being split into `backup/`. See `auditoria/parches/F1.md`.

use super::{BackupArgs, RestoreArgs};
use crate::config::CliConfig;
use anyhow::{Result, bail};
use dialoguer::Confirm;
use ferris_core::DatabaseType;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const MANIFEST: &str = "manifest.json";
const DATABASE_FILE: &str = "database.sqlite";
const FILES_DIR: &str = "files";
const CONFIG_DIR: &str = "config";
const CONFIG_FILE: &str = "ferris.toml";
const FORMAT_VERSION: u32 = 1;

/// Create a backup, or verify an existing one when `--verify` is given.
///
/// # Errors
///
/// Returns an error when the dialect is not SQLite, the database file is
/// missing, or a file cannot be copied or hashed.
pub fn run(args: BackupArgs, config: &CliConfig) -> Result<()> {
    if let Some(dir) = args.verify {
        let manifest = verify_dir(&dir)?;
        info!(
            "Backup {} verified: {} file(s), checksums match.",
            dir.display(),
            manifest.files.len()
        );
        return Ok(());
    }
    let out = args.out.unwrap_or_else(|| PathBuf::from("backups"));
    let dir = create_backup(config, &out)?;
    info!("Backup written to {}", dir.display());
    Ok(())
}

/// Restore a backup after an explicit confirmation.
///
/// # Errors
///
/// Returns an error when the manifest or a checksum is invalid, when `--yes`
/// is absent in a non-interactive shell, or when a destination cannot be
/// written.
pub fn restore(RestoreArgs { from, yes }: RestoreArgs, config: &CliConfig) -> Result<()> {
    // Verify before prompting: a corrupt backup is reported even when the
    // operator would have confirmed.
    let manifest = verify_dir(&from)?;
    let confirmed = yes || confirm_restore(&from, config)?;
    if !confirmed {
        info!("Restore cancelled; nothing was written.");
        return Ok(());
    }
    restore_verified(&from, &manifest, config, true)
}

/// What `restore` needs from a backup directory.
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    /// Schema version, so a future format is rejected instead of half-read.
    format: u32,
    ferris_version: String,
    dialect: String,
    created_at: String,
    created_unix: u64,
    database_name: String,
    /// The config file the backup was taken from, for provenance only. Restore
    /// writes to the config this invocation loaded, not this path.
    #[serde(skip_serializing_if = "Option::is_none")]
    config_path: Option<String>,
    /// Relative path, SHA-256 (lowercase hex) and size of every copied file.
    files: Vec<FileEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FileEntry {
    path: String,
    sha256: String,
    bytes: u64,
}

fn create_backup(config: &CliConfig, out: &Path) -> Result<PathBuf> {
    let database_type = super::db_map::parse_database_type(&config.database)?;
    if database_type != DatabaseType::SQLite {
        bail!(
            "`ferris backup` supports only SQLite; the configured database is `{}`. \
             Take a logical dump with pg_dump/mysqldump and copy the files directory separately.",
            config.database.r#type
        );
    }

    let (created_at, created_unix) = utc_now();
    let dir = unique_backup_dir(out, &timestamp_name(created_unix))?;
    create_private_dir(&dir)?;
    if let Err(error) = fill_backup(config, &dir, &created_at, created_unix) {
        // A failed backup must not leave a half-written directory that looks
        // restorable.
        if let Err(cleanup) = fs::remove_dir_all(&dir) {
            warn!(
                "Could not remove the partial backup {}: {cleanup}",
                dir.display()
            );
        }
        return Err(error);
    }
    Ok(dir)
}

fn fill_backup(config: &CliConfig, dir: &Path, created_at: &str, created_unix: u64) -> Result<()> {
    let mut entries = Vec::new();

    let database_file = dir.join(DATABASE_FILE);
    snapshot_sqlite(config, &database_file)?;
    set_private_file(&database_file)?;
    entries.push(file_entry(dir, &database_file)?);

    // `[files] local_path` (or `FERRIS_FILES_DIR` in the core config) is
    // resolved against the process working directory, which `main` sets to the
    // project directory. A missing source is an empty files backup, not an
    // error: a fresh project has no uploads yet.
    let files_source = PathBuf::from(&config.files.local_path);
    let files_dir = dir.join(FILES_DIR);
    create_private_dir(&files_dir)?;
    if files_source.is_dir() {
        copy_tree(
            &files_source,
            &files_dir,
            Path::new(FILES_DIR),
            &mut entries,
        )?;
    } else if files_source.exists() {
        bail!(
            "[files] local_path {} is not a directory",
            files_source.display()
        );
    }

    let config_path = if config.source.config_path.is_file() {
        let config_dir = dir.join(CONFIG_DIR);
        create_private_dir(&config_dir)?;
        let destination = config_dir.join(CONFIG_FILE);
        entries.push(copy_hashed(&config.source.config_path, &destination, dir)?);
        Some(config.source.config_path.display().to_string())
    } else {
        None
    };

    let manifest = Manifest {
        format: FORMAT_VERSION,
        ferris_version: env!("CARGO_PKG_VERSION").to_string(),
        dialect: "sqlite".to_string(),
        created_at: created_at.to_string(),
        created_unix,
        database_name: config.database.name.clone(),
        config_path,
        files: entries,
    };
    write_private_file(&dir.join(MANIFEST), &serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}

/// A consistent snapshot of a live SQLite database via `VACUUM INTO`.
///
/// A plain copy of the file can capture a torn page while a writer is active;
/// `VACUUM INTO` runs inside SQLite's read transaction and emits one consistent
/// database.
fn snapshot_sqlite(config: &CliConfig, destination: &Path) -> Result<()> {
    let name = config.database.name.as_str();
    if name.is_empty() || name == ":memory:" {
        // An in-memory database has no durable state, and the bundled SQLite
        // treats `VACUUM INTO` as a no-op for a `:memory:` source. A zero-byte
        // file is a valid empty SQLite database — the faithful snapshot.
        drop(private_file(destination)?);
        return Ok(());
    }
    if !Path::new(name).is_file() {
        bail!("SQLite database {name} does not exist; nothing to back up");
    }
    let database = super::db_map::core_database(&config.database, name.to_string())?;
    let target = destination
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("backup path is not valid UTF-8"))?;
    // `VACUUM INTO` takes a string expression; the path comes from the CLI, so
    // the only metacharacter that needs escaping is the single quote.
    let statement = format!("VACUUM INTO '{}'", target.replace('\'', "''"));

    let runtime = crate::runtime::block_on_runtime(None)?;
    runtime.block_on(async move {
        let db = ferris_db::open_unmanaged(&database).await?;
        db.execute(&ferris_db::Stmt::raw(statement)).await?;
        db.close().await;
        Ok(())
    })
}

fn verify_dir(dir: &Path) -> Result<Manifest> {
    let manifest_path = dir.join(MANIFEST);
    let bytes = fs::read(&manifest_path)
        .map_err(|e| anyhow::anyhow!("could not read {}: {e}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("{} is not a valid backup manifest: {e}", dir.display()))?;

    if manifest.format != FORMAT_VERSION {
        bail!(
            "unsupported backup format {} (this ferris understands {FORMAT_VERSION})",
            manifest.format
        );
    }
    if manifest.files.is_empty() {
        bail!(
            "{} lists no files; refusing an empty manifest",
            dir.display()
        );
    }

    let mut seen = std::collections::BTreeSet::new();
    for entry in &manifest.files {
        if !seen.insert(entry.path.clone()) {
            bail!("{} lists `{}` twice", dir.display(), entry.path);
        }
        let path = safe_join(dir, &entry.path)?;
        let metadata = fs::metadata(&path)
            .map_err(|e| anyhow::anyhow!("missing {} in the backup: {e}", entry.path))?;
        if !metadata.is_file() {
            bail!("{} is not a file", entry.path);
        }
        if metadata.len() != entry.bytes {
            bail!(
                "size mismatch for {}: manifest says {}, file is {} bytes",
                entry.path,
                entry.bytes,
                metadata.len()
            );
        }
        let actual = sha256_file(&path)?;
        if actual != entry.sha256 {
            bail!(
                "checksum mismatch for {}: manifest {}, file {}",
                entry.path,
                entry.sha256,
                actual
            );
        }
    }

    if !manifest
        .files
        .iter()
        .any(|entry| entry.path == DATABASE_FILE)
    {
        bail!("{} has no {DATABASE_FILE} entry", dir.display());
    }
    Ok(manifest)
}

fn restore_verified(
    dir: &Path,
    manifest: &Manifest,
    config: &CliConfig,
    confirmed: bool,
) -> Result<()> {
    if !confirmed {
        bail!("restore requires --yes (or an interactive confirmation); nothing was written");
    }
    if manifest.dialect != "sqlite" {
        bail!(
            "restore supports only SQLite backups; this one was taken from `{}`",
            manifest.dialect
        );
    }
    if super::db_map::parse_database_type(&config.database)? != DatabaseType::SQLite {
        bail!(
            "restore supports only SQLite; the configured database is `{}`",
            config.database.r#type
        );
    }
    if config.database.name.is_empty() || config.database.name == ":memory:" {
        bail!("cannot restore into an in-memory SQLite database; point [database] name at a file");
    }

    let database_destination = PathBuf::from(&config.database.name);
    install_file(&dir.join(DATABASE_FILE), &database_destination)?;

    let files_source = dir.join(FILES_DIR);
    if files_source.is_dir() {
        install_dir(&files_source, &PathBuf::from(&config.files.local_path))?;
    }

    let config_source = dir.join(CONFIG_DIR).join(CONFIG_FILE);
    if config_source.is_file() {
        install_file(&config_source, &config.source.config_path)?;
    }

    info!(
        "Restored database {}, files {} and config {}.",
        database_destination.display(),
        config.files.local_path,
        config.source.config_path.display()
    );
    Ok(())
}

/// `--yes` is mandatory when stdin is not a terminal, so a script cannot
/// restore by accident.
fn confirm_restore(from: &Path, config: &CliConfig) -> Result<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        bail!(
            "restore overwrites the database, files and config; pass --yes in a non-interactive shell"
        );
    }
    Ok(Confirm::new()
        .with_prompt(format!(
            "Restore {} over database `{}`, files `{}` and config `{}`?",
            from.display(),
            config.database.name,
            config.files.local_path,
            config.source.config_path.display()
        ))
        .default(false)
        .interact()?)
}

/// Copy through a temporary sibling, so a failure mid-copy does not truncate
/// the destination.
fn install_file(source: &Path, destination: &Path) -> Result<()> {
    let parent = crate::project::parent_dir(destination);
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".ferris-restore-{}", uuid::Uuid::new_v4()));
    if let Err(error) = copy_exact(source, &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Replace a directory with the backup copy, keeping the previous contents
/// only until the new tree is in place.
fn install_dir(source: &Path, destination: &Path) -> Result<()> {
    let parent = crate::project::parent_dir(destination);
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".ferris-restore-{}", uuid::Uuid::new_v4()));
    create_private_dir(&temporary)?;
    if let Err(error) = copy_plain_tree(source, &temporary) {
        let _ = fs::remove_dir_all(&temporary);
        return Err(error);
    }

    if destination.exists() {
        let previous = parent.join(format!(".ferris-restore-old-{}", uuid::Uuid::new_v4()));
        fs::rename(destination, &previous)?;
        if let Err(error) = fs::rename(&temporary, destination) {
            // A failed restore must not leave the files directory missing.
            let _ = fs::rename(&previous, destination);
            let _ = fs::remove_dir_all(&temporary);
            return Err(error.into());
        }
        if let Err(error) = fs::remove_dir_all(&previous) {
            warn!(
                "Restored files, but could not remove the old copy {}: {error}",
                previous.display()
            );
        }
    } else {
        fs::rename(&temporary, destination)?;
    }
    Ok(())
}

fn copy_exact(source: &Path, destination: &Path) -> Result<()> {
    fs::copy(source, destination)
        .map_err(|e| anyhow::anyhow!("could not copy {}: {e}", source.display()))?;
    set_private_file(destination)
}

fn sorted_names(dir: &Path) -> Result<Vec<std::ffi::OsString>> {
    let mut names: Vec<_> = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    Ok(names)
}

fn copy_plain_tree(source: &Path, destination: &Path) -> Result<()> {
    for name in sorted_names(source)? {
        let from = source.join(&name);
        let to = destination.join(&name);
        let file_type = fs::symlink_metadata(&from)?.file_type();
        if file_type.is_dir() {
            create_private_dir(&to)?;
            copy_plain_tree(&from, &to)?;
        } else if file_type.is_file() {
            copy_exact(&from, &to)?;
        } else {
            bail!(
                "{} is a symbolic link or special file; a restore cannot reproduce it",
                from.display()
            );
        }
    }
    Ok(())
}

/// Copy `source` into the backup tree, recording path, size and digest.
fn copy_hashed(source: &Path, destination: &Path, root: &Path) -> Result<FileEntry> {
    let (sha256, bytes) = copy_private_hashed(source, destination)?;
    Ok(FileEntry {
        path: relative_path(root, destination)?,
        sha256,
        bytes,
    })
}

fn copy_tree(
    source: &Path,
    destination: &Path,
    relative: &Path,
    entries: &mut Vec<FileEntry>,
) -> Result<()> {
    for name in sorted_names(source)? {
        let from = source.join(&name);
        let to = destination.join(&name);
        let child = relative.join(&name);
        let file_type = fs::symlink_metadata(&from)?.file_type();
        if file_type.is_dir() {
            create_private_dir(&to)?;
            copy_tree(&from, &to, &child, entries)?;
        } else if file_type.is_file() {
            let (sha256, bytes) = copy_private_hashed(&from, &to)?;
            entries.push(FileEntry {
                path: path_string(&child)?,
                sha256,
                bytes,
            });
        } else {
            // Following a link could copy a file from outside the files root
            // into the backup; copying the link itself cannot be verified by a
            // checksum. Refuse both.
            bail!(
                "{} is a symbolic link or special file; backups copy regular files only",
                from.display()
            );
        }
    }
    Ok(())
}

fn copy_private_hashed(source: &Path, destination: &Path) -> Result<(String, u64)> {
    let bytes = fs::read(source)
        .map_err(|e| anyhow::anyhow!("could not read {}: {e}", source.display()))?;
    write_private_file(destination, &bytes)?;
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    Ok((ferris_server::tls::sha256_hex(&bytes), length))
}

fn file_entry(root: &Path, path: &Path) -> Result<FileEntry> {
    let metadata = fs::metadata(path)?;
    Ok(FileEntry {
        path: relative_path(root, path)?,
        sha256: sha256_file(path)?,
        bytes: metadata.len(),
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).map_err(|e| anyhow::anyhow!("could not read {}: {e}", path.display()))?;
    Ok(ferris_server::tls::sha256_hex(&bytes))
}

fn relative_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| anyhow::anyhow!("{} is outside {}", path.display(), root.display()))?;
    path_string(relative)
}

fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| anyhow::anyhow!("backup path {} is not valid UTF-8", path.display()))
}

/// Resolve a manifest path without allowing `..` or an absolute path to escape
/// the backup directory.
fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.is_absolute() {
        bail!("manifest path `{relative}` is absolute; refusing it");
    }
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        bail!("manifest path `{relative}` leaves the backup directory; refusing it");
    }
    Ok(root.join(path))
}

fn utc_now() -> (String, u64) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    (format_utc(seconds), seconds)
}

/// `YYYY-MM-DDTHH:MM:SSZ`, without a date-time dependency.
fn format_utc(seconds: u64) -> String {
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// `YYYYMMDDTHHMMSSZ`, the directory name (no colons).
fn timestamp_name(seconds: u64) -> String {
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let time = seconds % 86_400;
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's algorithm). All
/// values are `u64`: the clock cannot produce a date before the epoch here.
const fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn unique_backup_dir(out: &Path, name: &str) -> Result<PathBuf> {
    fs::create_dir_all(out)?;
    let mut candidate = out.join(name);
    let mut suffix = 1u32;
    while candidate.exists() {
        candidate = out.join(format!("{name}-{suffix}"));
        suffix += 1;
        if suffix > 10_000 {
            bail!(
                "could not find a free backup directory name in {}",
                out.display()
            );
        }
    }
    Ok(candidate)
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    // `mode` is masked by the process umask; make the result explicit.
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn private_file(path: &Path) -> Result<fs::File> {
    Ok(fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

fn set_private_file(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = private_file(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_core::DatabaseConfig;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferris-backup-{label}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn create_db(path: &Path) {
        let runtime = crate::runtime::block_on_runtime(None).unwrap();
        runtime.block_on(async {
            let config = DatabaseConfig {
                r#type: DatabaseType::SQLite,
                name: path.display().to_string(),
                max_connections: 1,
                min_connections: 0,
                ..DatabaseConfig::default()
            };
            let db = ferris_db::open_unmanaged(&config).await.unwrap();
            db.execute(&ferris_db::Stmt::raw(
                "CREATE TABLE note (id INTEGER PRIMARY KEY, body TEXT)",
            ))
            .await
            .unwrap();
            db.execute(&ferris_db::Stmt::new(
                "INSERT INTO note (body) VALUES (?)",
                vec![ferris_core::Value::String("hola".into())],
            ))
            .await
            .unwrap();
            db.close().await;
        });
    }

    /// A project with a real SQLite file, a files root and a config file.
    fn project(label: &str) -> (PathBuf, CliConfig) {
        let root = scratch(label);
        let database = root.join("site.db");
        create_db(&database);
        let files = root.join("files");
        fs::create_dir_all(files.join("avatars")).unwrap();
        fs::write(files.join("hello.txt"), "bonjour").unwrap();
        fs::write(files.join("avatars/user.txt"), "avatar").unwrap();
        let config = root.join("config/ferris.toml");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(&config, "[server]\n").unwrap();

        let mut cli = CliConfig::default();
        cli.database.r#type = "sqlite".into();
        cli.database.name = database.display().to_string();
        cli.files.local_path = files.display().to_string();
        cli.source.project_dir = root.clone();
        cli.source.config_path = config;
        (root, cli)
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn sha256_is_the_standard_digest() {
        assert_eq!(
            ferris_server::tls::sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            ferris_server::tls::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn backup_writes_manifest_checksums_and_private_permissions() {
        let (root, config) = project("manifest");
        let dir = create_backup(&config, &root.join("backups")).unwrap();

        let manifest: Manifest =
            serde_json::from_slice(&fs::read(dir.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest.format, FORMAT_VERSION);
        assert_eq!(manifest.dialect, "sqlite");
        assert_eq!(manifest.ferris_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(manifest.database_name, config.database.name);
        assert!(manifest.created_at.ends_with('Z'));

        let database = manifest
            .files
            .iter()
            .find(|entry| entry.path == DATABASE_FILE)
            .expect("the database is in the manifest");
        assert_eq!(
            database.sha256,
            sha256_file(&dir.join(DATABASE_FILE)).unwrap()
        );
        for path in [
            "files/hello.txt",
            "files/avatars/user.txt",
            "config/ferris.toml",
        ] {
            assert!(
                manifest.files.iter().any(|entry| entry.path == path),
                "{path} is missing"
            );
        }
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(DATABASE_FILE)), 0o600);
        assert_eq!(mode(&dir.join(MANIFEST)), 0o600);
        verify_dir(&dir).unwrap();
    }

    #[test]
    fn a_tampered_file_fails_verification_before_restore() {
        let (root, config) = project("tamper");
        let dir = create_backup(&config, &root.join("backups")).unwrap();
        let database = dir.join(DATABASE_FILE);
        let mut bytes = fs::read(&database).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&database, bytes).unwrap();

        let error = verify_dir(&dir).expect_err("the digest no longer matches");
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
    }

    #[test]
    fn a_non_sqlite_dialect_is_refused_without_creating_a_directory() {
        let root = scratch("dialect");
        let mut config = CliConfig::default();
        config.database.r#type = "postgres".into();
        let out = root.join("backups");
        let error = create_backup(&config, &out).expect_err("postgres cannot be file-copied");
        assert!(error.to_string().contains("only SQLite"), "{error}");
        assert!(!out.exists(), "a refused backup creates nothing");
    }

    #[test]
    fn restore_without_confirmation_does_not_write() {
        let (root, config) = project("no-yes");
        let dir = create_backup(&config, &root.join("backups")).unwrap();
        let (_, destination) = project("no-yes-destination");
        let database = PathBuf::from(&destination.database.name);
        let files = PathBuf::from(&destination.files.local_path);
        let config_before = fs::read(&destination.source.config_path).unwrap();
        let database_before = fs::read(&database).unwrap();
        fs::write(files.join("keep.txt"), "keep me").unwrap();

        let error = restore_verified(&dir, &verify_dir(&dir).unwrap(), &destination, false)
            .expect_err("without --yes a restore is refused");
        assert!(error.to_string().contains("--yes"), "{error}");
        assert_eq!(fs::read(&database).unwrap(), database_before);
        assert_eq!(
            fs::read(&destination.source.config_path).unwrap(),
            config_before
        );
        assert!(files.join("keep.txt").is_file());
    }

    #[test]
    fn restore_replaces_database_files_and_config() {
        let (root, config) = project("restore");
        let dir = create_backup(&config, &root.join("backups")).unwrap();

        let (destination_root, destination) = project("restore-destination");
        let database = PathBuf::from(&destination.database.name);
        let files = PathBuf::from(&destination.files.local_path);
        fs::write(files.join("stale.txt"), "stale").unwrap();
        fs::write(&destination.source.config_path, "[server]\nport = 1\n").unwrap();

        restore_verified(&dir, &verify_dir(&dir).unwrap(), &destination, true).unwrap();

        assert_eq!(
            sha256_file(&database).unwrap(),
            sha256_file(&dir.join(DATABASE_FILE)).unwrap()
        );
        assert_eq!(
            fs::read_to_string(files.join("hello.txt")).unwrap(),
            "bonjour"
        );
        assert!(!files.join("stale.txt").exists());
        assert_eq!(
            fs::read_to_string(&destination.source.config_path).unwrap(),
            "[server]\n"
        );
        assert!(destination_root.join("config/ferris.toml").is_file());
    }

    #[test]
    fn an_in_memory_database_can_be_backed_up_but_not_restored_into() {
        let root = scratch("memory");
        let mut config = CliConfig::default();
        config.database.r#type = "sqlite".into();
        config.database.name = ":memory:".into();
        config.files.local_path = root.join("files").display().to_string();
        config.source.project_dir = root.clone();
        config.source.config_path = root.join("ferris.toml");

        let dir = create_backup(&config, &root.join("backups")).unwrap();
        assert!(dir.join(DATABASE_FILE).is_file());
        verify_dir(&dir).unwrap();

        let error = restore_verified(&dir, &verify_dir(&dir).unwrap(), &config, true)
            .expect_err("an in-memory destination cannot hold a restored file");
        assert!(error.to_string().contains("in-memory"), "{error}");
    }
}
