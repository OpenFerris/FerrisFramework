//! Client-side hydration entrypoint (binary target `ferris-web-hydrate`).
//!
//! `ferris dev --hot-reload frontend` builds the front with `--lib` and
//! consumes the `hydrate` export declared in [`ferris_web::hydrate`]. This
//! binary exists so a standalone `--bin ferris-web-hydrate` build stays a valid
//! hydration entry. Its `main` is exported as the wasm start, and the inline
//! bootstrap also calls `hydrate()`, so the lib entry is idempotent.

fn main() {
    #[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
    ferris_web::hydrate::hydrate();
}
