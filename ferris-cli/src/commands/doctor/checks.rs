//! One check per concern: toolchain, project layout, hub services, secrets.

use std::path::Path;
use std::process::Command;

use crate::config::CliConfig;

use super::health::HealthCheck;

pub(super) fn check_rust_toolchain() -> HealthCheck {
    match Command::new("rustc").arg("--version").output() {
        Ok(out) if out.status.success() => HealthCheck {
            name: "Rust toolchain".to_string(),
            passed: true,
            message: format!("rustc {}", String::from_utf8_lossy(&out.stdout).trim()),
            critical: true,
        },
        _ => HealthCheck {
            name: "Rust toolchain".to_string(),
            passed: false,
            message: "rustc not found or failed".to_string(),
            critical: true,
        },
    }
}

pub(super) fn check_cargo() -> HealthCheck {
    match Command::new("cargo").arg("--version").output() {
        Ok(out) if out.status.success() => HealthCheck {
            name: "Cargo".to_string(),
            passed: true,
            message: format!("cargo {}", String::from_utf8_lossy(&out.stdout).trim()),
            critical: true,
        },
        _ => HealthCheck {
            name: "Cargo".to_string(),
            passed: false,
            message: "cargo not found or failed".to_string(),
            critical: true,
        },
    }
}

/// Every installed app's translation catalogues parse and cover the declared
/// locales.
///
/// The boot skips a broken file (a translation must not take a site down), so a
/// typo would otherwise only show as an English label in one language.
pub(super) fn check_locales(bench_root: &Path) -> HealthCheck {
    let apps = bench_root.join("apps");
    let mut catalogues = 0_usize;
    let mut broken: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&apps) {
        let mut apps: Vec<_> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .collect();
        apps.sort_by_key(std::fs::DirEntry::file_name);
        for app in apps {
            match crate::commands::app_catalogues::validate(&app.path(), None) {
                Ok(tags) => catalogues += tags.len(),
                Err(error) => {
                    broken.push(format!("{}: {error}", app.file_name().to_string_lossy()))
                }
            }
        }
    }

    if broken.is_empty() {
        HealthCheck {
            name: "App catalogues".to_string(),
            passed: true,
            message: if catalogues == 0 {
                "no app translation catalogues installed".to_string()
            } else {
                format!("{catalogues} catalogue(s) are valid")
            },
            critical: true,
        }
    } else {
        HealthCheck {
            name: "App catalogues".to_string(),
            passed: false,
            message: broken.join("; "),
            // A skipped file degrades one language, it does not stop the boot.
            critical: false,
        }
    }
}

pub(super) fn check_cargo_toml(cwd: &Path, bench_root: &Path) -> HealthCheck {
    let path = cwd.join("Cargo.toml");
    if path.is_file() {
        HealthCheck {
            name: "Cargo.toml".to_string(),
            passed: true,
            message: "found".to_string(),
            critical: true,
        }
    } else if cwd == bench_root && bench_root.join("apps").is_dir() {
        HealthCheck {
            name: "Cargo.toml".to_string(),
            passed: true,
            message: "bench root: app crates live under apps/".to_string(),
            critical: true,
        }
    } else {
        HealthCheck {
            name: "Cargo.toml".to_string(),
            passed: false,
            message: "not found in current directory".to_string(),
            critical: true,
        }
    }
}

pub(super) fn check_project_structure(cwd: &Path, bench_root: &Path) -> HealthCheck {
    // Two layouts: the bench root owns `apps/` + `sites/`; an app directory
    // owns `src/` + `wit/`. Requiring `config/` inside every app is why doctor
    // failed on the scaffold once the bench config moved to the root.
    if cwd == bench_root {
        let missing: Vec<&str> = ["apps", "sites"]
            .into_iter()
            .filter(|dir| !cwd.join(dir).is_dir())
            .collect();
        return if missing.is_empty() {
            HealthCheck {
                name: "Project structure".to_string(),
                passed: true,
                message: "bench layout: apps/ and sites/ present".to_string(),
                critical: true,
            }
        } else {
            HealthCheck {
                name: "Project structure".to_string(),
                passed: false,
                message: format!("missing bench directories: {}", missing.join(", ")),
                critical: true,
            }
        };
    }

    let missing_src = !cwd.join("src").is_dir();
    let wit_message = if cwd.join("wit/ferris.wit").is_file() {
        "WIT interface present".to_string()
    } else {
        "wit/ferris.wit missing (components are built from it)".to_string()
    };
    if missing_src {
        HealthCheck {
            name: "Project structure".to_string(),
            passed: false,
            message: format!("missing directory: src ({wit_message})"),
            critical: true,
        }
    } else {
        HealthCheck {
            name: "Project structure".to_string(),
            passed: true,
            message: format!("src/ present; {wit_message}"),
            critical: true,
        }
    }
}

/// The WIT copy must match the contract this binary links.
///
/// A stale `wit/ferris.wit` compiles cleanly and produces a component the host
/// refuses to instantiate. Catching it here is the difference between a
/// five-second fix and a server that will not boot.
pub(super) fn check_wit_version(cwd: &Path, bench_root: &Path) -> HealthCheck {
    if cwd == bench_root {
        return HealthCheck {
            name: "WIT version".to_string(),
            passed: true,
            message: "bench root: each app carries its own wit/ferris.wit".to_string(),
            critical: true,
        };
    }
    let path = cwd.join("wit/ferris.wit");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HealthCheck {
            name: "WIT version".to_string(),
            passed: false,
            message: format!("{} not found", path.display()),
            critical: true,
        };
    };
    let expected = env!("CARGO_PKG_VERSION");
    let Some(found) = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("package ferris:framework@")
            .and_then(|rest| rest.trim_end_matches(';').split_whitespace().next())
            .map(str::to_string)
    }) else {
        return HealthCheck {
            name: "WIT version".to_string(),
            passed: false,
            message: "wit/ferris.wit has no `package ferris:framework@<version>;` line".to_string(),
            critical: true,
        };
    };
    if found == expected {
        HealthCheck {
            name: "WIT version".to_string(),
            passed: true,
            message: format!("contract {found} matches the host"),
            critical: true,
        }
    } else {
        HealthCheck {
            name: "WIT version".to_string(),
            passed: false,
            message: format!(
                "wit/ferris.wit is {found} but this ferris links {expected}; \
                 copy the host's ferris-wasm/wit/ferris.wit and rebuild"
            ),
            critical: true,
        }
    }
}

pub(super) fn check_config_files(config_path: &Path) -> HealthCheck {
    if config_path.is_file() {
        HealthCheck {
            name: "Config files".to_string(),
            passed: true,
            message: format!("{} present", config_path.display()),
            critical: true,
        }
    } else {
        HealthCheck {
            name: "Config files".to_string(),
            passed: false,
            message: format!("{} not found", config_path.display()),
            critical: true,
        }
    }
}

pub(super) fn check_database(config: &CliConfig) -> HealthCheck {
    use crate::commands::db_map::{core_database, is_sqlite, sqlite_file};

    if is_sqlite(&config.database) {
        let path = sqlite_file(&config.database);
        let existed = path.exists();
        let message = if existed {
            format!("SQLite database found at {}", path.display())
        } else {
            format!(
                "SQLite database will be created at {} on first use",
                path.display()
            )
        };
        return HealthCheck {
            name: "Database configuration".to_string(),
            passed: true,
            message,
            critical: existed,
        };
    }

    // A type or an ssl_mode this build does not understand is a failed check,
    // not a reason to fall back to a default: "the database is unreachable" and
    // "I read your config wrong" are different problems with different fixes.
    let database = match core_database(&config.database, config.database.name.clone()) {
        Ok(database) => database,
        Err(e) => {
            return HealthCheck {
                name: "Database configuration".to_string(),
                passed: false,
                message: format!("{e}"),
                critical: false,
            };
        }
    };
    let rt = match crate::runtime::block_on_runtime(None) {
        Ok(rt) => rt,
        Err(e) => {
            return HealthCheck {
                name: "Database configuration".to_string(),
                passed: false,
                message: format!("failed to start runtime: {e}"),
                critical: false,
            };
        }
    };

    let db_type = database.r#type;
    let result = rt.block_on(async {
        let db = ferris_db::create_pool(&database)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        db.health_check().await.map_err(|e| anyhow::anyhow!("{e}"))
    });

    match result {
        Ok(()) => HealthCheck {
            name: "Database configuration".to_string(),
            passed: true,
            message: format!(
                "reachable {db_type:?} at {}:{}",
                config.database.host, config.database.port
            ),
            critical: true,
        },
        Err(e) => HealthCheck {
            name: "Database configuration".to_string(),
            passed: false,
            message: format!(
                "cannot reach {db_type:?} at {}:{}: {e}",
                config.database.host, config.database.port
            ),
            critical: false,
        },
    }
}

/// Whether anything in this config needs Redis. Only `realtime` does: the job
/// queue and the scheduler fall back to the durable `jobs` table when no Redis
/// is reachable (R-FW-1a), so `jobs.enabled` alone is not a hard requirement.
pub(super) fn redis_required(config: &CliConfig) -> bool {
    config.realtime.enabled
}

/// Report `Secret` values that predate encryption.
///
/// A field declared `Password` used to be hashed on write, so an existing
/// installation has Argon2 strings in `client_secret`, `webhook_secret`, … Those
/// cannot be decrypted; the operator has to re-enter them. This check is the
/// only place that says so without waiting for the integration to fail, and it
/// is deliberately **not** critical: an install that never used an integration
/// has nothing to fix.
pub(super) fn check_secret_fields(config: &CliConfig) -> HealthCheck {
    const NAME: &str = "Secret fields";

    if let Err(error) = ferris_core::secret::keyring_from_config(&config.secrets) {
        return HealthCheck {
            name: NAME.to_string(),
            passed: false,
            message: format!("[secrets] key is unusable: {error}"),
            critical: false,
        };
    }

    use crate::commands::db_map::core_database;
    let database = match core_database(&config.database, config.database.name.clone()) {
        Ok(database) => database,
        Err(e) => {
            return HealthCheck {
                name: NAME.to_string(),
                passed: true,
                message: format!("skipped: database configuration is invalid ({e})"),
                critical: false,
            };
        }
    };
    let Ok(rt) = crate::runtime::block_on_runtime(None) else {
        return HealthCheck {
            name: NAME.to_string(),
            passed: true,
            message: "skipped: no async runtime".to_string(),
            critical: false,
        };
    };

    ferris_server::catalog::register_compiled_doctype_catalog();
    let mut user = ferris_core::User::system_user("doctor".to_string(), "Doctor".to_string());
    user.roles = vec!["System Manager".to_string()];

    let report = rt.block_on(async move {
        let db = ferris_db::create_pool(&database).await?;
        let engine = ferris_document::DocumentEngine::new(db);
        let mut stale: Vec<String> = Vec::new();
        for meta in ferris_schema::list_doctypes_metas() {
            let secret_fields: Vec<&str> = meta
                .fields
                .iter()
                .filter(|field| field.fieldtype == "Secret")
                .map(|field| field.fieldname.as_str())
                .collect();
            if secret_fields.is_empty() {
                continue;
            }
            // No metadata in the context on purpose: this is a maintenance scan
            // of the *stored* blob, and a redacted read would hand back the mask.
            let ctx = ferris_document::EngineContext::new(meta.name.clone(), "").with_user(&user);
            let query = ferris_document::ListQuery {
                limit: Some(10_000),
                ..Default::default()
            };
            let Ok((rows, _total)) = engine.list(&ctx, &query).await else {
                continue;
            };
            for field in secret_fields {
                let count = rows
                    .iter()
                    .filter(|row| {
                        row.get(field)
                            .and_then(ferris_core::Value::as_str)
                            .is_some_and(|value| {
                                !value.is_empty() && !ferris_core::secret::is_secret_envelope(value)
                            })
                    })
                    .count();
                if count > 0 {
                    stale.push(format!("{}.{field} ({count} row(s))", meta.name));
                }
            }
        }
        Ok::<Vec<String>, ferris_core::Error>(stale)
    });

    match report {
        Ok(stale) if stale.is_empty() => HealthCheck {
            name: NAME.to_string(),
            passed: true,
            message: "every stored secret is encrypted".to_string(),
            critical: false,
        },
        Ok(stale) => HealthCheck {
            name: NAME.to_string(),
            passed: false,
            message: format!(
                "pre-encryption secret value(s) need the value re-entered in the Desk: {}",
                stale.join(", ")
            ),
            critical: false,
        },
        Err(e) => HealthCheck {
            name: NAME.to_string(),
            passed: true,
            message: format!("skipped: {e}"),
            critical: false,
        },
    }
}

pub(super) fn check_redis(config: &CliConfig) -> HealthCheck {
    let url = config.redis.url.trim();

    if !redis_required(config) {
        return HealthCheck {
            name: "Redis".to_string(),
            passed: true,
            message: "not required: realtime is disabled".to_string(),
            critical: false,
        };
    }

    if url.is_empty() {
        return HealthCheck {
            name: "Redis".to_string(),
            passed: false,
            message: "realtime is enabled but redis.url is empty".to_string(),
            critical: false,
        };
    }

    let client = match redis::Client::open(url) {
        Ok(client) => client,
        Err(e) => {
            return HealthCheck {
                name: "Redis".to_string(),
                passed: false,
                message: format!("cannot parse {url}: {e}"),
                critical: false,
            };
        }
    };
    let mut conn = match client.get_connection() {
        Ok(conn) => conn,
        Err(e) => {
            return HealthCheck {
                name: "Redis".to_string(),
                passed: false,
                message: format!("cannot connect to {url}: {e}"),
                critical: false,
            };
        }
    };
    match redis::cmd("PING").query::<String>(&mut conn) {
        Ok(pong) => HealthCheck {
            name: "Redis".to_string(),
            passed: true,
            message: format!("PING -> {pong} at {url}"),
            critical: false,
        },
        Err(e) => HealthCheck {
            name: "Redis".to_string(),
            passed: false,
            message: format!("PING failed: {e}"),
            critical: false,
        },
    }
}

pub(super) fn check_wasm_target() -> HealthCheck {
    let Ok(out) = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    else {
        return HealthCheck {
            name: "WASM target".to_string(),
            passed: false,
            message: "rustup not found; install Rust via rustup for WASM component builds"
                .to_string(),
            critical: false,
        };
    };
    if !out.status.success() {
        return HealthCheck {
            name: "WASM target".to_string(),
            passed: false,
            message: "rustup target list failed".to_string(),
            critical: false,
        };
    }
    let installed = String::from_utf8_lossy(&out.stdout).to_string();
    // Which target is installed is worth saying: the message used to claim
    // `wasm32-wasip2` whatever matched, and `wasm32-unknown-unknown` is a
    // different target with a different loader.
    let found = ["wasm32-wasip2", "wasm32-wasip1", "wasm32-unknown-unknown"]
        .iter()
        .find(|t| installed.lines().any(|line| line.trim() == **t));
    HealthCheck {
        name: "WASM target".to_string(),
        passed: found.is_some(),
        message: match found {
            Some(target) => format!("WASM target installed ({target})"),
            None => "no WASM target installed; run `rustup target add wasm32-wasip2`".to_string(),
        },
        critical: false,
    }
}
