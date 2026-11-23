//! Per-account language picker.
//!
//! The locale is stored on the `User` row and applied by the server on the next
//! SSR render, so changing it reloads the page. A client-only swap would leave
//! the SSR output and the hydrated tree disagreeing on the first frame, which
//! is exactly the flash this catalogue exists to avoid.

use leptos::ev::Event;
use leptos::prelude::*;

use crate::api::drive;
use crate::components::ui::select_native::SelectNative;
use crate::desk::session;
use crate::i18n::use_i18n;

/// Native label for one locale. Unknown locales fall back to their tag, which
/// is still readable and far better than a blank row.
fn language_label(i18n: &crate::i18n::I18n, code: &str) -> String {
    let key = format!("desk.language.{code}");
    let text = i18n.t(&key);
    if text == key { code.to_string() } else { text }
}

#[cfg(target_arch = "wasm32")]
fn reload_page() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}

#[cfg(not(target_arch = "wasm32"))]
const fn reload_page() {}

/// A select with the locales `GET /api/i18n` advertises.
#[component]
pub fn LanguagePicker() -> impl IntoView {
    let i18n = use_i18n();
    let locale = i18n.locale();
    let supported = {
        let advertised = i18n.supported();
        if advertised.is_empty() {
            vec![locale.clone()]
        } else {
            advertised
        }
    };
    let current = RwSignal::new(locale);
    let error = RwSignal::new(None::<String>);

    let on_change = Callback::new(move |event: Event| {
        let next = event_target_value(&event);
        if next == current.get_untracked() {
            return;
        }
        current.set(next.clone());
        error.set(None);
        drive(async move {
            match session::set_language(&next).await {
                Ok(()) => reload_page(),
                Err(message) => error.set(Some(message)),
            }
        });
    });

    view! {
        <div class="px-2 py-1.5">
            <label
                class="mb-1 block text-xs font-medium text-muted-foreground"
                for="desk-language"
            >
                {i18n.t("desk.language.label")}
            </label>
            <SelectNative
                id="desk-language"
                value=current.read_only()
                aria_label=i18n.t("desk.language.label")
                on_change=on_change
            >
                {supported
                    .into_iter()
                    .map(|code| {
                        let label = language_label(&i18n, &code);
                        view! { <option value=code>{label}</option> }
                    })
                    .collect_view()}
            </SelectNative>
            {move || {
                error.get().map(|message| {
                    view! { <p class="mt-1 text-xs text-destructive">{message}</p> }
                })
            }}
        </div>
    }
}
