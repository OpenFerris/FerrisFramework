//! Selective WASI preview-2 linking.

use wasmtime_wasi::WasiView;

/// Carrier for the `wasi:io` host traits: their state is the resource table
/// hanging off [`WasiView::ctx`].
struct IoTable;

impl wasmtime::component::HasData for IoTable {
    type Data<'a> = &'a mut wasmtime::component::ResourceTable;
}

/// Register a **selective** set of WASI preview-2 interfaces on `linker`:
/// clocks, random, stdio and environment, plus the filesystem **only** when the
/// guest was mounted on its world — never sockets.
///
/// This replaces `wasmtime_wasi::p2::add_to_linker_sync`, which links the
/// whole preview-2 surface (filesystem reads/writes, TCP/UDP, name lookup).
/// The split is grounded in what a `wasm32-wasip2` `std` guest actually
/// imports — verified empirically with `wasm-tools component wit` on the
/// reference component `test/target/wasm32-wasip2/debug/test.wasm`:
///
/// * **Imported unconditionally by rustc's wasip2 std** (present in the
///   reference guest although it never touches env vars or `process-exit`):
///   `wasi:cli/{environment,exit,stdin,stdout,stderr,terminal-*}` and
///   `wasi:io/{error,poll,streams}` + `wasi:clocks/monotonic-clock`. These
///   cannot be dropped without breaking instantiation of every plugin.
/// * **Imported on demand; kept so guest conveniences keep working:**
///   `wasi:clocks/wall-clock` (`std::time::SystemTime`, chrono) and
///   `wasi:random/{random,insecure,insecure-seed}` (`uuid`, `rand`).
/// * **Conditional:** `wasi:filesystem/{preopens,types}` is linked only when
///   `with_filesystem` is true, i.e. when `HostState::mount_world` actually
///   preopened the app's world. A guest without the `world` capability imports
///   `wasi:filesystem/types` as soon as it touches `std::fs`, and instantiation
///   fails with
///   `component imports instance wasi:filesystem/types@0.2.9, but a matching
///   implementation was not found in the linker` — the fail-closed behaviour
///   this module exists for.
/// * **Removed:** `wasi:sockets/*` (the host owns every socket).
///
/// The bodies mirror wasmtime-wasi 49's private `add_nonblocking_to_linker`
/// and `add_sync_wasi_io`, minus filesystem and sockets, so the kept
/// interfaces behave exactly as they did under `add_to_linker_sync`.
pub(super) fn add_selective_wasi<T: WasiView>(
    linker: &mut wasmtime::component::Linker<T>,
    with_filesystem: bool,
) -> wasmtime::Result<()> {
    use wasmtime_wasi::cli::{WasiCli, WasiCliView as _};
    use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView as _};
    use wasmtime_wasi::filesystem::{WasiFilesystem, WasiFilesystemView as _};
    use wasmtime_wasi::p2::bindings::{cli, clocks, filesystem, random};
    use wasmtime_wasi::random::{WasiRandom, WasiRandomView as _};

    let l = linker;

    // wasi:clocks — wall-clock backs `SystemTime`, monotonic backs `Instant`
    // and stream subscriptions.
    clocks::wall_clock::add_to_linker::<T, WasiClocks>(l, T::clocks)?;
    clocks::monotonic_clock::add_to_linker::<T, WasiClocks>(l, T::clocks)?;

    // wasi:random — `uuid`/`rand` reach the guest through getrandom.
    random::random::add_to_linker::<T, WasiRandom>(l, T::random)?;
    random::insecure::add_to_linker::<T, WasiRandom>(l, T::random)?;
    random::insecure_seed::add_to_linker::<T, WasiRandom>(l, T::random)?;

    // wasi:cli — stdio plus the `environment`/`exit`/terminal set that the std
    // imports unconditionally (see the doc comment).
    cli::exit::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::environment::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::stdin::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::stdout::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::stderr::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::terminal_input::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::terminal_output::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::terminal_stdin::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::terminal_stdout::add_to_linker::<T, WasiCli>(l, T::cli)?;
    cli::terminal_stderr::add_to_linker::<T, WasiCli>(l, T::cli)?;

    // wasi:filesystem — only with a mounted world. `preopens` answers the
    // guest's directory list and `types` resolves every open through it, so the
    // preopen is the whole boundary; `sync` mirrors the `io` flavour below.
    if with_filesystem {
        filesystem::preopens::add_to_linker::<T, WasiFilesystem>(l, T::filesystem)?;
        wasmtime_wasi::p2::bindings::sync::filesystem::types::add_to_linker::<T, WasiFilesystem>(
            l,
            T::filesystem,
        )?;
    }

    // wasi:io — sync flavour, mirroring `add_sync_wasi_io`. `error` comes from
    // `wasmtime-wasi-io`, where the generated bindings' `with: { "wasi:io/error"
    // }` mapping points (poll/streams are re-exported by `bindings::sync::io`).
    wasmtime_wasi_io::bindings::wasi::io::error::add_to_linker::<T, IoTable>(l, |t| t.ctx().table)?;
    wasmtime_wasi::p2::bindings::sync::io::poll::add_to_linker::<T, IoTable>(l, |t| t.ctx().table)?;
    wasmtime_wasi::p2::bindings::sync::io::streams::add_to_linker::<T, IoTable>(l, |t| {
        t.ctx().table
    })?;

    Ok(())
}
