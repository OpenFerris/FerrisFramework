#[cfg(feature = "wasm")]
use std::sync::Arc;

use ferris_core::FeatureFlags;
#[cfg(feature = "wasm")]
use ferris_wasm::{PluginStore, RuntimeConfig, SharedRuntime};

#[cfg(feature = "wasm")]
use super::boot::start_plugin_clock;
use super::{bootstrap_authz, gate_roles};
use crate::host_features;
use crate::roles::{ServerRole, ServerRoles};

/// Serialise the engine-touching tests in this binary against each other:
/// the schema registry and the global engine are both process-wide.
static SERIALISE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Isolates an engine-touching test: holds the serialise lock and restores
/// the engine the test replaced. Both the schema registry and the global
/// engine are process-wide.
struct BootGuard {
    _serialised: std::sync::MutexGuard<'static, ()>,
    previous: ferris_auth::permissions::PermissionEngine,
}
impl Drop for BootGuard {
    fn drop(&mut self) {
        ferris_auth::permissions::set_global_engine(self.previous.clone());
    }
}

async fn isolated() -> (ferris_db::Db, BootGuard) {
    let _serialised = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
    let pool = ferris_db::memory_db().await.unwrap();
    let previous = ferris_auth::permissions::global_engine_ref()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    (
        pool,
        BootGuard {
            _serialised,
            previous,
        },
    )
}

/// §C3(e): the boot sequence is the whole path between an operator's policy
/// file on disk and the engine that answers every request — `run()` reads
/// `settings.policy_path`, registers the core DocTypes, and installs. Pin
/// that file lands in the *live* engine by running the same sequence with a
/// real file and asking the installed engine about a role only that file
/// grants.
///
/// `Notification` declares no DocType rules on purpose — its recipient owns
/// it via the owner rule alone — so `Audit-Lens` reads it *only* through the
/// operator document. If the boot path dropped the file, this test would read
/// "no matching statement" and fail.
#[tokio::test]
async fn the_boot_path_reaches_the_engine_with_the_operator_document() {
    let (pool, _guard) = isolated().await;
    let unique = format!("ferris-boot-{}-{}", std::process::id(), line!());
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).unwrap();

    let file = dir.join("policies.json");
    std::fs::write(
        &file,
        r#"[{"version":"1","statements":[{"effect":"allow",
            "principals":[{"principal_type":"role","identifier":"Audit-Lens"}],
            "actions":["read"],
            "resources":[{"resource_type":"Notification","name":"*","owner":null}],
            "conditions":{}}]}]"#,
    )
    .unwrap();

    let modules = crate::router::modules::native_modules(&ferris_core::FeatureFlags::default());
    let report = bootstrap_authz(&pool, Some(file.as_path()), &modules)
        .await
        .unwrap();
    assert_eq!(report.json_policies, 1);

    let allowed = ferris_api::authz::explain(
        "Notification",
        &["Audit-Lens"],
        "sam@example.com",
        None,
        ferris_auth::permissions::Permission::Read,
    );
    assert!(allowed.contains("allow"), "{allowed}");

    let denied = ferris_api::authz::explain(
        "Notification",
        &["Some Other Role"],
        "sam@example.com",
        None,
        ferris_auth::permissions::Permission::Read,
    );
    assert!(denied.contains("no matching statement"), "{denied}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The flip side of the same sequence: an absent policy path is not a boot
/// failure, so the engine still answers from the DocType-declared rules alone.
#[tokio::test]
async fn the_boot_path_tolerates_a_missing_policy_file() {
    let (pool, _guard) = isolated().await;
    let absent = std::env::temp_dir().join(format!(
        "ferris-boot-absent-{}-{}",
        std::process::id(),
        line!()
    ));
    let modules = crate::router::modules::native_modules(&ferris_core::FeatureFlags::default());
    let report = bootstrap_authz(&pool, Some(absent.as_path()), &modules)
        .await
        .unwrap();
    assert_eq!(report.json_policies, 0);
}

/// §C3(a)+C3(e) together: a persisted Role Permissions row joins the same
/// boot install as the registry and the operator document. `Notification`
/// declares no DocType rules, so `Audit-Lens` reads it *only* through the
/// stored row.
#[tokio::test]
async fn the_boot_path_loads_persisted_role_permissions() {
    let (pool, _guard) = isolated().await;

    let store = ferris_auth::docperm::DocPermStore::new(pool.clone());
    store.init().await.unwrap();
    store
        .upsert(&ferris_auth::docperm::DocPermRow {
            doctype: "Notification".to_string(),
            role: "Audit-Lens".to_string(),
            read: true,
            ..Default::default()
        })
        .await
        .unwrap();

    let modules = crate::router::modules::native_modules(&ferris_core::FeatureFlags::default());
    let report = bootstrap_authz(&pool, None, &modules).await.unwrap();
    assert!(
        report.statements > 0,
        "the stored row should contribute statements"
    );

    let allowed = ferris_api::authz::explain(
        "Notification",
        &["Audit-Lens"],
        "sam@example.com",
        None,
        ferris_auth::permissions::Permission::Read,
    );
    assert!(allowed.contains("allow"), "{allowed}");

    let denied = ferris_api::authz::explain(
        "Notification",
        &["No Such Role"],
        "sam@example.com",
        None,
        ferris_auth::permissions::Permission::Read,
    );
    assert!(denied.contains("no matching statement"), "{denied}");
}

/// §C4 regression: `Server::run` reaches for this seam at boot — a store
/// with epoch interrupts enabled must get a live clock. That clock is what
/// makes the CPU cap real: the host arms an epoch deadline around every
/// plugin call, and before C4 nothing bumped the engine's epoch, so a
/// plugin in an infinite loop hung its worker forever while looking
/// protected. The guest below never returns; it completes (and the test
/// fails) only when no clock was started.
#[cfg(feature = "wasm")]
#[tokio::test]
async fn c4_the_boot_starts_a_clock_that_interrupts_runaway_guests() {
    let (pool, _guard) = isolated().await;
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            // A 1 ms tick keeps the trap fast without racing the guest.
            epoch_increment_ms: 1,
            ..Default::default()
        })
        .unwrap(),
    );
    let store = Arc::new(PluginStore::new(runtime.clone(), pool));

    // The seam `Server::run` calls at boot:
    let clock = start_plugin_clock(Some(&store))
        .expect("a store with epoch interrupts enabled must get a boot clock");
    assert!(!clock.is_finished(), "the boot clock must stay alive");

    // A guest that never returns, under the same deadline the host arms on
    // every plugin call. The clock keeps bumping the engine's epoch from
    // this tokio runtime while the guest spins on a worker thread, so the
    // deadline fires and the call traps instead of hanging the worker.
    let spin = wat::parse_str(
        r#"(module
            (func $busy nop)
            (func (export "spin")
                (loop (call $busy) (br 0))))"#,
    )
    .expect("sample wat");
    let module = wasmtime::Module::new(runtime.engine(), spin).expect("compile spin");

    let engine = runtime.engine().clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_epoch_deadline(1);
        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("instantiate spin");
        let spin = instance
            .get_typed_func::<(), ()>(&mut store, "spin")
            .expect("typed func");
        let _ = tx.send(spin.call(&mut store, ()));
    });

    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(Err(_))) => {}
        Ok(Ok(Ok(()))) => {
            panic!("C4 regression: the runaway guest completed without being interrupted")
        }
        Ok(Err(_)) => panic!("spin thread ended without reporting"),
        Err(_) => {
            panic!("C4 regression: the runaway guest is still running — no live boot clock")
        }
    }
}

/// The flip side of the same seam: no plugin store configured, or a store
/// whose runtime has interrupts off — the exact configuration the dev
/// server used to run with — must not pretend a clock exists.
#[cfg(feature = "wasm")]
#[tokio::test]
async fn c4_no_clock_without_plugins_or_with_interrupts_disabled() {
    assert!(start_plugin_clock(None).is_none());

    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .unwrap(),
    );
    let store = Arc::new(PluginStore::new(
        runtime,
        ferris_db::memory_db().await.unwrap(),
    ));
    assert!(start_plugin_clock(Some(&store)).is_none());
}

#[test]
fn gate_roles_removes_only_the_disabled_features() {
    let mut features = FeatureFlags::default();
    features.realtime = false;
    features.jobs = false;

    let gated = gate_roles(&ServerRoles::all(), &features).unwrap();
    assert!(!gated.has(ServerRole::Realtime));
    assert!(!gated.has(ServerRole::Worker));
    assert!(gated.has(ServerRole::Api));
    // `true` keeps a compiled role and still cannot resurrect a compiled-out
    // one: the matrix runs both ways.
    assert_eq!(
        gated.has(ServerRole::Web),
        host_features::is_compiled("web")
    );
    assert_eq!(
        gated.has(ServerRole::Scheduler),
        host_features::is_compiled("scheduler")
    );
}

#[test]
fn gate_roles_true_never_adds_a_role() {
    let features = FeatureFlags::default();
    let only_api = ServerRoles::parse(&["api".to_string()]).unwrap();
    let gated = gate_roles(&only_api, &features).unwrap();
    assert!(gated.has(ServerRole::Api));
    for role in [
        ServerRole::Web,
        ServerRole::Realtime,
        ServerRole::Worker,
        ServerRole::Scheduler,
    ] {
        assert!(!gated.has(role), "a feature flag must not add {role}");
    }
}

#[test]
fn gate_roles_can_leave_nothing_enabled() {
    let mut features = FeatureFlags::default();
    features.realtime = false;
    features.jobs = false;
    features.scheduler = false;
    let background = ServerRoles::parse(&[
        "realtime".to_string(),
        "worker".to_string(),
        "scheduler".to_string(),
    ])
    .unwrap();
    assert!(gate_roles(&background, &features).unwrap().is_empty());
}
