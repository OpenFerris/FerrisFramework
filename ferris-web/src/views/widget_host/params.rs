//! Instance parameter values, plus the owner tag the host catalog lives under.

use crate::ext::WidgetDef;

/// Owner tag the host publishes its own catalog under (registered by
/// `ferris_core::ui_extensions::register_host_widgets`): its widgets render
/// natively instead of mounting a plugin element.
pub(super) const HOST_OWNER: &str = "host";

/// Value of `name` for this instance: the instance's own value first, else
/// the default the definition declares (the editors write what the author
/// picked; a hand-written dashboard may simply omit it).
pub(super) fn param_or<'a>(
    def: &'a WidgetDef,
    params: &'a [(String, String)],
    name: &str,
) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .or_else(|| {
            def.params
                .iter()
                .find(|param| param.name == name)
                .and_then(|param| param.default.as_deref())
        })
}

/// Numeric value of `name`: the host only transports strings, so a value
/// that is absent or not a number falls back to `fallback`.
pub(super) fn number_param(
    def: &WidgetDef,
    params: &[(String, String)],
    name: &str,
    fallback: u64,
) -> u64 {
    param_or(def, params, name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}
