//! Transient toasts for the top bar's actions.
//!
//! The Desk already owns a toast host (`ferris_web_ui::components::ui::
//! toast_custom`, provided once by the shell), so the form contributes to it
//! instead of drawing a second stack: two hosts would show two overlays for one
//! action, with different lifetimes. Nothing here is persisted — the queue is
//! in memory, the host removes each entry on expiry, and closing the tab loses
//! them. That is the point: a toast reports *this click*, while the
//! notifications panel is history, and an action that also wrote to history
//! would leave a permanent record of "Documento guardado".
//!
//! [`Toasts`] is captured once, in the component body, and then cloned into the
//! callbacks that need it. `use_context` cannot be called from a click handler
//! or from an async task (there is no reactive owner there), which is exactly
//! where most of these messages originate.

use leptos::prelude::*;

use crate::components::ui::toast_custom::_builder::ToastBuilder;
use crate::components::ui::toast_custom::_context::ToasterContext;
use crate::components::ui::toast_custom::_data::ToastLevel;

/// How long a form toast stays on screen.
///
/// Long enough to read a server message (the default 2.5 s truncated error
/// text), short enough that a burst of saves does not pile up.
const EXPIRY_MS: u32 = 4_000;

/// A cheap handle on the Desk's toast host, or a no-op when there is none.
///
/// A standalone render (a test, a story) has no toaster provided; the action
/// must still work, so the absent case silently drops the message instead of
/// panicking on `expect_context`.
#[derive(Clone)]
pub(super) struct Toasts(Option<ToasterContext>);

impl Toasts {
    /// Take the host provided by the shell, if any.
    pub(super) fn from_context() -> Self {
        Self(use_context::<ToasterContext>())
    }

    /// Report that an action landed.
    pub(super) fn success(&self, message: impl Into<String>) {
        self.emit(ToastLevel::Success, message.into());
    }

    /// Report that an action failed; the message is the server's own text.
    pub(super) fn error(&self, message: impl Into<String>) {
        self.emit(ToastLevel::Error, message.into());
    }

    fn emit(&self, level: ToastLevel, message: String) {
        let Some(toaster) = &self.0 else {
            return;
        };
        // `ToastLevel::Error`/`Warn` are announced assertively and the rest
        // politely by the shared toast, so the caller never picks a role.
        toaster.toast(
            ToastBuilder::new(message)
                .with_level(level)
                .with_expiry(Some(EXPIRY_MS)),
        );
    }
}
