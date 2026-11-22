//! Config command
//!
//! `get`/`set` read and write the file `ferris` actually loaded (resolved from
//! `--config` and `--dir`), with `toml_edit`, preserving formatting and
//! comments. Values support dotted keys (`server.port`) and are typed by
//! inference (bool / int / float / string).

use super::*;
use crate::config::{CliConfig, ConfigSource};
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, Value};

/// Sections that describe this machine's services or carry key material rather
/// than project settings. `reset` leaves them alone: the compiled-in defaults
/// point at `localhost:5432` with `ferris`/`ferris`, so resetting a sqlite
/// project used to repoint it at Postgres without a word. `[secrets]` is here
/// because dropping its `key` makes every stored `Secret` field undecryptable
/// for good.
const PRESERVED_ON_RESET: [&str; 3] = ["database", "redis", "secrets"];

/// Nested sections that must survive a reset for the same reason: `[server.tls]`
/// keeps the process from silently downgrading to plain HTTP, and
/// `[server.pki]` holds private key material.
const PRESERVED_SUBTABLES: [(&str, &str); 2] = [("server", "tls"), ("server", "pki")];

/// Placeholder written instead of a secret. Anything that could sign a token or
/// open a connection is replaced by it.
const REDACTED: &str = "<redacted>";

/// Substrings that mark a key as carrying a credential. The match is by
/// substring on the lowercase segment, so `smtp_password`, `secret_access_key`
/// and `admin_password_hash` are covered without being listed one by one. The
/// old exact-name list missed every one of those.
const SECRET_MARKERS: [&str; 9] = [
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "private_key",
    "signing_key",
    "access_key",
    "credential",
];

/// Path segments whose URLs carry a password even when the value itself does not
/// look like it (`redis://cache:6379` is a local cache, `redis.url` is still an
/// endpoint that may gain userinfo later).
const CONNECTION_CONTEXT: [&str; 4] = ["redis", "database", "smtp", "imap"];

pub fn run(args: super::ConfigArgs, config: &CliConfig) -> Result<()> {
    match args.command {
        super::ConfigCommand::Show => show_config(config),
        super::ConfigCommand::Set { key, value } => set_config(&config.source, &key, &value),
        super::ConfigCommand::Get { key } => get_config(&config.source, &key),
        super::ConfigCommand::Reset => reset_config(&config.source),
        super::ConfigCommand::Edit => edit_config(&config.source),
    }
}

/// Print the effective configuration with every secret replaced.
///
/// The values here are the merged ones (file over defaults, then environment),
/// which is exactly the set a `ferris config show` in a CI log used to leak.
///
/// The compiled feature set goes to **stderr**: stdout stays a JSON document so
/// `ferris config show | jq` keeps working, while an operator looking at the
/// terminal still sees which subsystems this binary actually contains.
fn show_config(config: &CliConfig) -> Result<()> {
    let json = serde_json::to_string_pretty(config)?;
    let mut value: serde_json::Value = serde_json::from_str(&json)?;
    redact(&mut value, &[]);
    println!("{value:#}");
    eprintln!(
        "compiled features: {}",
        crate::features::describe(&crate::features::compiled())
    );
    Ok(())
}

/// Replace secret-looking values in place.
///
/// The decision is made on the **whole path**, not just the leaf name: any
/// segment that contains a credential marker is enough, and everything under
/// `[secrets]` is key material by definition. A URL is redacted when the path
/// says it belongs to a connection (`redis`, database, SMTP, IMAP) or when the
/// value carries userinfo — `https://example.com/hook` stays readable, as it
/// should.
fn redact(value: &mut serde_json::Value, path: &[&str]) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                redact(child, &[path, &[key.as_str()]].concat());
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                redact(child, path);
            }
        }
        leaf => {
            let text = match leaf {
                serde_json::Value::String(text) => Some(text.as_str()),
                _ => None,
            };
            if path_is_sensitive(path, text) {
                *leaf = serde_json::Value::String(REDACTED.to_string());
            }
        }
    }
}

/// Whether a value at `path` must never be printed.
fn path_is_sensitive(path: &[&str], value: Option<&str>) -> bool {
    if path
        .first()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("secrets"))
    {
        return true;
    }
    if path.iter().any(|segment| has_secret_marker(segment)) {
        return true;
    }
    let Some(name) = path.last() else {
        return false;
    };
    if !is_url_key(name) {
        return false;
    }
    let connection = path.iter().any(|segment| {
        let segment = segment.to_ascii_lowercase();
        CONNECTION_CONTEXT
            .iter()
            .any(|marker| segment.contains(marker))
    });
    connection || value.is_some_and(url_has_userinfo)
}

fn has_secret_marker(segment: &str) -> bool {
    let segment = segment.to_ascii_lowercase();
    SECRET_MARKERS.iter().any(|marker| segment.contains(marker))
}

/// `url`, `urls`, `database_url`, `webhook_urls`, … — a key whose value is an
/// address, possibly with credentials in the authority.
fn is_url_key(segment: &str) -> bool {
    let segment = segment.to_ascii_lowercase();
    segment == "url" || segment == "urls" || segment.ends_with("_url") || segment.ends_with("_urls")
}

/// `scheme://user:password@host/...` — the authority has an `@`.
fn url_has_userinfo(value: &str) -> bool {
    value.split_once("://").is_some_and(|(_, rest)| {
        let authority = rest.split('/').next().unwrap_or(rest);
        authority.contains('@')
    })
}

/// Hide the userinfo of a URL for human output (`tenant info`). The host stays
/// readable, which is the part an operator needs to check.
pub(super) fn redact_url(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return value.to_string();
    };
    let (authority, tail) = match rest.split_once('/') {
        Some((authority, tail)) => (authority, Some(tail)),
        None => (rest, None),
    };
    let Some((_userinfo, host)) = authority.rsplit_once('@') else {
        return value.to_string();
    };
    let mut out = format!("{scheme}://{REDACTED}@{host}");
    if let Some(tail) = tail {
        out.push('/');
        out.push_str(tail);
    }
    out
}

fn config_path(source: &ConfigSource) -> PathBuf {
    source.config_path.clone()
}

fn read_document(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        return Err(anyhow::anyhow!(
            "config file not found: {}. Run 'ferris config set' from a project root.",
            path.display()
        ));
    }
    let content = fs::read_to_string(path)?;
    content
        .parse::<DocumentMut>()
        .map_err(|e| anyhow::anyhow!("config file is not valid TOML: {e}"))
}

/// Descend a dotted path, creating intermediate tables when `create` is set.
/// Returns the leaf item (created as `None` when missing and `create`).
fn descend_mut<'a>(doc: &'a mut DocumentMut, path: &[&str], create: bool) -> Result<&'a mut Item> {
    let mut table = doc.as_table_mut();
    for (i, part) in path.iter().enumerate() {
        let last = i + 1 == path.len();
        if last {
            return if create {
                Ok(table.entry(part).or_insert(Item::None))
            } else {
                table
                    .get_mut(part)
                    .ok_or_else(|| anyhow::anyhow!("config key '{}' not found", path.join(".")))
            };
        }
        if !table.contains_key(part) {
            if !create {
                return Err(anyhow::anyhow!("config key '{}' not found", path.join(".")));
            }
            table.insert(part, Item::Table(Table::new()));
        }
        table = match table.get_mut(part) {
            Some(Item::Table(t)) => t,
            Some(_) => return Err(anyhow::anyhow!("config path '{part}' is not a table")),
            None => unreachable!("key was just inserted"),
        };
    }
    unreachable!("empty path")
}

/// Split a dotted key, rejecting the shapes that used to write junk.
///
/// `a..b` and `.a` split into empty segments, which `toml_edit` then stores as a
/// literal `""` key. A key that cannot be addressed by `config get` should not
/// be writable at all.
fn split_key(key: &str) -> Result<Vec<&str>> {
    if key.is_empty() {
        return Err(anyhow::anyhow!("config key must not be empty"));
    }
    let parts: Vec<&str> = key.split('.').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Err(anyhow::anyhow!(
            "config key '{key}' has an empty segment; write one name per dot, \
             e.g. 'database.name'"
        ));
    }
    Ok(parts)
}

fn infer_value(raw: &str) -> Value {
    match raw {
        "true" => Value::from(true),
        "false" => Value::from(false),
        _ => raw.parse::<i64>().map_or_else(
            |_| {
                raw.parse::<f64>()
                    .map_or_else(|_| Value::from(raw.to_string()), Value::from)
            },
            Value::from,
        ),
    }
}

fn set_config(source: &ConfigSource, key: &str, value: &str) -> Result<()> {
    let path = split_key(key)?;
    let config_path = config_path(source);
    let mut doc = read_document(&config_path)?;

    {
        let leaf = descend_mut(&mut doc, &path, true)?;
        *leaf = Item::Value(infer_value(value));
    }

    // Validate before writing: `set database.name true` infers a boolean, which
    // is not a string, and used to land on disk before the load rejected it.
    write_validated(&config_path, &doc)?;

    println!("config: {key} = {value}");
    Ok(())
}

/// Write `doc` only if it still parses as a config, so a rejected edit leaves the
/// file exactly as it was.
fn write_validated(path: &Path, doc: &DocumentMut) -> Result<()> {
    let rendered = doc.to_string();
    let parsed: toml::Value = toml::from_str(&rendered)
        .map_err(|e| anyhow::anyhow!("the edit would make {} invalid TOML: {e}", path.display()))?;
    // `load_from_file` merges over the defaults, which is the same check the CLI
    // itself applies at startup.
    let merged = merge_for_check(parsed);
    let checked: Result<CliConfig, _> = merged.try_into();
    if let Err(e) = checked {
        return Err(anyhow::anyhow!(
            "the edit would make {} an unusable config: {e}. Nothing was written.",
            path.display()
        ));
    }

    fs::write(path, rendered)?;
    Ok(())
}

/// Merge a document over the defaults the way `load_from_file` does, so the
/// check sees what the CLI would actually load.
fn merge_for_check(file_value: toml::Value) -> toml::Value {
    let mut merged = toml::Value::try_from(CliConfig::default())
        .expect("the compiled-in defaults are serializable");
    crate::config::merge_toml(&mut merged, file_value);
    merged
}

fn get_config(source: &ConfigSource, key: &str) -> Result<()> {
    let path = split_key(key)?;
    let doc = read_document(&config_path(source))?;
    let found =
        lookup(&doc, &path).ok_or_else(|| anyhow::anyhow!("config key '{key}' not found"))?;
    // `get` used to be an oracle: `show` redacted and `get database.password`
    // printed the value. The same path rule applies here, so the two commands
    // cannot disagree about what a secret is.
    println!("{}", render_item(key, &path, found));
    Ok(())
}

/// Resolve a dotted path to its item. A path that names a table resolves to the
/// table itself, so `config get server` prints the section instead of reporting
/// it missing.
fn lookup<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
    let mut item = doc.as_table().get(path.first()?)?;
    for part in path.iter().skip(1) {
        item = item.as_table()?.get(part)?;
    }
    Some(item)
}

/// Render an item the way it reads in the file: a leaf as `key = value`, a table
/// as a `[header]` followed by its contents, recursively and with the full
/// dotted path in the header.
fn render_item(key: &str, path: &[&str], item: &Item) -> String {
    match item {
        // A value taken out of a document renders with its leading whitespace;
        // trim it so the line reads like the file does.
        Item::Value(value) => {
            let rendered = value.to_string();
            let rendered = rendered.trim();
            if path_is_sensitive(path, Some(rendered)) {
                format!("{key} = \"{REDACTED}\"")
            } else {
                format!("{key} = {rendered}")
            }
        }
        Item::Table(table) => {
            let mut lines = vec![format!("[{}]", path.join("."))];
            for (k, v) in table {
                let mut child = path.to_vec();
                child.push(k);
                lines.push(render_item(k, &child, v));
            }
            lines.join("\n")
        }
        Item::ArrayOfTables(_) => format!("{} = (table array)", path.join(".")),
        Item::None => format!("config key '{}' is empty", path.join(".")),
    }
}

fn reset_config(source: &ConfigSource) -> Result<()> {
    use dialoguer::Confirm;
    let config_path = config_path(source);
    let current = read_document(&config_path)?;
    let reset = reset_document(&current)?;
    let kept = preserved_summary();

    let confirm = Confirm::new()
        .with_prompt(format!(
            "Reset {} to defaults, keeping {kept} as they are? This rewrites the file.",
            config_path.display()
        ))
        .default(false)
        .interact()?;
    if !confirm {
        println!("Reset cancelled");
        return Ok(());
    }

    write_validated(&config_path, &reset)?;
    println!(
        "Configuration reset to defaults ({})",
        config_path.display()
    );
    println!("kept: {kept} (a reset used to replace these with defaults and drop the secret key)");
    Ok(())
}

/// The preserved sections, for the prompt and the post-reset summary.
fn preserved_summary() -> String {
    let mut kept: Vec<String> = PRESERVED_ON_RESET
        .iter()
        .map(|section| (*section).to_string())
        .collect();
    kept.extend(
        PRESERVED_SUBTABLES
            .iter()
            .map(|(section, key)| format!("{section}.{key}")),
    );
    kept.join(", ")
}

/// The defaults as a document, with this machine's service and key-material
/// sections carried over from `current`.
///
/// `[secrets] key` is the one thing a reset must never drop: without it every
/// encrypted `Secret` field in the database becomes unreadable, which is a data
/// loss no "reset the config" command is allowed to cause. TLS/PKI follows for
/// the same kind of reason — a reset is not a downgrade to plain HTTP.
fn reset_document(current: &DocumentMut) -> Result<DocumentMut> {
    let mut reset: DocumentMut = toml::to_string_pretty(&CliConfig::default())?
        .parse()
        .map_err(|e| anyhow::anyhow!("the defaults are not valid TOML: {e}"))?;

    for section in PRESERVED_ON_RESET {
        // A section that is not in the file stays absent: inventing one would
        // add config the project never had.
        if let Some(existing) = current.get(section) {
            reset.insert(section, existing.clone());
        }
    }

    for (section, key) in PRESERVED_SUBTABLES {
        let Some(existing) = current.get(section).and_then(|item| item.get(key)) else {
            continue;
        };
        // The defaults always contain `[server]`, so a missing table here is not
        // a state to invent; the section simply keeps its default.
        if let Some(target) = reset
            .as_table_mut()
            .get_mut(section)
            .and_then(Item::as_table_mut)
        {
            target.insert(key, existing.clone());
        }
    }
    Ok(reset)
}

fn edit_config(source: &ConfigSource) -> Result<()> {
    let config_path = config_path(source);
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());
    let status = editor_command(&editor).arg(&config_path).status()?;

    if !status.success() {
        return Err(anyhow::anyhow!("Editor failed"));
    }

    // Nothing is "reloaded" by this: the command has already exited by the time a
    // process could observe it. It does verify that what the editor left behind
    // is a config the CLI can load.
    CliConfig::load(Some(&config_path.to_string_lossy()), None)?;
    println!("Configuration saved: {}", config_path.display());
    Ok(())
}

/// `EDITOR` may carry arguments (`code --wait`, `emacsclient -nw`), which
/// `Command::new` would take for a program name and fail to spawn.
fn editor_command(editor: &str) -> std::process::Command {
    let mut parts = editor.split_whitespace();
    let mut command = std::process::Command::new(parts.next().unwrap_or("vi"));
    command.args(parts);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    const SQLITE_CONFIG: &str = r#"
[project]
name = "my-app"

[database]
type = "sqlite"
name = "my-app.db"
password = "hunter2"

[redis]
url = "redis://:s3cret@cache.internal:6379"
"#;

    fn parse(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    #[test]
    fn secrets_never_reach_the_output() {
        let mut config = CliConfig::default();
        config.auth.jwt_secret = Some("super-secret-value".into());
        config.database.password = "hunter2".into();
        config.redis.url = "redis://:s3cret@cache.internal:6379".into();

        let mut json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        redact(&mut json, &[]);

        assert_eq!(json["auth"]["jwt_secret"], REDACTED);
        assert_eq!(json["database"]["password"], REDACTED);
        assert_eq!(json["redis"]["url"], REDACTED);
        let rendered = serde_json::to_string(&json).unwrap();
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("s3cret"), "{rendered}");
        assert!(!rendered.contains("super-secret-value"), "{rendered}");
        // Everything else is still there: a redacted config is still useful.
        assert_eq!(json["database"]["name"], "ferris_dev");
        assert_eq!(json["redis"]["max_connections"], 20);
    }

    #[test]
    fn secrets_nested_in_custom_are_redacted_too() {
        let mut config = CliConfig::default();
        config
            .custom
            .insert("stripe".into(), serde_json::json!({"api_key": "sk_live_x"}));
        let mut json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        redact(&mut json, &[]);
        assert_eq!(json["custom"]["stripe"]["api_key"], REDACTED);
    }

    /// The regression for the exact-name list: every one of these used to reach
    /// stdout, including the master key that decrypts every `Secret` field.
    #[test]
    fn redaction_covers_the_whole_path_not_an_exact_name() {
        let mut json = serde_json::json!({
            "secrets": {
                "key": "6f1c-in-real-life",
                "previous_keys": ["old-key-material"]
            },
            "email": {"smtp_password": "hunter2"},
            "files": {"s3": {"secret_access_key": "AKIAEXAMPLE"}},
            "custom": {"database_url": "postgres://app:pw@db.internal/app"},
            "redis": {"url": "redis://:cachepw@cache.internal:6379"},
            "server": {"port": 8000},
            "webhooks": {"url": "https://example.com/hook"}
        });
        redact(&mut json, &[]);

        assert_eq!(json["secrets"]["key"], REDACTED);
        assert_eq!(json["secrets"]["previous_keys"][0], REDACTED);
        assert_eq!(json["email"]["smtp_password"], REDACTED);
        assert_eq!(json["files"]["s3"]["secret_access_key"], REDACTED);
        assert_eq!(json["custom"]["database_url"], REDACTED);
        assert_eq!(json["redis"]["url"], REDACTED);
        // Not a secret, and still useful: the port and a plain webhook URL.
        assert_eq!(json["server"]["port"], 8000);
        assert_eq!(json["webhooks"]["url"], "https://example.com/hook");

        let rendered = serde_json::to_string(&json).unwrap();
        for secret in [
            "6f1c-in-real-life",
            "old-key-material",
            "hunter2",
            "AKIAEXAMPLE",
            "pw@db",
            "cachepw",
        ] {
            assert!(!rendered.contains(secret), "{secret} leaked: {rendered}");
        }
    }

    /// `config get` is not an oracle: it renders the same redacted value as
    /// `config show`, at any depth.
    #[test]
    fn config_get_renders_secrets_redacted() {
        let doc = parse(&format!(
            "{SQLITE_CONFIG}\n[server]\nport = 8000\n\n[secrets]\nkey = \"master-key\"\n"
        ));

        let database = render_item(
            "database",
            &["database"],
            lookup(&doc, &["database"]).unwrap(),
        );
        assert!(database.contains("password = \"<redacted>\""), "{database}");
        assert!(!database.contains("hunter2"), "{database}");
        assert!(database.contains("type = \"sqlite\""), "{database}");

        let key = render_item(
            "key",
            &["secrets", "key"],
            lookup(&doc, &["secrets", "key"]).unwrap(),
        );
        assert_eq!(key, "key = \"<redacted>\"");
        assert!(!key.contains("master-key"), "{key}");

        let port = render_item(
            "port",
            &["server", "port"],
            lookup(&doc, &["server", "port"]).unwrap(),
        );
        assert_eq!(port, "port = 8000");
    }

    /// `tenant info` prints the Redis endpoint; the userinfo is the password.
    #[test]
    fn tenant_urls_hide_their_userinfo_but_keep_the_host() {
        assert_eq!(
            redact_url("redis://:pw@cache.internal:6379/0"),
            "redis://<redacted>@cache.internal:6379/0"
        );
        assert_eq!(
            redact_url("redis://cache.internal:6379"),
            "redis://cache.internal:6379"
        );
        assert_eq!(redact_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn getting_a_table_prints_the_table() {
        let doc = parse(SQLITE_CONFIG);
        let found = lookup(&doc, &["database"]).expect("`database` is a section");
        // The bug was that a table path left the leaf unset and the caller
        // reported "not found".
        assert!(matches!(found, Item::Table(_)));
        assert!(lookup(&doc, &["database", "type"]).is_some());
        assert!(lookup(&doc, &["database", "nope"]).is_none());
    }

    #[test]
    fn a_key_with_an_empty_segment_is_refused() {
        assert!(split_key("a..b").is_err());
        assert!(split_key(".a").is_err());
        assert!(split_key("a.").is_err());
        assert!(split_key("").is_err());
        assert_eq!(split_key("database.name").unwrap(), ["database", "name"]);
    }

    #[test]
    fn an_edit_that_would_break_the_config_is_not_written() {
        let dir = std::env::temp_dir().join(format!("ferris-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ferris.toml");
        std::fs::write(&path, SQLITE_CONFIG).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        // `name` is a string; a boolean does not load.
        let mut doc = parse(&before);
        let leaf = descend_mut(&mut doc, &["database", "name"], true).unwrap();
        *leaf = Item::Value(infer_value("true"));
        let err = write_validated(&path, &doc).expect_err("a bool where a string belongs");
        assert!(err.to_string().contains("Nothing was written"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        // A valid edit lands.
        let mut doc = parse(&before);
        let leaf = descend_mut(&mut doc, &["database", "name"], true).unwrap();
        *leaf = Item::Value(infer_value("renamed.db"));
        write_validated(&path, &doc).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("renamed.db")
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reset_keeps_the_database_and_redis_sections() {
        let current = parse(SQLITE_CONFIG);
        let reset = reset_document(&current).unwrap();

        assert_eq!(
            reset["database"]["type"].as_str(),
            Some("sqlite"),
            "a reset must not repoint a sqlite project at the postgres default"
        );
        assert_eq!(reset["database"]["name"].as_str(), Some("my-app.db"));
        assert_eq!(
            reset["redis"]["url"].as_str(),
            Some("redis://:s3cret@cache.internal:6379")
        );
        // Everything else comes from the defaults.
        assert_eq!(reset["server"]["port"].as_integer(), Some(8000));
        assert_eq!(
            reset["project"]["name"].as_str(),
            Some("ferris-app"),
            "the project section is a default, not a machine service"
        );
    }

    #[test]
    fn reset_keeps_the_secret_key_tls_and_pki() {
        let current = parse(
            r#"
[secrets]
key = "master-key"
previous_keys = ["old-key"]

[server.tls]
cert_path = "/etc/ferris/fullchain.pem"
key_path = "/etc/ferris/privkey.pem"

[server.pki]
ca_cert_path = "/etc/ferris/ca.pem"
ca_key_path = "/etc/ferris/ca-key.pem"
default_ttl_seconds = 3600
"#,
        );
        let reset = reset_document(&current).unwrap();

        // Without the key every encrypted `Secret` field becomes unreadable.
        assert_eq!(reset["secrets"]["key"].as_str(), Some("master-key"));
        let previous = reset["secrets"]["previous_keys"]
            .as_array()
            .expect("previous_keys stays an array");
        assert_eq!(
            previous.get(0).and_then(|key| key.as_str()),
            Some("old-key")
        );

        // A reset is not a silent downgrade to plain HTTP.
        assert_eq!(
            reset["server"]["tls"]["cert_path"].as_str(),
            Some("/etc/ferris/fullchain.pem")
        );
        assert_eq!(
            reset["server"]["pki"]["ca_key_path"].as_str(),
            Some("/etc/ferris/ca-key.pem")
        );
        // The rest of `[server]` still comes from the defaults.
        assert_eq!(reset["server"]["port"].as_integer(), Some(8000));
    }

    #[test]
    fn reset_writes_a_complete_config_and_only_spares_the_preserved_sections() {
        let current = parse("[server]\nport = 9000\n");
        let reset = reset_document(&current).unwrap();

        // Everything the defaults know about is written out; that is what "reset"
        // means, and it is what the CLI would load anyway with a partial file.
        assert_eq!(reset["server"]["port"].as_integer(), Some(8000));
        assert!(reset["features"]["realtime"].is_bool());
        // The file had no database section, so there is nothing to preserve and
        // the default stands — the load already merged it in before this point.
        assert_eq!(reset["database"]["type"].as_str(), Some("postgres"));
        assert!(reset.get("features").is_some(), "{reset}");
    }

    #[test]
    fn an_editor_with_arguments_is_split_into_program_and_args() {
        let command = editor_command("code --wait");
        let program = command.get_program().to_string_lossy().into_owned();
        assert_eq!(program, "code");
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["--wait"]);
    }

    #[test]
    fn an_editor_without_arguments_is_just_the_program() {
        let command = editor_command("vim");
        assert_eq!(command.get_program().to_string_lossy(), "vim");
        assert_eq!(command.get_args().count(), 0);
    }
}
