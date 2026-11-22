//! The site announcement strip above the page content.
//!
//! Split out of the shell to keep `app/mod.rs` under the repository's file-size
//! limit. It owns no state: the site settings are read from the same context the
//! rest of the chrome uses, and the two chrome flags decide whether the strip is
//! drawn at all.

use leptos::prelude::*;

use crate::app::use_site_state;

/// `Navbar Settings.announcement`, with its configured help link.
///
/// It starts empty on the SSR pass and on the first client render (the settings
/// arrive later), so hydration is deterministic. A bare page and a full-screen
/// board suppress it: in both cases the page owns the viewport.
#[component]
pub fn Announcement(bare_chrome: Memo<bool>, fullscreen: Signal<bool>) -> impl IntoView {
    let site = use_site_state();

    view! {
        {move || {
            let text = site.with(|s| s.announcement.clone());
            if bare_chrome.get() || fullscreen.get() || text.trim().is_empty() {
                return None;
            }
            let help = site.with(|s| s.help_link.clone());
            let help = ferris_web_shared::url::safe_href(&help);
            Some(view! {
                <div class="flex items-center gap-3 border-b bg-muted px-4 py-2 text-sm">
                    <span class="min-w-0 flex-1 truncate">{text}</span>
                    {help.map(|help| view! {
                        <a
                            class="shrink-0 underline"
                            href=help
                            target="_blank"
                            rel="noopener noreferrer"
                        >
                            "Ayuda"
                        </a>
                    })}
                </div>
            })
        }}
    }
}
