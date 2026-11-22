//! Build pipeline: wasm32-wasip2 core module or component -> `ferris-app` component.
//!
//! Modern toolchains (wasip2 + wit-bindgen 0.62+) produce a **component directly**
//! from `cargo build --target wasm32-wasip2`. This module provides helpers to
//! detect if a file is already a component and otherwise wrap a core module.

use anyhow::Result;
use std::borrow::Cow;
use std::path::Path;

/// The preamble every component binary starts with: the `\0asm` magic plus the
/// component-model version (`0x0d 0x00`) and layer (`0x01 0x00`). A core module
/// carries version `0x01 0x00 0x00 0x00` instead, so the two never collide.
const COMPONENT_PREAMBLE: &[u8; 8] = b"\0asm\x0d\x00\x01\x00";

/// Check if `bytes` is a component.
///
/// Reads the binary preamble instead of asking wasmtime. `Component::from_binary`
/// does not parse, it runs cranelift over the whole artifact: on a 3.5 MiB app
/// that cost ~840 ms per call, so every hot reload compiled the guest twice —
/// once here on a throwaway engine, once for real in `Plugin::from_bytes`.
#[must_use]
pub fn is_component(bytes: &[u8]) -> bool {
    bytes.starts_with(COMPONENT_PREAMBLE)
}

/// Ensure the given bytes are a valid component.
///
/// - If already a component, borrow them as-is (no copy).
/// - If a core module, wrap it into a component using `wit-component`.
///
/// # Errors
///
/// Returns an error when a core module cannot be wrapped or validated as a
/// component.
pub fn ensure_component(bytes: &[u8]) -> Result<Cow<'_, [u8]>> {
    if is_component(bytes) {
        Ok(Cow::Borrowed(bytes))
    } else {
        // Legacy core module -> component (for older toolchains)
        let component = wit_component::ComponentEncoder::default()
            .validate(true)
            .module(bytes)?
            .encode()?;
        Ok(Cow::Owned(component))
    }
}

/// Load a component from file (auto-detect core vs component).
///
/// # Errors
///
/// Returns an error when the file cannot be read or a core module cannot
/// be wrapped into a component.
pub fn load_component(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = std::fs::read(path)?;
    if is_component(&bytes) {
        // Already a component: hand back the bytes we just read, without the
        // extra full-artifact copy a `Cow` round-trip would cost here.
        return Ok(bytes);
    }
    bytes = wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&bytes)?
        .encode()?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_preamble_is_recognized() {
        assert!(is_component(b"\0asm\x0d\x00\x01\x00rest of the component"));
    }

    #[test]
    fn core_modules_are_not_components() {
        let core_module = wat::parse_str(
            r#"(module (func (export "add") (param i32 i32) (result i32)
                   local.get 0 local.get 1 i32.add))"#,
        )
        .expect("sample core module");
        assert!(!is_component(&core_module));
        assert!(!is_component(b""));
        assert!(!is_component(b"not wasm at all"));
        assert!(!is_component(b"\0asm"));
    }

    /// The header check must agree with what wasmtime actually accepts, since the
    /// whole point is to replace that call.
    #[test]
    fn header_check_matches_what_wasmtime_accepts() {
        let engine = wasmtime::Engine::default();

        let core_module = wat::parse_str(
            r#"(module (func (export "add") (param i32 i32) (result i32)
                   local.get 0 local.get 1 i32.add))"#,
        )
        .expect("sample core module");
        assert!(
            wasmtime::component::Component::from_binary(&engine, &core_module).is_err(),
            "a core module must not validate as a component"
        );
        assert!(!is_component(&core_module));

        let component = wat::parse_str(
            r#"(component
                 (core module
                   (func (export "add") (param i32 i32) (result i32)
                     local.get 0 local.get 1 i32.add)))"#,
        )
        .expect("sample component");
        assert!(
            wasmtime::component::Component::from_binary(&engine, &component).is_ok(),
            "a component must validate as a component"
        );
        assert!(is_component(&component));
    }

    #[test]
    fn ensure_component_borrows_an_existing_component() {
        let component = b"\0asm\x0d\x00\x01\x00payload";
        let wrapped = ensure_component(component).expect("already a component");
        assert!(matches!(wrapped, Cow::Borrowed(_)));
        assert_eq!(wrapped.as_ref(), component);
    }
}
