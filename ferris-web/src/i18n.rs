//! Locale context for the Desk (SSR and hydration).
//!
//! The server negotiates `Accept-Language` once per render and provides a
//! [`DeskI18n`]; the shell serializes the same value into a `<meta>` tag so the
//! hydrated app starts with the identical catalogue and locale. There is no
//! second fetch and no flash of English before the client asks for messages.
//!
//! The data type lives in `ferris_web_shared` so `ferris-web-ui` can read the
//! same context; the Leptos helpers stay here because this crate is the one
//! that owns the shell and the hydration bootstrap.

pub use ferris_web_shared::i18n::{DeskI18n, Translate};
use leptos::prelude::*;

/// The `<meta>` the shell stamps with the effective catalogue.
pub const I18N_META: &str = "ferris-i18n";

/// A `Copy` handle to the catalogue of the current component tree.
///
/// `DeskI18n` owns a map and therefore cannot be `Copy`; a view with several
/// closures would move it into the first one. The handle is an arena id, so it
/// can be captured by every closure and still answer `t`/`t_with` by borrowing
/// the catalogue. `with_value` keeps the older `StoredValue` call sites
/// (`i18n.with_value(|i| i.t(..))`) compiling unchanged.
#[derive(Clone, Copy)]
pub struct I18n {
    inner: leptos::prelude::StoredValue<DeskI18n>,
}

impl Translate for I18n {
    fn t(&self, key: &str) -> String {
        self.inner.with_value(|i18n| i18n.t(key))
    }

    fn t_with(&self, key: &str, args: &[(&str, &str)]) -> String {
        self.inner.with_value(|i18n| i18n.t_with(key, args))
    }
}

impl I18n {
    /// Translate one key.
    #[must_use]
    pub fn t(&self, key: &str) -> String {
        self.inner.with_value(|i18n| i18n.t(key))
    }

    /// Translate and interpolate `{name}` placeholders.
    #[must_use]
    pub fn t_with(&self, key: &str, args: &[(&str, &str)]) -> String {
        self.inner.with_value(|i18n| i18n.t_with(key, args))
    }

    /// Borrow the catalogue for a closure.
    pub fn with_value<R>(&self, f: impl FnOnce(&DeskI18n) -> R) -> R {
        self.inner.with_value(f)
    }

    /// Like [`Self::with_value`], but `None` when the arena no longer owns the
    /// entry (a component torn down while an async task still holds the handle).
    pub fn try_with_value<R>(&self, f: impl FnOnce(&DeskI18n) -> R) -> Option<R> {
        self.inner.try_with_value(f)
    }

    /// An owned snapshot, for functions that take `&DeskI18n`.
    #[must_use]
    pub fn value(&self) -> DeskI18n {
        self.inner.with_value(Clone::clone)
    }

    /// The current locale tag.
    #[must_use]
    pub fn locale(&self) -> String {
        self.inner.with_value(|i18n| i18n.locale.clone())
    }

    /// Locales the deployment advertises.
    #[must_use]
    pub fn supported(&self) -> Vec<String> {
        self.inner.with_value(|i18n| i18n.supported.clone())
    }
}

/// The context handle for the current component tree.
///
/// Call it once in a component body and capture the `Copy` handle in every
/// event handler or `move` closure.
#[must_use]
pub fn use_i18n() -> I18n {
    I18n {
        inner: leptos::prelude::StoredValue::new(
            leptos::context::use_context::<DeskI18n>().unwrap_or_default(),
        ),
    }
}

/// The catalogue value, for tests and for the few helpers that still take
/// `&DeskI18n`.
#[must_use]
pub fn use_i18n_value() -> DeskI18n {
    leptos::context::use_context::<DeskI18n>().unwrap_or_default()
}

/// The Desk context, read back from the bootstrap `<meta>` on the client.
///
/// SSR provides the same type through `ferris_server`'s
/// `provide_desk_context`; this path exists so hydration does not depend on a
/// second request.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub fn from_meta() -> Option<DeskI18n> {
    let document = web_sys::window()?.document()?;
    let selector = format!("meta[name=\"{I18N_META}\"]");
    let element = document.query_selector(&selector).ok()??;
    let content = element.get_attribute("content")?;
    serde_json::from_str(&content).ok()
}

/// The server and tests have no document to read; the shell falls back to the
/// context or the default catalogue.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub const fn from_meta() -> Option<DeskI18n> {
    None
}

/// Translate a key using the nearest [`DeskI18n`] context.
///
/// Must be called from a component or a handler that captured [`use_i18n`]; a
/// missing provider yields the key, which is visible instead of blank.
#[must_use]
pub fn t(key: &str) -> String {
    leptos::context::use_context::<DeskI18n>().map_or_else(|| key.to_string(), |i18n| i18n.t(key))
}

#[cfg(test)]
mod tests {
    use super::DeskI18n;

    #[test]
    fn the_catalog_flattens_to_the_effective_messages() {
        let catalog = serde_json::json!({
            "login.title": "Ferris Desk",
            "login.submit": "Entrar",
            "not-a-string": 3
        });
        let i18n = DeskI18n::from_catalog("es", &catalog);
        assert_eq!(i18n.locale, "es");
        assert_eq!(i18n.t("login.title"), "Ferris Desk");
        assert_eq!(i18n.t("missing.key"), "missing.key");
        assert!(!i18n.messages.contains_key("not-a-string"));
    }

    #[test]
    fn an_empty_catalog_still_returns_the_key() {
        let i18n = DeskI18n::default();
        assert_eq!(i18n.t("login.title"), "login.title");
    }
}
