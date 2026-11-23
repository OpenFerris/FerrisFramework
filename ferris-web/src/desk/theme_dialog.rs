//! The theme chooser dialog, shared by the sidebar's module menu and the Desk
//! home's top bar.
//!
//! It used to live inline in `sidebar/header_dropdown.rs`. The Desk home now
//! offers the same action from its avatar menu, and two copies of a dialog that
//! lists themes, marks the active one and paints a preview would drift: the
//! preview reads a theme's own variables, so a change to the token list has to
//! reach both entry points. The dialog is therefore one component with one open
//! signal per owner.
//!
//! The preview is a miniature window painted with the theme's own palette: the
//! choice is made on the colours and not on the name.

use leptos::prelude::*;

use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::i18n::use_i18n;
use crate::theme::{ThemeCtx, ThemeSpec};
use icons::Check;

/// The "Seleccionar tema" dialog. `open` is owned by the caller so the trigger can
/// be a menu item, a row or a button without the dialog knowing about it.
#[component]
pub fn ThemeDialog(open: RwSignal<bool>) -> impl IntoView {
    let theme = StoredValue::new(expect_context::<ThemeCtx>());
    let i18n = use_i18n();

    view! {
        <Dialog>
            <DialogContent open=open class="max-w-sm">
                <DialogHeader>
                    <DialogTitle>{ i18n.t("desk.theme.select") }</DialogTitle>
                    <DialogDescription>{ i18n.t("desk.theme.choose") }</DialogDescription>
                </DialogHeader>

                <div class="flex flex-col gap-1">
                    {move || open.get().then(|| {
                        let ctx = theme.get_value();
                        let active_id = ctx.current.get();
                        ctx.themes
                            .get()
                            .into_iter()
                            .map(|spec| {
                            let is_active = spec.id == active_id;
                            let theme = ctx.clone();
                            let pick = spec.id.clone();
                            let theme_id = spec.id.clone();
                            let label = spec.label.clone();
                            let preview = spec.clone();

                            view! {
                                <Button
                                    variant=if is_active { ButtonVariant::Accent } else { ButtonVariant::Ghost }
                                    attr:data-theme-id=theme_id
                                    class="h-auto w-full justify-start gap-3 py-2"
                                    on:click=move |_| {
                                        theme.select(&pick);
                                        open.set(false);
                                    }
                                >
                                    <ThemePreview spec=preview />
                                    <span class="flex min-w-0 flex-1 flex-col items-start">
                                        <span class="truncate text-sm font-medium">{label}</span>
                                        <span class="truncate text-xs text-muted-foreground">{spec.id.clone()}</span>
                                    </span>
                                    {move || is_active.then(|| view! { <Check /> })}
                                </Button>
                            }
                        }).collect_view()
                    })}
                </div>
            </DialogContent>
        </Dialog>
    }
}

/// Miniature of a theme: a window frame painted with that theme's own palette,
/// so the choice is made on the colours and not on the name.
#[component]
fn ThemePreview(spec: ThemeSpec) -> impl IntoView {
    let frame = var_of(&spec, "--border");
    let window_bg = var_of(&spec, "--background");
    let title_bar_bg = var_of(&spec, "--muted");
    let dot = var_of(&spec, "--muted-foreground");
    let bar = var_of(&spec, "--accent");
    view! {
        <span
            aria-hidden="true"
            class="flex h-16 w-24 shrink-0 flex-col overflow-hidden rounded-md border"
            style=move || format!("border-color:{frame};background:{window_bg}")
        >
            <span class="flex h-4 items-center gap-1 rounded-t-md px-1.5" style=format!("background:{title_bar_bg}")>
                <span class="size-1.5 rounded-full opacity-70" style=format!("background:{dot}")></span>
                <span class="size-1.5 rounded-full opacity-70" style=format!("background:{dot}")></span>
                <span class="size-1.5 rounded-full opacity-70" style=format!("background:{dot}")></span>
            </span>
            <span class="flex flex-1 flex-col gap-1 p-1.5">
                <span class="h-1.5 rounded-sm" style=format!("background:{bar}")></span>
                <span class="h-1.5 w-3/4 rounded-sm" style=format!("background:{bar}")></span>
                <span class="h-1.5 w-1/2 rounded-sm" style=format!("background:{bar}")></span>
            </span>
        </span>
    }
}

/// Helper to read a palette variable from a theme with safe fallback.
fn var_of(spec: &ThemeSpec, key: &str) -> String {
    let key = key.trim_start_matches("--");
    spec.vars
        .iter()
        .find(|(k, _)| k.trim_start_matches("--") == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "var(--muted-foreground)".to_string())
}
