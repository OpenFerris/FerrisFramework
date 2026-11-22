//! `platform::process-*`: the host forks, the app only names the program.

use std::path::PathBuf;
use std::time::Duration;

use ferris_wasm::{ProcessService, ProcessSpec};

use super::support;

fn spec(program: &str, args: &[&str]) -> ProcessSpec {
    ProcessSpec {
        program: program.to_string(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        env: Vec::new(),
        cwd: None,
        stdin: None,
        timeout_ms: Some(u32::try_from(Duration::from_secs(10).as_millis()).unwrap_or(10_000)),
    }
}

fn world_root(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("ferris-proc-world-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("probe")).expect("world");
    // The supervisor no longer falls back to the host `PATH` (WASM-004): the
    // program must live in the app world. A tiny trampoline keeps the tests
    // asserting the same `sh -c` behaviour without granting `sh`.
    let tool = root.join("probe/tool.sh");
    std::fs::write(&tool, "#!/bin/sh\nexec \"$@\"\n").expect("tool script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
            .expect("tool is executable");
    }
    root
}

#[tokio::test]
async fn a_program_runs_inside_the_app_world() {
    let root = world_root("run");
    let store = support::store_with_worlds(root.clone()).await;
    let supervisor = ferris_server::plugin_process::ProcessSupervisor::new(store);
    let output = supervisor
        .run("probe", &spec("tool.sh", &["printf", "hello"]))
        .await
        .expect("the program runs");
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout, b"hello");
    assert!(!output.timed_out);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_program_that_outlives_its_deadline_is_killed() {
    let root = world_root("timeout");
    let store = support::store_with_worlds(root.clone()).await;
    let supervisor = ferris_server::plugin_process::ProcessSupervisor::new(store);
    let mut request = spec("tool.sh", &["sleep", "30"]);
    request.timeout_ms = Some(100);
    let output = supervisor
        .run("probe", &request)
        .await
        .expect("the deadline is reported, not an error");
    assert!(output.timed_out);
    assert_ne!(output.status, 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn spawning_and_waiting_reports_the_same_output() {
    let root = world_root("spawn");
    let store = support::store_with_worlds(root.clone()).await;
    let supervisor = ferris_server::plugin_process::ProcessSupervisor::new(store);
    let id = supervisor
        .spawn("probe", &spec("tool.sh", &["printf", "spawned"]))
        .await
        .expect("spawned");
    let output = supervisor.wait("probe", &id).await.expect("waited");
    assert_eq!(output.stdout, b"spawned");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn an_absolute_program_is_refused() {
    let root = world_root("absolute");
    let store = support::store_with_worlds(root.clone()).await;
    let supervisor = ferris_server::plugin_process::ProcessSupervisor::new(store);
    let error = supervisor
        .run("probe", &spec("/bin/sh", &["-c", "echo nope"]))
        .await
        .expect_err("an absolute path is outside the world");
    assert!(error.to_string().contains("absolute"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}
