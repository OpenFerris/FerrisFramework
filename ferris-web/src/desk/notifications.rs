//! Live notification delivery for the Desk.
//!
//! The server exposes `GET /api/notifications/stream` as Server-Sent Events, one
//! stream per signed-in user. This module owns the browser side of it: an
//! `EventSource` that prepends every arrival to the shared feed, plus the
//! attention side effects (toast, sound, native notification) that must fire
//! once per notification.
//!
//! The stream machine is `wasm32` only; the local feed edits ([`mark_read`],
//! [`dismiss`]) are plain signal mutations and compile everywhere so the panel
//! can share them.
//!
//! Announcements are funneled through a queue signal and drained by an `Effect`
//! owned by the Desk shell, rather than performed straight from the browser
//! event callback. That is deliberate: building the toast's click `Callback`
//! needs a reactive owner, and a DOM event handler runs outside one.

use leptos::prelude::*;

use crate::api::drive;
use crate::desk::session::{self as desk_session, NotificationFeed};

/// Mark one notification read: locally right away, then on the server.
pub fn mark_read(id: &str, feed: RwSignal<NotificationFeed>) {
    if id.is_empty() {
        return;
    }

    let mut changed = false;
    feed.update(|f| {
        for item in f.items.iter_mut() {
            if item.id == id && !item.is_read {
                item.is_read = true;
                f.unread = f.unread.saturating_sub(1);
                changed = true;
            }
        }
    });

    if changed {
        let id = id.to_string();
        drive(async move {
            if desk_session::mark_notification_read(&id).await.is_err() {
                // The write did not land. Prefer the server's answer, since
                // another view may have marked this one — or everything — read
                // meanwhile; the badge and the rows come back consistent.
                if let Ok(fresh) = desk_session::notifications().await {
                    feed.set(fresh);
                } else {
                    // Still unreachable: undo the optimistic edit so the badge
                    // does not keep a count the server never accepted.
                    feed.update(|f| {
                        if let Some(item) = f.items.iter_mut().find(|item| item.id == id)
                            && item.is_read
                        {
                            item.is_read = false;
                            f.unread = f.unread.saturating_add(1);
                        }
                    });
                }
            }
        });
    }
}

/// Mark every notification read: locally right away, then on the server.
///
/// Shared by the sidebar panel and the Desk home's bell dropdown. Both can be
/// mounted at once (the dropdown is part of the shell, the panel is opened from
/// the sidebar), so the optimistic update and its rollback live here instead of
/// in either view: a second copy would sooner or later roll back to a different
/// snapshot than the one the other view is showing.
pub fn mark_all_read(feed: RwSignal<NotificationFeed>) {
    drive(async move {
        if desk_session::mark_notifications_read().await.is_ok() {
            feed.update(|f| {
                f.unread = 0;
                for item in f.items.iter_mut() {
                    item.is_read = true;
                }
            });
        } else if let Ok(fresh) = desk_session::notifications().await {
            feed.set(fresh);
        }
    });
}

/// Remove one notification from the feed, rolling the local edit back to the
/// server's answer if the request fails.
pub fn dismiss(name: &str, feed: RwSignal<NotificationFeed>) {
    if name.is_empty() {
        return;
    }

    let name = name.to_string();
    feed.update(|f| {
        if let Some(index) = f.items.iter().position(|n| n.id == name) {
            let was_unread = !f.items[index].is_read;
            f.items.remove(index);
            if was_unread {
                f.unread = f.unread.saturating_sub(1);
            }
        }
    });

    drive(async move {
        if desk_session::dismiss_notification(&name).await.is_err()
            && let Ok(fresh) = desk_session::notifications().await
        {
            feed.set(fresh);
        }
    });
}

#[cfg(target_arch = "wasm32")]
mod live {
    use std::cell::RefCell;

    use leptos::prelude::*;
    use leptos_router::NavigateOptions;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{
        EventSource, HtmlAudioElement, MessageEvent, NotificationOptions, NotificationPermission,
    };

    use crate::components::ui::toast_custom::_builder::ToastBuilder;
    use crate::components::ui::toast_custom::_context::ToasterContext;
    use crate::components::ui::toast_custom::_data::ToastLevel;
    use crate::desk::session::Notification;

    use super::{NotificationFeed, mark_read};

    /// A short synthesized "ding" (8 kHz, 8-bit PCM, embedded as a data URI) so
    /// the Desk ships no extra asset and needs no static file route for it.
    const BEEP_DATA_URI: &str = "data:audio/wav;base64,UklGRvgDAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YdQDAAB/f4CAf357ent+goWFgnx3dHd9hYuLhXxzb3J7h5CRiXxvaW15iZSWjX1sY2d2iZmckn5qXmFyiZ2imIFoWVttiaConoRnVFVoh6OupIhnUE5jhaW0qoxnTEhdgqa5sZFoSUFWf6e+uJdqRjtQeqjDv51sQzVIdafHxaNwRDJEcKPFx6d1RzJBbJ/DyKt6SjM+Z5vBya9/TjQ8Ypa/yrODUjU6XpG8y7aIVjY4Wo25y7mNWjg2Voi2y7yRXjo1UoOzyr+WYjw0Tn+vycGbZz4zSnqryMOfbEEyR3Wnx8WjcEQyRHCjxcendUcyQWyfw8irekozPmebwcmvfk40PGKWv8qzg1I1Ol6RvMu2iFY2OFqNucu5jVo4NlaItsu8kV46NVKDs8q/lmI8NE5+r8nBm2c+M0p6q8jDn2xBMkd1p8fFo3BEMkRwo8XHp3VHMkFsn8PIq3pKMz5nm8HJr35ONDxilr/Ks4NSNTpekbzLtohWNjhajbnLuY1aODZWiLbLvJFeOjVSg7PKv5ZiPDROfq/JwZtnPjNKeqvIw59sQTJHdafHxaNwRDJEcKPFx6d1RzJBbJ/DyKt6SjM+Z5vBya9+TjQ8Ypa/yrODUjU6XpG8y7aIVjY4Wo25y7mNWjg2Voi2y7yRXjo1UoOzyr+WYjw0Tn+vycGbZz4zSnqryMOfbEEyR3Wnx8WjcEQyRHCjxcendUcyQWyfw8irekozPmebwcmvfk40PGKWv8qzg1I1Ol6RvMu2iFY2OFqNucu5jVo4NlaItsu8kV46NVKDs8q/lmI8NE5/r8nBm2c+M0p6q8jDn2xBMkd1p8fFo3BEMkRwo8XHp3VHMkFsn8PIq3pKMz5nm8HJr35ONDxilr/Ks4NSNTpekbzLtohWNjhajbnLuY1aODZWiLbLvJFeOjVSg7PKv5ZiPDROf6/JwZtnPjNKeqvIw59sQTJHdafHxaNwRDJEcKPFx6d1RzJBbJ/DyKt6SjM+Z5vBya9+TjQ8Ypa/yrODUjU6XpG8y7aIVjY4Wo25y7mNWjg2Voi2y7yRXjo1UoOzyr+WYjw0Tn+vycGbZz4zSnqryMOfbEEyR3Wnx8WjcEQyRHCjxcendUcyQWyfw8irekozPmebwcmvfk40PGKWv8qzg1I1O1+Rusezh1k8Pl2LssGxi19DQl2Gq7uvjWVKR12CpbWtj2tQS15+nq+pkHBXUV98mammkXRdVmF6k6OhkHhjXGR4jp2dj3tpYWd4ipeYjX1uZ2t4hpGSi39zbW94g4uNiH93c3R6gYaHhX97eHl8f4GCgH9+fg==";

    thread_local! {
        /// The live stream, kept so `stop` can detach its handler and close it.
        /// The closure is retained because a forgotten one would be collected
        /// while the source still points at it.
        static STREAM: RefCell<Option<(EventSource, Closure<dyn FnMut(MessageEvent)>)>> =
            const { RefCell::new(None) };

        /// The reusable notification chime. Held here (not just in a local) so
        /// the browser cannot collect the element while it is still playing.
        static BEEP: RefCell<Option<HtmlAudioElement>> = const { RefCell::new(None) };
    }

    /// Open the authenticated notification stream, replacing any previous one.
    ///
    /// Each arrival is inserted at the top of `feed` (deduplicated by id, unread
    /// count bumped) and pushed onto `incoming` for the shell to announce.
    pub fn start(feed: RwSignal<NotificationFeed>, incoming: RwSignal<Vec<Notification>>) {
        stop();

        let Ok(source) = EventSource::new("/api/notifications/stream") else {
            return;
        };

        let handler = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let Some(text) = event.data().as_string() else {
                return;
            };
            let Ok(item) = serde_json::from_str::<Notification>(&text) else {
                return;
            };

            let mut fresh = false;
            feed.update(|f| {
                if f.items.iter().any(|n| n.id == item.id) {
                    return;
                }
                if !item.is_read {
                    f.unread = f.unread.saturating_add(1);
                }
                f.items.insert(0, item.clone());
                fresh = true;
            });

            if fresh {
                incoming.update(|queue| queue.push(item));
            }
        });

        source.set_onmessage(Some(handler.as_ref().unchecked_ref()));
        STREAM.with(|slot| *slot.borrow_mut() = Some((source, handler)));
    }

    /// Close the stream, if one is open. Safe to call repeatedly (logout).
    pub fn stop() {
        STREAM.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some((source, handler)) = slot.take() {
                source.set_onmessage(None);
                source.close();
                drop(handler);
            }
        });
    }

    /// Announce a newly arrived notification: a toast and a sound when the Desk
    /// is in focus, a native notification when it is not. Nothing is shown while
    /// the panel is open — the user is already looking at it.
    pub fn announce(
        item: Notification,
        feed: RwSignal<NotificationFeed>,
        panel_open: RwSignal<bool>,
        toaster: ToasterContext,
        navigate: impl Fn(&str, NavigateOptions) + Clone + Send + Sync + 'static,
    ) {
        if panel_open.get_untracked() {
            return;
        }

        if !document_has_focus() {
            show_native(&item);
            return;
        }

        play_beep();

        let id = item.id.clone();
        let route = item.route.clone();
        let nav = navigate.clone();
        let on_click = Callback::new(move |_| {
            mark_read(&id, feed);
            if !route.trim().is_empty() {
                nav(&route, NavigateOptions::default());
            }
        });

        toaster.toast(
            ToastBuilder::new(item.title.clone())
                .with_level(ToastLevel::Info)
                .with_expiry(Some(6_000))
                .with_on_click(on_click),
        );
    }

    /// Whether the Desk document currently holds focus. Defaults to `true`:
    /// showing an in-page toast beats staying silent.
    fn document_has_focus() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.has_focus().ok())
            .unwrap_or(true)
    }

    /// Play the embedded beep. The element is created once and kept in a
    /// thread-local so it is reused and stays alive across playbacks. Autoplay
    /// policies can still reject this until the user has interacted with the
    /// page; that is fine and deliberately ignored.
    fn play_beep() {
        BEEP.with(|slot| {
            let mut slot = slot.borrow_mut();

            if slot.is_none() {
                let Some(document) = web_sys::window().and_then(|w| w.document()) else {
                    return;
                };
                let Ok(element) = document.create_element("audio") else {
                    return;
                };
                let Ok(audio) = element.dyn_into::<HtmlAudioElement>() else {
                    return;
                };
                audio.set_src(BEEP_DATA_URI);
                audio.set_volume(0.4);
                audio.set_preload("auto");
                *slot = Some(audio);
            }

            if let Some(audio) = slot.as_ref() {
                // Rewind so repeated notifications each play the full chime.
                audio.set_current_time(0.0);
                let _ = audio.play();
            }
        });
    }

    /// Show a browser notification, asking for permission the first time.
    fn show_native(item: &Notification) {
        if web_sys::Notification::permission() == NotificationPermission::Default {
            // Ask once; a later arrival can use the answer.
            let _ = web_sys::Notification::request_permission();
            return;
        }

        if web_sys::Notification::permission() != NotificationPermission::Granted {
            return;
        }

        let options = NotificationOptions::new();
        options.set_body(&item.description);
        let _ = web_sys::Notification::new_with_options(&item.title, &options);
    }
}

#[cfg(target_arch = "wasm32")]
pub use live::{announce, start, stop};
