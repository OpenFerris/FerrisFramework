//! Attachment Gallery (`Attachment Gallery`): the document's files as a
//! thumbnail grid, Frappe-style.
//!
//! Backed by the same engagement endpoints as the form sidebar
//! (`DocStore::{list,add,remove}_attachment`) — one list, one source of truth.
//! On a `new` form there is nothing to attach to yet, so the widget says so
//! instead of offering controls that would fail. Loading, empty and error are
//! all rendered states, never a silent blank. The field carries no value of
//! its own (like Frappe's): the files live in the engagement store.

use icons::{Image, Info, Paperclip, Plus, Trash2};
use leptos::prelude::*;
use leptos::task::spawn_local;

use super::doc_context::FormDocContext;
use crate::api::DocStore;
use crate::api::types::Attachment;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::Input;
use crate::components::ui::skeleton::Skeleton;
use crate::i18n::{t, use_i18n};

#[component]
pub fn AttachmentGalleryField(id: String, read_only: bool) -> impl IntoView {
    let i18n = use_i18n();
    let reload_i18n = i18n;
    let add_i18n = i18n;
    let body_i18n = i18n;
    let ctx = use_context::<FormDocContext>();
    let items = RwSignal::new(Vec::<Attachment>::new());
    let loading = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);
    let add_url = RwSignal::new(String::new());

    let reload = move || {
        let Some(ctx) = ctx else { return };
        if ctx.is_new.get() {
            items.set(Vec::new());
            loading.set(false);
            error.set(None);
            return;
        }
        let doctype = ctx.doctype.get();
        let name = ctx.docname.get();
        loading.set(true);
        error.set(None);
        let i18n = reload_i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                loading.set(false);
                error.set(Some(i18n.t("components.form.field.server_unavailable")));
                return;
            };
            match store.list_attachments(&doctype, &name).await {
                Ok(list) => items.set(list),
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        });
    };
    // Re-fetch per document: subscribing to the identity is what makes the
    // gallery follow the router's `TASK00001 → TASK00002` navigation.
    Effect::new(move |_| {
        if let Some(ctx) = ctx {
            let _ = (ctx.docname.get(), ctx.is_new.get());
        }
        reload();
    });

    let add = move || {
        let Some(ctx) = ctx else { return };
        let url = add_url.get().trim().to_string();
        if url.is_empty() || ctx.is_new.get() {
            return;
        }
        let file_name = url.rsplit('/').next().unwrap_or_else(|| "").to_string();
        let file_name = if file_name.is_empty() {
            add_i18n.t("components.file_picker.unnamed")
        } else {
            file_name
        };
        let doctype = ctx.doctype.get();
        let name = ctx.docname.get();
        let i18n = add_i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                error.set(Some(i18n.t("components.form.field.server_unavailable")));
                return;
            };
            match store
                .add_attachment(&doctype, &name, &file_name, &url, 0)
                .await
            {
                Ok(_) => {
                    add_url.set(String::new());
                    reload();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // The states are branched inside a `move ||` so every signal read below is
    // tracked: evaluating this once at render time would leave the skeleton up
    // forever after the fetch lands.
    let body = move || {
        let Some(ctx) = ctx else {
            return view! {
                <GalleryNote text=body_i18n.t("components.form.gallery.no_context") />
            }
            .into_any();
        };
        if ctx.is_new.get() {
            return view! {
                <GalleryNote text=body_i18n.t("components.form.gallery.save_first") />
            }
            .into_any();
        }
        if loading.get() {
            return view! {
                <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
                    <Skeleton class="h-28" />
                    <Skeleton class="h-28" />
                </div>
            }
            .into_any();
        }
        if let Some(e) = error.get() {
            return view! {
                <div class="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                    <Info class="mt-0.5 size-4 shrink-0" />
                    <span class="min-w-0 flex-1">{e}</span>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| reload()
                    >
                        {t("components.form.gallery.retry")}
                    </Button>
                </div>
            }
            .into_any();
        }
        if items.get().is_empty() {
            return view! {
                <GalleryNote text=body_i18n.t("components.form.gallery.empty") />
            }
            .into_any();
        }
        let doctype = ctx.doctype;
        let docname = ctx.docname;
        view! {
            <ul class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
                {items
                    .get()
                    .into_iter()
                    .map(|a| {
                        let is_image = is_image_url(&a.file_url);
                        let (doctype, docname) = (doctype, docname);
                        // `StoredValue` is `Copy`, which is what lets the
                        // delete handler below live inside a `Show` (children
                        // must stay `Fn` — they cannot move an owned String
                        // into a nested event closure).
                        let attachment_id = StoredValue::new(a.id.clone());
                        view! {
                            <li class="group relative flex flex-col overflow-hidden rounded-lg border bg-card">
                                <a
                                    href=a.file_url.clone()
                                    target="_blank"
                                    rel="noreferrer"
                                    class="flex h-28 items-center justify-center overflow-hidden bg-muted text-muted-foreground"
                                    title=a.file_name.clone()
                                >
                                    {if is_image {
                                        view! {
                                            <img
                                                src=a.file_url.clone()
                                                alt=a.file_name.clone()
                                                class="h-full w-full object-cover"
                                            />
                                        }
                                        .into_any()
                                    } else {
                                        view! { <Paperclip class="size-6" /> }.into_any()
                                    }}
                                </a>
                                <div class="flex items-center justify-between gap-1 px-2 py-1">
                                    <span class="min-w-0 flex-1 truncate text-xs">
                                        {a.file_name.clone()}
                                    </span>
                                    <Show when=move || !read_only>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::IconXs
                                            attr:aria-label=t("components.form.gallery.remove")
                                            on:click=move |_| {
                                                let id = attachment_id.get_value();
                                                let (doctype, docname) = (doctype, docname);
                                                spawn_local(async move {
                                                    let Ok(store) = DocStore::runtime() else {
                                                        error.set(Some(t(
                                                            "components.form.field.server_unavailable",
                                                        )));
                                                        return;
                                                    };
                                                    match store
                                                        .remove_attachment(
                                                            &doctype.get(),
                                                            &docname.get(),
                                                            &id,
                                                        )
                                                        .await
                                                    {
                                                        Ok(_) => reload(),
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            }
                                        >
                                            <Trash2 class="size-3.5" />
                                        </Button>
                                    </Show>
                                </div>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any()
    };

    view! {
        <div id=id class="flex flex-col gap-2" data-name="__AttachmentGalleryField">
            {body}
            <Show
                when=move || {
                    !read_only
                        && ctx
                            .is_some_and(|c| !c.is_new.get())
                }
            >
                // The bytes live in ferris-files; this registers an
                // already-stored file, which is exactly what the endpoint
                // accepts.
                <div class="flex items-center gap-2">
                    <Input
                        placeholder=t("components.form.gallery.url_placeholder")
                        bind_value=add_url
                        attr:aria-label=t("components.form.gallery.url_aria")
                    />
                    <Button
                        variant=ButtonVariant::Secondary
                        size=ButtonSize::IconSm
                        attr:aria-label=t("components.form.gallery.attach")
                        on:click=move |_| add()
                    >
                        <Plus class="size-4" />
                    </Button>
                </div>
            </Show>
        </div>
    }
}

/// One-line note used for the new/empty/context-missing states.
#[component]
fn GalleryNote(#[prop(into)] text: String) -> impl IntoView {
    view! {
        <p class="flex items-center gap-2 rounded-md border border-dashed bg-muted/40 px-3 py-2 text-sm text-muted-foreground">
            <Image class="size-4 shrink-0" />
            {text}
        </p>
    }
}

/// Whether the URL points at something the grid can show as a thumbnail.
fn is_image_url(url: &str) -> bool {
    let path = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif"]
        .iter()
        .any(|ext| path.ends_with(ext))
}
