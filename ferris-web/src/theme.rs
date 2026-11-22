//! Desk theming on the client (P-THEMES).
//!
//! Themes are just data: an id, a label and the CSS custom properties to apply
//! on `:root`. The server offers `GET /api/themes` (built-ins `light`/`dark`
//! plus anything an app registered server-side); the client starts from the
//! same built-ins as a fallback so the desk always has at least two themes even
//! when the API is unreachable.
//!
//! The active theme is persisted in `localStorage` and re-applied on every
//! load. Applying a theme sets `data-theme` and `color-scheme` on `<html>` and
//! injects a single `<style id="ferris-theme">` that overrides the palette
//! variables on `:root`.

use leptos::prelude::*;
use std::sync::{Mutex, OnceLock};

use crate::components::hooks::theme_dom;

// The palette and the policy live in the wire contract / shared leaf, so the
// UI components that switch the theme read exactly the same table the boot
// path paints. Re-exported so the Desk keeps calling
// `ferris_web::theme::builtin_themes()`.
pub use ferris_web_shared::theme::{DEFAULT_THEME, THEME_STORAGE_KEY, ThemeSpec, builtin_themes};

// `DEFAULT_THEME` is documented where it is defined (the shared leaf); the
// boot path and the toggle read that one value.

/// The catalogue is install-wide, not per request: a page can ask for it on
/// boot and again after sign-in, and the answer does not change. Keep it for the
/// lifetime of the document so only the first caller hits `/api/themes`.
static THEME_CACHE: OnceLock<Mutex<Option<Vec<ThemeSpec>>>> = OnceLock::new();

/// Fetch the themes offered by the server; falls back to the built-ins when the
/// request fails (guests, SSR, maintenance).
pub async fn themes_from_server() -> Vec<ThemeSpec> {
    if let Some(cached) = THEME_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
    {
        return cached.clone();
    }

    match crate::api::client::api_json("GET", "/api/themes", None).await {
        Ok(value) => match serde_json::from_value::<Vec<ThemeSpec>>(value) {
            Ok(themes) => {
                *THEME_CACHE
                    .get_or_init(|| Mutex::new(None))
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(themes.clone());
                themes
            }
            Err(_) => builtin_themes(),
        },
        Err(_) => builtin_themes(),
    }
}

/// Shared desk theme state: the list of themes and the active id.
#[derive(Clone)]
pub struct ThemeCtx {
    pub themes: RwSignal<Vec<ThemeSpec>>,
    pub current: RwSignal<String>,
}

impl ThemeCtx {
    pub fn default_ctx() -> Self {
        let ctx = Self {
            themes: RwSignal::new(builtin_themes()),
            current: RwSignal::new(DEFAULT_THEME.to_string()),
        };

        // The switch components live in `ferris-web-ui`, which cannot depend on
        // this crate. Publishing the engine as a controller is what keeps the
        // toggle on the real `ferris.theme` path instead of a private flag.
        let controller_is_dark = ctx.clone();
        let controller_set = ctx.clone();
        provide_context(crate::components::hooks::use_theme_mode::ThemeController {
            is_dark: Signal::derive(move || controller_is_dark.is_dark()),
            set_dark: Callback::new(move |dark: bool| controller_set.set_dark(dark)),
        });

        ctx
    }

    /// Whether the active theme is dark, from `ThemeSpec::dark`.
    ///
    /// An id outside the catalogue (a removed app theme) keeps the historical
    /// `dark` id as the fallback so the toggle can still leave it.
    pub fn is_dark(&self) -> bool {
        let id = self.current.get_untracked();
        ferris_web_shared::theme::is_dark_theme(&self.themes.get_untracked(), &id)
    }

    /// Switch to the first theme with the requested brightness.
    ///
    /// A catalogue with no theme on that side (only `light` installed) makes
    /// this a no-op: selecting a nonexistent id would leave the flag and the
    /// painted palette disagreeing.
    pub fn set_dark(&self, dark: bool) {
        let target = self
            .themes
            .get_untracked()
            .into_iter()
            .find(|theme| theme.dark == dark)
            .map(|theme| theme.id);
        if let Some(id) = target {
            self.select(&id);
        }
    }

    /// The active spec, if it exists in the list.
    pub fn active(&self) -> Option<ThemeSpec> {
        let id = self.current.get_untracked();
        self.themes.get_untracked().into_iter().find(|t| t.id == id)
    }

    /// Apply a theme by id: update state, persist, and re-paint the document.
    pub fn select(&self, id: &str) {
        let Some(spec) = self.themes.get_untracked().into_iter().find(|t| t.id == id) else {
            return;
        };
        self.current.set(spec.id.clone());
        persist_theme(&spec.id);
        #[cfg(target_arch = "wasm32")]
        apply_theme(&spec);
    }

    /// Initialise on the client: restore the persisted choice immediately from
    /// the built-in palette, then reconcile with the server's theme list.
    ///
    /// The immediate step is what makes the saved theme survive a reload (and a
    /// signed-out visit): it does not wait on `/api/themes`, which needs a
    /// session and can be slow or unreachable. `localStorage` alone is enough to
    /// paint the desk, so a guest never flashes the default palette.
    pub fn init(&self) {
        self.restore();
        self.refresh();
    }

    /// Restore and paint the saved palette without asking the server.
    ///
    /// Used on the guest `/login` page: `/api/themes` is session-guarded, and a
    /// 401 there only produced console noise. The built-ins and the persisted
    /// id are enough to render the sign-in screen; [`Self::refresh`] runs after
    /// sign-in, when the request can succeed.
    pub fn init_local(&self) {
        self.restore();
    }

    /// Paint the persisted theme using the currently known catalogue.
    fn restore(&self) {
        let restored = stored_theme().unwrap_or_else(|| DEFAULT_THEME.to_string());
        self.current.set(restored.clone());
        #[cfg(target_arch = "wasm32")]
        if let Some(spec) = self.active() {
            apply_theme(&spec);
        }
    }

    /// Fetch the server's themes and re-apply the saved (or default) one.
    ///
    /// Called on an authenticated boot and again after sign-in, when
    /// `/api/themes` can succeed. A saved id may only be described by a palette
    /// the apps registered server-side.
    pub fn refresh(&self) {
        let ctx = self.clone();
        crate::api::drive(async move {
            let themes = themes_from_server().await;
            ctx.themes.set(themes);
            let stored = stored_theme();
            let id = stored
                .filter(|id| {
                    ctx.themes
                        .get_untracked()
                        .iter()
                        .any(|theme| theme.id == *id)
                })
                .unwrap_or_else(|| DEFAULT_THEME.to_string());
            ctx.current.set(id.clone());
            #[cfg(target_arch = "wasm32")]
            if let Some(spec) = ctx.themes.get_untracked().into_iter().find(|t| t.id == id) {
                apply_theme(&spec);
            }
        });
    }
}

/// Read the persisted theme id (if any).
pub fn stored_theme() -> Option<String> {
    theme_dom::stored_theme_id()
}

/// Persist the active theme id.
pub fn persist_theme(id: &str) {
    theme_dom::persist_theme_id(id);
}

/// Paint the active theme on `<html>`.
///
/// The DOM write is `ferris-web-ui`'s [`theme_dom::apply_theme`], shared with the
/// settings toggle so there is one implementation of `data-theme`,
/// `color-scheme` and the injected palette `<style>`.
#[cfg(target_arch = "wasm32")]
pub fn apply_theme(spec: &ThemeSpec) {
    theme_dom::apply_theme(spec);
}

#[cfg(test)]
mod tests {
    use super::ThemeCtx;
    use leptos::prelude::*;

    #[test]
    fn toggling_the_engine_changes_the_active_palette() {
        // `default_ctx` publishes signals and a context, both of which need an
        // arena outside a component.
        let owner = Owner::new();
        owner.set();

        let ctx = ThemeCtx::default_ctx();
        assert!(!ctx.is_dark());

        ctx.set_dark(true);
        assert!(ctx.is_dark());
        assert_eq!(ctx.current.get_untracked(), "dark");

        ctx.set_dark(false);
        assert!(!ctx.is_dark());
        assert_eq!(ctx.current.get_untracked(), "light");
    }
}
