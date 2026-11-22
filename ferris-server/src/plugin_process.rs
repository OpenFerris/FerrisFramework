//! The host side of `platform::process-*`.
//!
//! An app can run a program, but it does not fork: the server owns the child,
//! the pipes, the deadline and the reaping. That is what makes a runaway
//! process attributable to one app and killable with it, instead of leaking
//! into the host's process group.
//!
//! # Containment
//!
//! * `cwd` is resolved inside `plugins/<app>/` (the guest's `/world`) and may
//!   not escape it. A `program` carrying a path is resolved inside the world
//!   too.
//! * A **bare** `program` name is resolved inside the world first and, only
//!   when the operator lists it in `FERRIS_APP_ALLOWED_PROGRAMS`, looked up in
//!   the system tool directories. There is no implicit `PATH` search: before
//!   that list existed, every installed app could exec `sh`, `python` or any
//!   other interpreter on the host by naming it. The allow-list is the
//!   operator's explicit decision, and it is empty by default.
//! * There is no shell: `program` + `args` go to `execve` verbatim, so a value
//!   that looks like a command substitution is just an argument.
//! * The environment starts empty. The host adds only a fixed baseline
//!   (`PATH`, `HOME`, `TMPDIR`, `LANG`, the app name) plus the keys the app
//!   declared; the host's own environment — credentials included — is never
//!   inherited.
//! * At most [`MAX_RUNNING_PER_APP`] children run per app at once; the counter
//!   is shared by every call into the supervisor, so `spawn` cannot sidestep
//!   it. Dropping the supervisor aborts every child it still tracks, which is
//!   what ties the processes to the app's lifecycle.
//! * Output is read concurrently and capped; a stream that crosses
//!   [`MAX_OUTPUT`] is a typed failure, never a silently truncated buffer. A
//!   program that writes forever is killed by the deadline.
//!
//! A missing world, an unsafe path, an unresolvable program, an exhausted
//! process quota or a refused spawn are typed failures. A process that exits
//! non-zero is a **successful** call: its status is the data the guest asked
//! for.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use ferris_wasm::host::BridgeError;
use ferris_wasm::{PluginStore, ProcessOutput, ProcessService, ProcessSpec};
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

/// Largest stdout/stderr captured per stream.
///
/// The cap protects the host; it is not a per-app setting because an app that
/// genuinely needs more should write to a file in its world instead of piping
/// gigabytes through a guest call. Crossing it fails the call (see
/// [`read_capped`]): truncating would hand the guest a short read it cannot
/// detect.
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// The only `PATH` a child sees: the system tools, never the host's shell
/// profile or a deployment-specific directory.
///
/// The same list is searched for an allow-listed bare program name; it is not a
/// search path for every app, only for the names the operator admitted.
const BASELINE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Operator allow-list of bare program names an app may run outside its world.
///
/// Comma-separated and empty by default, because the safe answer is "only what
/// the app ships". Listing `sh` here is a decision the operator makes on
/// purpose, with the consequences that follow.
const ALLOWED_PROGRAMS_ENV: &str = "FERRIS_APP_ALLOWED_PROGRAMS";

/// Children one app may have running at once.
///
/// A quota, not a cap on the OS: an app that leaks processes used to be able to
/// fork until the host ran out of descriptors. Eight is enough for a build or a
/// batch of converters and small enough that one app cannot starve the rest.
const MAX_RUNNING_PER_APP: usize = 8;

/// One spawned process the app may still wait for or kill.
struct Running {
    app: String,
    result: oneshot::Receiver<ProcessOutput>,
    task: AbortHandle,
}

/// The process-wide half of the supervisor: the store and the per-app quota.
///
/// It is separate from [`ProcessSupervisor`] so a task started by `spawn` keeps
/// charging the same counter after the call that created it returned; the child
/// table is per supervisor and goes away with it.
struct Core {
    plugins: Arc<PluginStore>,
    running: Mutex<HashMap<String, usize>>,
}

/// The process-wide supervisor behind `platform::process-*`.
pub struct ProcessSupervisor {
    core: Arc<Core>,
    children: Mutex<HashMap<String, Running>>,
}

impl std::fmt::Debug for ProcessSupervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let children = self.children.lock().len();
        let quota = self.core.running.lock().clone();
        f.debug_struct("ProcessSupervisor")
            .field("running", &children)
            .field("quota", &quota)
            .finish_non_exhaustive()
    }
}

impl ProcessSupervisor {
    /// A supervisor over the loaded apps (their worlds and grants).
    #[must_use]
    pub fn new(plugins: Arc<PluginStore>) -> Self {
        Self {
            core: Arc::new(Core {
                plugins,
                running: Mutex::new(HashMap::new()),
            }),
            children: Mutex::new(HashMap::new()),
        }
    }
}

impl Drop for ProcessSupervisor {
    fn drop(&mut self) {
        // The tasks own the children (`kill_on_drop`); aborting them is what
        // makes "the app is gone" also mean "its processes are gone".
        let children = std::mem::take(&mut *self.children.lock());
        for (_, running) in children {
            running.task.abort();
        }
    }
}

/// One child's admission ticket into the per-app quota.
///
/// The ticket releases its slot when dropped, whether the child finished, the
/// deadline fired or the task was aborted.
struct Slot {
    app: String,
    core: Arc<Core>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        release(&mut self.core.running.lock(), &self.app);
    }
}

/// Charge one running child to `app`, or refuse when it already has the quota.
fn charge(counts: &mut HashMap<String, usize>, app: &str) -> Result<()> {
    let count = counts.entry(app.to_string()).or_insert(0);
    if *count >= MAX_RUNNING_PER_APP {
        return Err(BridgeError::unsupported(format!(
            "platform::process: app `{app}` already has {MAX_RUNNING_PER_APP} processes running; \
             wait for one to finish"
        )));
    }
    *count += 1;
    Ok(())
}

/// Give a slot back, dropping the app's entry once it reaches zero.
fn release(counts: &mut HashMap<String, usize>, app: &str) {
    let empty = counts.get_mut(app).is_some_and(|count| {
        *count = count.saturating_sub(1);
        *count == 0
    });
    if empty {
        counts.remove(app);
    }
}

impl Core {
    fn world(&self, app: &str) -> Result<PathBuf> {
        self.plugins.world_of(app).ok_or_else(|| {
            BridgeError::unsupported(format!(
                "platform::process-*: app `{app}` has no world; a process runs inside \
                 `plugins/{app}/`"
            ))
        })
    }

    /// Check everything that can be checked before an id is handed out, so a
    /// refused spawn never leaves a row in the child table.
    fn validate(&self, app: &str, spec: &ProcessSpec) -> Result<()> {
        let world = self.world(app)?;
        resolve_program(&world, &spec.program)?;
        resolve_cwd(&world, spec.cwd.as_deref())?;
        Ok(())
    }

    fn admit(self: &Arc<Self>, app: &str) -> Result<Slot> {
        charge(&mut self.running.lock(), app)?;
        Ok(Slot {
            app: app.to_string(),
            core: Arc::clone(self),
        })
    }

    /// Build and run one program to completion.
    async fn execute(self: &Arc<Self>, app: &str, spec: &ProcessSpec) -> Result<ProcessOutput> {
        let _slot = self.admit(app)?;
        let world = self.world(app)?;
        let program = resolve_program(&world, &spec.program)?;
        let cwd = resolve_cwd(&world, spec.cwd.as_deref())?;
        let mut command = Command::new(&program);
        command
            .args(&spec.args)
            .current_dir(&cwd)
            .env_clear()
            .env("PATH", BASELINE_PATH)
            .env("HOME", &world)
            .env("TMPDIR", world.join("data"))
            .env("FERRIS_APP", app)
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // The child is killed when its future is dropped (deadline, kill,
            // or a shutdown that aborts the task).
            .kill_on_drop(true);
        for (key, value) in &spec.env {
            if key.is_empty() || key.contains('\0') || key.contains('=') || value.contains('\0') {
                return Err(BridgeError::unsupported(format!(
                    "platform::process: invalid environment entry `{key}`"
                )));
            }
            command.env(key, value);
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "platform::process: spawning `{}` in {}",
                program.display(),
                cwd.display()
            )
        })?;
        if let Some(input) = &spec.stdin
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin
                .write_all(input)
                .await
                .context("platform::process: writing stdin")?;
            // Dropping closes the pipe, which is what an EOF-driven program
            // waits for.
            drop(stdin);
        }
        Box::pin(collect(child, spec.timeout_ms)).await
    }
}

#[async_trait]
impl ProcessService for ProcessSupervisor {
    async fn run(&self, app: &str, spec: &ProcessSpec) -> Result<ProcessOutput> {
        Box::pin(Arc::clone(&self.core).execute(app, spec)).await
    }

    async fn spawn(&self, app: &str, spec: &ProcessSpec) -> Result<String> {
        // Validate before creating the id, so a bad spec never leaves a row.
        self.core.validate(app, spec)?;
        let id = format!("proc-{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        let core = Arc::clone(&self.core);
        let app_for_task = app.to_string();
        let spec = spec.clone();
        let task = tokio::spawn(async move {
            let output = core
                .execute(&app_for_task, &spec)
                .await
                .unwrap_or_else(|error| ProcessOutput {
                    // `-1` is the conventional "did not produce a status"; the
                    // error itself is not ferried through the WIT record, so the
                    // guest sees an abnormal exit instead of a silent success.
                    status: -1,
                    stdout: Vec::new(),
                    stderr: error.to_string().into_bytes(),
                    timed_out: false,
                });
            let _ = tx.send(output);
        });
        self.children.lock().insert(
            id.clone(),
            Running {
                app: app.to_string(),
                result: rx,
                task: task.abort_handle(),
            },
        );
        Ok(id)
    }

    async fn wait(&self, app: &str, process_id: &str) -> Result<ProcessOutput> {
        let running = {
            let mut children = self.children.lock();
            match children.get(process_id) {
                Some(running) if running.app != app => {
                    anyhow::bail!("`{process_id}` does not belong to app `{app}`")
                }
                Some(_) => children.remove(process_id),
                None => anyhow::bail!("unknown or already waited process `{process_id}`"),
            }
        };
        let Some(running) = running else {
            anyhow::bail!("unknown or already waited process `{process_id}`");
        };
        running
            .result
            .await
            .map_err(|_| anyhow::anyhow!("process `{process_id}` ended without an output"))
    }

    async fn kill(&self, app: &str, process_id: &str) -> Result<()> {
        let running = {
            let mut children = self.children.lock();
            match children.get(process_id) {
                Some(running) if running.app != app => {
                    anyhow::bail!("`{process_id}` does not belong to app `{app}`")
                }
                Some(_) => children.remove(process_id),
                None => return Ok(()),
            }
        };
        if let Some(running) = running {
            running.task.abort();
        }
        Ok(())
    }
}

/// Run one child to completion under an optional deadline.
// One arm awaits and the other returns, so the suggested `map_or_else` cannot
// express it without an async block in each closure.
#[allow(clippy::option_if_let_else)]
async fn collect(mut child: Child, timeout_ms: Option<u32>) -> Result<ProcessOutput> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let collect = Box::pin(async {
        let (stdout, stderr) = tokio::join!(read_capped(stdout), read_capped(stderr));
        let status = child.wait().await.context("platform::process: waiting")?;
        Ok::<ProcessOutput, anyhow::Error>(ProcessOutput {
            status: status.code().unwrap_or(-1),
            stdout: stdout?,
            stderr: stderr?,
            timed_out: false,
        })
    });
    let Some(ms) = timeout_ms.filter(|ms| *ms > 0) else {
        return collect.await;
    };
    match tokio::time::timeout(Duration::from_millis(u64::from(ms)), collect).await {
        Ok(output) => output,
        Err(_) => Ok(ProcessOutput {
            status: -1,
            stdout: Vec::new(),
            stderr: format!("killed after the {ms} ms deadline").into_bytes(),
            timed_out: true,
        }),
    }
}

/// Read one pipe, refusing the whole call once it would cross [`MAX_OUTPUT`].
///
/// The previous shape kept the first 8 MiB and dropped the rest: the guest got
/// a JSON or a report that looked complete and was not, with nothing in the
/// record saying so. A typed failure is the only honest answer — and dropping
/// the pipe also sends the writer an `EPIPE`, so the program stops instead of
/// writing into the void.
async fn read_capped(pipe: Option<impl AsyncRead + Unpin>) -> Result<Vec<u8>> {
    let Some(mut pipe) = pipe else {
        return Ok(Vec::new());
    };
    let mut captured = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = pipe
            .read(&mut buffer)
            .await
            .context("platform::process: reading output")?;
        if read == 0 {
            break;
        }
        if captured.len().saturating_add(read) > MAX_OUTPUT {
            return Err(BridgeError::unsupported(format!(
                "platform::process: output exceeds the {MAX_OUTPUT} byte cap; write to a file in \
                 the app world instead of piping it through the call"
            )));
        }
        captured.extend_from_slice(&buffer[..read]);
    }
    Ok(captured)
}

/// The operator allow-list, read from the environment on every resolution so a
/// restart is the only thing a change needs (and a test can drive it directly
/// through [`resolve_program_in`]).
fn allowed_programs() -> Vec<String> {
    std::env::var(ALLOWED_PROGRAMS_ENV).map_or_else(
        |_| Vec::new(),
        |raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        },
    )
}

/// Resolve `program` without a shell, against the operator allow-list.
fn resolve_program(world: &Path, program: &str) -> Result<PathBuf> {
    resolve_program_in(world, program, &allowed_programs())
}

/// Resolve `program` without a shell: the world first, then — only for a name
/// the operator listed — the baseline tool directories.
fn resolve_program_in(world: &Path, program: &str, allowed: &[String]) -> Result<PathBuf> {
    let program = program.trim();
    if program.is_empty() || program.contains('\0') {
        return Err(BridgeError::unsupported(
            "platform::process: `program` is empty or contains a NUL byte",
        ));
    }
    let candidate = Path::new(program);
    if candidate.is_absolute() {
        return Err(BridgeError::unsupported(format!(
            "platform::process: `{program}` is absolute; a program must be a name or a path \
             inside the app world"
        )));
    }
    if program.contains('/') || program.contains('\\') {
        let inside = ferris_core::plugins::world_path(world, candidate).map_err(|error| {
            BridgeError::unsupported(format!(
                "platform::process: `{program}` is not inside the app world: {error}"
            ))
        })?;
        if !inside.is_file() {
            return Err(BridgeError::unsupported(format!(
                "platform::process: `{}` does not exist in the app world",
                inside.display()
            )));
        }
        return Ok(inside);
    }
    let in_world = world.join(program);
    if in_world.is_file() {
        return Ok(in_world);
    }
    if allowed.iter().any(|name| name == program) {
        for dir in BASELINE_PATH.split(':') {
            let candidate = Path::new(dir).join(program);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(BridgeError::unsupported(format!(
        "platform::process: `{program}` is not in the app world, and a bare name runs only when \
         the operator lists it in {ALLOWED_PROGRAMS_ENV}"
    )))
}

/// Resolve `cwd` inside the world, refusing anything that escapes it.
fn resolve_cwd(world: &Path, cwd: Option<&str>) -> Result<PathBuf> {
    let Some(cwd) = cwd.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(world.to_path_buf());
    };
    let inside = ferris_core::plugins::world_path(world, Path::new(cwd)).map_err(|error| {
        BridgeError::unsupported(format!(
            "platform::process: `{cwd}` is not inside the app world: {error}"
        ))
    })?;
    if !inside.is_dir() {
        return Err(BridgeError::unsupported(format!(
            "platform::process: the working directory `{}` does not exist",
            inside.display()
        )));
    }
    Ok(inside)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absolute_program_is_refused() {
        let world = std::env::temp_dir();
        assert!(resolve_program_in(&world, "/bin/sh", &[]).is_err());
    }

    #[test]
    fn a_relative_program_is_resolved_inside_the_world() {
        let world = std::env::temp_dir().join(format!("ferris-proc-{}", std::process::id()));
        std::fs::create_dir_all(&world).expect("world");
        let script = world.join("tool.sh");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").expect("script");
        let resolved = resolve_program_in(&world, "tool.sh", &[]).expect("program");
        assert_eq!(resolved, script);
        assert!(resolve_program_in(&world, "../escape", &[]).is_err());
        let _ = std::fs::remove_dir_all(&world);
    }

    /// WASM-004: a bare name is not looked up in the host `PATH` unless the
    /// operator admitted it, so `sh` is not one `program` field away.
    #[test]
    fn a_bare_program_needs_the_operator_allowlist() {
        let world = std::env::temp_dir().join(format!("ferris-proc-bare-{}", std::process::id()));
        std::fs::create_dir_all(&world).expect("world");
        let err = resolve_program_in(&world, "sh", &[]).expect_err("not admitted");
        assert!(err.to_string().contains(ALLOWED_PROGRAMS_ENV), "{err}");
        assert!(
            resolve_program_in(&world, "sh", &["sh".to_string()]).is_ok(),
            "the operator's explicit listing is what admits a system tool"
        );
        let _ = std::fs::remove_dir_all(&world);
    }

    #[test]
    fn cwd_may_not_escape_the_world() {
        let world = std::env::temp_dir();
        assert!(resolve_cwd(&world, Some("..")).is_err());
        assert!(resolve_cwd(&world, Some("/")).is_err());
        assert_eq!(resolve_cwd(&world, None).expect("root"), world);
    }

    /// The per-app quota counts down as well as up, or an app would run out of
    /// slots after eight processes for the rest of its life.
    #[test]
    fn the_per_app_quota_refuses_the_child_past_the_limit() {
        let mut counts = HashMap::new();
        for _ in 0..MAX_RUNNING_PER_APP {
            charge(&mut counts, "app").expect("under the quota");
        }
        let err = charge(&mut counts, "app").expect_err("over the quota");
        assert!(err.to_string().contains("already has"), "{err}");
        assert!(charge(&mut counts, "other").is_ok(), "the quota is per app");
        release(&mut counts, "app");
        assert!(
            charge(&mut counts, "app").is_ok(),
            "a finished child gives its slot back"
        );
        for _ in 0..MAX_RUNNING_PER_APP {
            release(&mut counts, "app");
        }
        assert!(!counts.contains_key("app"), "an idle app leaves no entry");
    }

    /// WASM-010: past the cap the call fails; it does not return a prefix that
    /// looks like the whole output.
    #[tokio::test]
    async fn output_past_the_cap_is_an_error_not_a_truncation() {
        let data = vec![b'x'; MAX_OUTPUT + 1];
        let err = read_capped(Some(&data[..]))
            .await
            .expect_err("over the cap");
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[tokio::test]
    async fn output_at_the_cap_is_returned_whole() {
        let data = vec![b'x'; MAX_OUTPUT];
        let captured = read_capped(Some(&data[..])).await.expect("at the cap");
        assert_eq!(captured.len(), MAX_OUTPUT);
    }
}
