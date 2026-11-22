//! Custom-element naming: every plugin component mounts inside the Desk's
//! reserved `ferris-` namespace.

/// `"invoice-actions"` → `"ferris-invoice-actions"`. Custom element names must
/// contain a hyphen, and the `ferris-` prefix is the parts that guarantees
/// every plugin component stays in a reserved namespace.
///
/// Only reachable on wasm (the DOM mount) and in unit tests, so the native
/// check would flag it as dead code. `pub(crate)` because the widget host
/// (Fase 1) and the website page both mount through their own anchors.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn component_tag(component: &str) -> String {
    let cleaned: String = component
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("ferris-{cleaned}")
}
