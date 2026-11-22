//! The list rows of the form sidebar.
//!
//! Split out of `sidebar/mod.rs` so that file keeps the data flow and the
//! section layout. Each list owns only its rendering; the removal callback is
//! the parent's (it knows the document and the local list signal).

use leptos::prelude::*;

use crate::api::types::{Assignment, Attachment, Share};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::views::user_identity::UserIdentity;

/// One row per assignee, with the button that removes the assignment.
#[component]
pub(super) fn AssignmentsList(
    assignments: RwSignal<Vec<Assignment>>,
    on_remove: Callback<String>,
) -> impl IntoView {
    view! {
        <Show when=move || !assignments.get().is_empty()>
            <div class="flex flex-col gap-1.5">
                {move || {
                    assignments
                        .get()
                        .into_iter()
                        .map(|assignment| {
                            let user = assignment.assigned_to.clone();
                            let shown = user.clone();
                            view! {
                                <div class="flex items-center justify-between gap-2 text-sm">
                                    <UserIdentity id=shown />
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::IconSm
                                        class="size-7 shrink-0 text-muted-foreground"
                                        attr:aria-label="Quitar asignación"
                                        on:click={
                                            let user = user.clone();
                                            move |_| on_remove.run(user.clone())
                                        }
                                    >
                                        <Icon name="x" class="size-3.5" />
                                    </Button>
                                </div>
                            }
                        })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}

/// One row per attachment: the file link and its remove button.
#[component]
pub(super) fn AttachmentsList(
    attachments: RwSignal<Vec<Attachment>>,
    on_remove: Callback<String>,
) -> impl IntoView {
    view! {
        <Show when=move || !attachments.get().is_empty()>
            <div class="flex flex-col gap-1.5">
                {move || {
                    attachments
                        .get()
                        .into_iter()
                        .map(|attachment| {
                            let id = attachment.id.clone();
                            let url = attachment.file_url.clone();
                            view! {
                                <div class="flex items-center justify-between gap-2 text-sm">
                                    <a
                                        class="flex min-w-0 items-center gap-2 text-primary hover:underline"
                                        href=url
                                        target="_blank"
                                        rel="noreferrer"
                                    >
                                        <span class="truncate">{attachment.file_name.clone()}</span>
                                    </a>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::IconSm
                                        class="size-7 shrink-0 text-muted-foreground"
                                        attr:aria-label="Quitar adjunto"
                                        on:click={
                                            let id = id.clone();
                                            move |_| on_remove.run(id.clone())
                                        }
                                    >
                                        <Icon name="x" class="size-3.5" />
                                    </Button>
                                </div>
                            }
                        })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}

/// One row per share: the person, the granted level and the remove button.
#[component]
pub(super) fn SharesList(
    shares: RwSignal<Vec<Share>>,
    on_remove: Callback<String>,
) -> impl IntoView {
    view! {
        <Show when=move || !shares.get().is_empty()>
            <div class="flex flex-col gap-1.5">
                {move || {
                    shares
                        .get()
                        .into_iter()
                        .map(|share| {
                            let user = share.user.clone();
                            let shown = user.clone();
                            let level = if share.write {
                                "Puede editar"
                            } else if share.read {
                                "Puede ver"
                            } else {
                                "—"
                            };
                            view! {
                                <div class="flex items-center justify-between gap-2 text-sm">
                                    <UserIdentity id=shown />
                                    <span class="shrink-0 text-xs text-muted-foreground">{level}</span>
                                    <Button
                                        variant=ButtonVariant::Ghost
                                        size=ButtonSize::IconSm
                                        class="size-7 shrink-0 text-muted-foreground"
                                        attr:aria-label="Quitar"
                                        on:click={
                                            let user = user.clone();
                                            move |_| on_remove.run(user.clone())
                                        }
                                    >
                                        <Icon name="x" class="size-3.5" />
                                    </Button>
                                </div>
                            }
                        })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}

/// The document's tags as removable chips.
#[component]
pub(super) fn TagChips(tags: Memo<Vec<String>>, on_remove: Callback<String>) -> impl IntoView {
    view! {
        <Show when=move || !tags.get().is_empty()>
            <div class="flex flex-wrap gap-1">
                {move || {
                    tags.get()
                        .into_iter()
                        .map(|tag| {
                            let label = tag.clone();
                            let title = tag.clone();
                            view! {
                                <span
                                    class="inline-flex max-w-full min-w-0 items-center gap-1 rounded-md border bg-secondary px-2 py-0.5 text-xs text-secondary-foreground"
                                    title=title
                                >
                                    <span class="truncate">{label}</span>
                                    <button
                                        type="button"
                                        class="shrink-0 rounded-sm text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none max-md:-me-1 max-md:flex max-md:size-6 max-md:items-center max-md:justify-center"
                                        aria-label=format!("Quitar etiqueta {}", tag)
                                        on:click={
                                            let tag = tag.clone();
                                            move |_| on_remove.run(tag.clone())
                                        }
                                    >
                                        <Icon name="x" class="size-3" />
                                    </button>
                                </span>
                            }
                        })
                        .collect_view()
                }}
            </div>
        </Show>
    }
}
