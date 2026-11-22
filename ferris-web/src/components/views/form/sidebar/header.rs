//! The header and the provenance footer of the form sidebar.

use leptos::prelude::*;

use crate::api::users;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::views::time;
use crate::components::views::user_identity::UserName;

/// The card at the top: the document itself — picture (or initials), title and
/// subtitle — with the document actions on the right.
///
/// It is the open document, not its owner: opening a User shows that user's full
/// name and email; opening a task shows the task's title and its id. The owner
/// is still one line in the provenance footer.
#[component]
pub(super) fn SidebarHeader(
    #[prop(into)] title: Signal<String>,
    #[prop(into)] subtitle: Signal<String>,
    #[prop(into)] image: Signal<String>,
    #[prop(into)] is_new: Signal<bool>,
    liked: RwSignal<bool>,
    like_count: RwSignal<u64>,
    on_like: Callback<()>,
    on_print: Callback<()>,
) -> impl IntoView {
    let initials = move || users::initials(&title.get());
    let has_image = Signal::derive(move || !image.get().trim().is_empty());
    let has_subtitle = Signal::derive(move || !subtitle.get().trim().is_empty());

    view! {
        <div class="flex flex-col gap-3">
            <div class="flex items-start justify-between gap-2">
                <Show
                    when=move || has_image.get()
                    fallback=move || view! {
                        <span
                            class="flex size-14 shrink-0 items-center justify-center rounded-xl bg-muted text-lg font-medium text-muted-foreground select-none"
                            aria-hidden="true"
                        >
                            {initials}
                        </span>
                    }
                >
                    <img
                        src=move || image.get()
                        alt=""
                        class="size-14 shrink-0 rounded-xl bg-muted object-cover"
                    />
                </Show>
                <div class="flex items-center gap-0.5">
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::IconSm
                        attr:aria-label="Imprimir"
                        attr:title="Imprimir"
                        attr:disabled=move || is_new.get()
                        on:click=move |_| on_print.run(())
                    >
                        <Icon name="printer" />
                    </Button>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::IconSm
                        attr:aria-label="Me gusta"
                        attr:title=move || format!("Me gusta ({})", like_count.get())
                        attr:aria-pressed=move || liked.get().to_string()
                        attr:disabled=move || is_new.get()
                        on:click=move |_| on_like.run(())
                    >
                        // Two static icons, not a reactive `class`: `Icon`
                        // evaluates its class once, so a signal read there warns
                        // and never updates.
                        // Both icons stay mounted and CSS picks one: a `Show`
                        // predicate is evaluated while the view is built, which
                        // reads `liked` outside a tracking scope.
                        <span class="contents" class:hidden=move || liked.get()>
                            <Icon name="heart" class="size-4" />
                        </span>
                        <span class="contents" class:hidden=move || !liked.get()>
                            <Icon name="heart" class="size-4 fill-current text-destructive" />
                        </span>
                    </Button>
                </div>
            </div>

            <div class="flex min-w-0 flex-col gap-0.5">
                <p class="truncate text-base font-semibold text-foreground" title=move || title.get()>
                    {move || title.get()}
                </p>
                <Show when=move || has_subtitle.get()>
                    <p class="truncate text-sm text-muted-foreground">
                        {move || subtitle.get()}
                    </p>
                </Show>
            </div>
        </div>
    }
}

/// One provenance line: who, and how long ago.
#[component]
fn ProvenanceLine(
    icon: &'static str,
    label: &'static str,
    who: Signal<String>,
    at: Signal<String>,
) -> impl IntoView {
    let relative = Signal::derive(move || time::relative(&at.get()));
    let exact = Signal::derive(move || time::exact(&at.get()));
    let has_who = Signal::derive(move || !who.get().trim().is_empty());
    view! {
        <div class="flex items-start gap-2">
            <Icon name=icon class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
            <div class="flex min-w-0 flex-col gap-0.5">
                <p class="text-sm leading-5 text-muted-foreground">
                    {label} " "
                    <Show when=move || has_who.get() fallback=|| view! { <span>"—"</span> }>
                        <span class="font-medium text-foreground"><UserName id=who /></span>
                    </Show>
                </p>
                <p class="text-xs text-muted-foreground" title=move || exact.get()>
                    {move || relative.get()}
                </p>
            </div>
        </div>
    }
}

/// "Última edición por …" and "Creado por …", the document's provenance.
#[component]
pub(super) fn SidebarProvenance(
    owner: Signal<String>,
    edited_by: Signal<String>,
    creation: Signal<String>,
    modified: Signal<String>,
) -> impl IntoView {
    view! {
        <section class="flex flex-col gap-3" aria-label="Procedencia">
            <ProvenanceLine icon="pencil" label="Última edición por" who=edited_by at=modified />
            <ProvenanceLine icon="user-round" label="Creado por" who=owner at=creation />
        </section>
    }
}
