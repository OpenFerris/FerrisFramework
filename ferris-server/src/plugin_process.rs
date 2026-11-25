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
/// [`read_capped_with`]): truncating would hand the guest a short read it cannot
/// detect.
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// Wall-clock cap for one child when neither the guest nor the operator asks
/// for another one. Thirty seconds is enough for a converter or a build step
/// and short enough that a wedged program does not hold a slot forever.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Operator allow-list of binaries an app may run, comma-separated.
///
/// An entry is a bare name (`convert`) or a path relative to the app world
/// (`bin/convert`). An empty list preserves the older behaviour — the world is
/// the boundary — while a non-empty list is a strict additional filter applied
/// before spawn.
const ALLOWED_BINS_ENV: &str = "FERRIS_APP_ALLOWED_BINS";

/// Optional override for [`DEFAULT_TIMEOUT_MS`], in milliseconds.
const TIMEOUT_ENV: &str = "FERRIS_APP_PROCESS_TIMEOUT_MS";

/// Optional override for the per-stream output cap, in bytes.
const OUTPUT_ENV: &str = "FERRIS_APP_PROCESS_MAX_OUTPUT";

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

/// Limits and the binary allow-list applied to every app process.
///
/// The policy is host configuration, not something a guest can widen: the
/// deadline is a cap (`min` with the guest's `timeout-ms`), the output cap is
/// passed to the reader, and [`ProcessPolicy::check_program`] refuses a program
/// before [`Command::spawn`] when the operator listed the binaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPolicy {
    allowed_bins: Vec<String>,
    timeout_ms: u64,
    max_output: usize,
}

impl Default for ProcessPolicy {
    fn default() -> Self {
        Self {
            allowed_bins: Vec::new(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_output: MAX_OUTPUT,
        }
    }
}

impl ProcessPolicy {
    /// Build a policy from already-parsed values.
    ///
    /// # Errors
    ///
    /// A zero timeout/output cap, a NUL byte, an absolute path or a `..`
    /// component in an allow-list entry.
    pub fn new(allowed_bins: Vec<String>, timeout_ms: u64, max_output: usize) -> Result<Self> {
        if timeout_ms == 0 {
            anyhow::bail!("{TIMEOUT_ENV} must be greater than zero");
        }
        if max_output == 0 {
            anyhow::bail!("{OUTPUT_ENV} must be greater than zero");
        }
        let mut normalized = Vec::with_capacity(allowed_bins.len());
        for bin in allowed_bins {
            let bin = bin.trim().trim_start_matches("./").to_string();
            if bin.is_empty() {
                continue;
            }
            if bin.contains('\0') || bin.contains("..") || Path::new(&bin).is_absolute() {
                anyhow::bail!(
                    "{ALLOWED_BINS_ENV}: `{bin}` must be a name or a relative path inside the app world"
                );
            }
            normalized.push(bin);
        }
        normalized.sort();
        normalized.dedup();
        Ok(Self {
            allowed_bins: normalized,
            timeout_ms,
            max_output,
        })
    }

    /// Read the policy from the environment, falling back to the defaults for
    /// every variable that is absent.
    ///
    /// # Errors
    ///
    /// The parse/validation errors of [`ProcessPolicy::from_values`].
    pub fn from_env() -> Result<Self> {
        let allowed = std::env::var(ALLOWED_BINS_ENV).ok();
        let timeout = std::env::var(TIMEOUT_ENV).ok();
        let output = std::env::var(OUTPUT_ENV).ok();
        Self::from_values(allowed.as_deref(), timeout.as_deref(), output.as_deref())
    }

    /// Testable form of [`ProcessPolicy::from_env`].
    ///
    /// # Errors
    ///
    /// [`ProcessPolicy::new`] plus a non-numeric timeout/output value.
    pub fn from_values(
        allowed_bins: Option<&str>,
        timeout_ms: Option<&str>,
        max_output: Option<&str>,
    ) -> Result<Self> {
        let allowed = allowed_bins.map_or_else(Vec::new, |raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        });
        let timeout = parse_limit(timeout_ms, TIMEOUT_ENV, DEFAULT_TIMEOUT_MS)?;
        let output = parse_limit(
            max_output,
            OUTPUT_ENV,
            u64::try_from(MAX_OUTPUT).unwrap_or(u64::MAX),
        )?;
        let max_output = usize::try_from(output)
            .map_err(|_| anyhow::anyhow!("{OUTPUT_ENV} does not fit this platform"))?;
        Self::new(allowed, timeout, max_output)
    }

    /// The admitted binary names/paths, sorted and normalized.
    #[must_use]
    pub fn allowed_bins(&self) -> &[String] {
        &self.allowed_bins
    }

    /// The host's wall-clock cap.
    #[must_use]
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    /// The per-stream output cap.
    #[must_use]
    pub fn max_output(&self) -> usize {
        self.max_output
    }

    /// Refuse a program the operator did not list. An empty list means the
    /// world boundary alone decides, which is the behaviour before the policy
    /// existed.
    ///
    /// # Errors
    ///
    /// A typed `unsupported` bridge error naming the allow-list variable.
    pub fn check_program(&self, program: &str) -> Result<()> {
        if self.allowed_bins.is_empty() {
            return Ok(());
        }
        let program = program.trim().trim_start_matches("./");
        if self.allowed_bins.iter().any(|bin| bin == program) {
            return Ok(());
        }
        Err(BridgeError::unsupported(format!(
            "platform::process: `{program}` is not in the operator's process allow-list \
             ({ALLOWED_BINS_ENV})"
        )))
    }

    /// The guest's deadline clamped to the host cap. `None`/`0` from the guest
    /// still gets the host deadline, because "no deadline" is how a runaway
    /// process kept a slot for the lifetime of the server.
    #[must_use]
    fn effective_timeout_ms(&self, requested: Option<u32>) -> Option<u32> {
        let cap = u32::try_from(self.timeout_ms).unwrap_or(u32::MAX);
        match requested.filter(|ms| *ms > 0) {
            Some(ms) => Some(ms.min(cap)),
            None => Some(cap).filter(|ms| *ms > 0),
        }
    }
}

fn parse_limit(raw: Option<&str>, variable: &str, default: u64) -> Result<u64> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    raw.parse::<u64>()
        .map_err(|_| anyhow::anyhow!("{variable} must be a positive integer, got `{raw}`"))
}

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
    policy: ProcessPolicy,
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
    /// A supervisor over the loaded apps (their worlds and grants), with the
    /// process policy read from the environment.
    ///
    /// An invalid policy is a loud warning and the safe defaults, never a
    /// silently widened one: the server keeps running, but the deadline and the
    /// output cap are still the conservative values.
    #[must_use]
    pub fn new(plugins: Arc<PluginStore>) -> Self {
        let policy = ProcessPolicy::from_env().unwrap_or_else(|error| {
            tracing::warn!(%error, "invalid app process policy; using the defaults");
            ProcessPolicy::default()
        });
        Self::with_policy(plugins, policy)
    }

    /// A supervisor with an explicit policy (tests and embedders).
    #[must_use]
    pub fn with_policy(plugins: Arc<PluginStore>, policy: ProcessPolicy) -> Self {
        Self {
            core: Arc::new(Core {
                plugins,
                running: Mutex::new(HashMap::new()),
                policy,
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
        self.policy.check_program(&spec.program)?;
        check_process_grant(&world, &spec.program)?;
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
        // Re-check here as well: `execute` is also reached from `spawn` after
        // its own validation, and a child must never be forked for a binary
        // outside the operator's list.
        self.policy.check_program(&spec.program)?;
        check_process_grant(&world, &spec.program)?;
        let cwd = resolve_cwd(&world, spec.cwd.as_deref())?;
        let timeout_ms = self.policy.effective_timeout_ms(spec.timeout_ms);
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
        Box::pin(collect(child, timeout_ms, self.policy.max_output())).await
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
async fn collect(
    mut child: Child,
    timeout_ms: Option<u32>,
    max_output: usize,
) -> Result<ProcessOutput> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let collect = Box::pin(async {
        let (stdout, stderr) = tokio::join!(
            read_capped_with(stdout, max_output),
            read_capped_with(stderr, max_output)
        );
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
/// Read one pipe under the policy's cap.
async fn read_capped_with(
    pipe: Option<impl AsyncRead + Unpin>,
    max_output: usize,
) -> Result<Vec<u8>> {
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
        if captured.len().saturating_add(read) > max_output {
            return Err(BridgeError::unsupported(format!(
                "platform::process: output exceeds the {max_output} byte cap; write to a file in \
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

/// Enforce the operator's approved grant when the installed world carries one.
///
/// `grants.toml` is written by `ferris app install`; the host checks **that**
/// file, not the app's manifest. A world without the file predates trusted
/// installs (or is a test), so the policy and the world boundary remain the
/// only gates, exactly as before.
fn check_process_grant(world: &Path, program: &str) -> Result<()> {
    let path = world.join(ferris_wasm::trust::GRANTS_FILE);
    if !path.is_file() {
        return Ok(());
    }
    let grants = ferris_wasm::trust::GrantSet::load(&path).map_err(|error| {
        BridgeError::unsupported(format!(
            "platform::process: cannot read the approved grants {}: {error}",
            path.display()
        ))
    })?;
    if !grants.allows_capability("process") {
        return Err(BridgeError::unsupported(format!(
            "platform::process: the operator did not approve the `process` capability in {}",
            path.display()
        )));
    }
    if !grants.allows_process_bin(program) {
        return Err(BridgeError::unsupported(format!(
            "platform::process: `{program}` is not an approved process bin in {}",
            path.display()
        )));
    }
    Ok(())
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

    /// The allow-list and the two limits parse together; the defaults keep the
    /// old 8 MiB cap and add the 30 s wall clock.
    #[test]
    fn the_policy_parses_its_allowlist_and_limits() {
        let policy = ProcessPolicy::from_values(
            Some(" bin/tool.sh, convert ,, ./helper "),
            Some("5000"),
            Some("1024"),
        )
        .expect("valid policy");
        assert_eq!(
            policy
                .allowed_bins()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["bin/tool.sh", "convert", "helper"]
        );
        assert_eq!(policy.timeout_ms(), 5_000);
        assert_eq!(policy.max_output(), 1_024);
        assert!(policy.check_program("convert").is_ok());
        assert!(policy.check_program("./bin/tool.sh").is_ok());
        assert!(policy.check_program("sh").is_err());
    }

    #[test]
    fn the_policy_defaults_to_thirty_seconds() {
        let policy = ProcessPolicy::from_values(None, None, None).expect("defaults");
        assert_eq!(policy.timeout_ms(), DEFAULT_TIMEOUT_MS);
        assert_eq!(policy.max_output(), MAX_OUTPUT);
        assert!(policy.allowed_bins().is_empty());
        assert!(
            policy.check_program("anything").is_ok(),
            "an empty allow-list keeps the world as the boundary"
        );
    }

    #[test]
    fn the_policy_rejects_zero_and_escapes() {
        assert!(ProcessPolicy::from_values(None, Some("0"), None).is_err());
        assert!(ProcessPolicy::from_values(None, None, Some("0")).is_err());
        assert!(ProcessPolicy::from_values(None, Some("soon"), None).is_err());
        assert!(ProcessPolicy::from_values(Some("../sh"), None, None).is_err());
        assert!(ProcessPolicy::from_values(Some("/bin/sh"), None, None).is_err());
    }

    /// A guest deadline can be shorter than the host cap, never longer.
    #[test]
    fn the_guest_deadline_cannot_outlive_the_host_cap() {
        let policy = ProcessPolicy::new(Vec::new(), 5_000, MAX_OUTPUT).expect("policy");
        assert_eq!(policy.effective_timeout_ms(None), Some(5_000));
        assert_eq!(policy.effective_timeout_ms(Some(0)), Some(5_000));
        assert_eq!(policy.effective_timeout_ms(Some(100)), Some(100));
        assert_eq!(policy.effective_timeout_ms(Some(60_000)), Some(5_000));
    }

    /// The host reads the operator's `grants.toml`, not the app manifest: a bin
    /// the approval does not name is refused before spawn.
    #[test]
    fn the_operator_grant_is_what_admits_a_bin() {
        let world = std::env::temp_dir().join(format!(
            "ferris-proc-grant-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        std::fs::create_dir_all(&world).expect("world");
        assert!(
            check_process_grant(&world, "bin/convert").is_ok(),
            "no grants.toml keeps the pre-trust behaviour"
        );
        let grants = ferris_wasm::trust::GrantSet::new(
            vec!["process".to_string()],
            Vec::new(),
            vec!["bin/convert".to_string()],
        );
        grants
            .save(&world.join(ferris_wasm::trust::GRANTS_FILE))
            .expect("grants");
        assert!(check_process_grant(&world, "bin/convert").is_ok());
        let error = check_process_grant(&world, "bin/other").expect_err("not approved");
        assert!(error.to_string().contains("not an approved"), "{error}");

        let without = ferris_wasm::trust::GrantSet::new(
            Vec::new(),
            Vec::new(),
            vec!["bin/convert".to_string()],
        );
        without
            .save(&world.join(ferris_wasm::trust::GRANTS_FILE))
            .expect("grants");
        let error = check_process_grant(&world, "bin/convert").expect_err("capability withheld");
        assert!(
            error.to_string().contains("`process` capability"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(&world);
    }

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
        let err = read_capped_with(Some(&data[..]), MAX_OUTPUT)
            .await
            .expect_err("over the cap");
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[tokio::test]
    async fn output_at_the_cap_is_returned_whole() {
        let data = vec![b'x'; MAX_OUTPUT];
        let captured = read_capped_with(Some(&data[..]), MAX_OUTPUT)
            .await
            .expect("at the cap");
        assert_eq!(captured.len(), MAX_OUTPUT);
    }
}
