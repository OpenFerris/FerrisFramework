//! The file picker: choose an uploaded file or upload a new one.
//!
//! One dialog for every "attach a file" affordance (the `Attach`/`Attach Image`
//! fields, the sidebar's attachments). It lists `GET /api/files`, filters by
//! name while typing (debounced), uploads through `POST /api/files` and hands
//! the selected [`FileItem`] back to its owner.
//!
//! Only the browser can read a `File` and `fetch` its bytes, so the upload
//! handler is `wasm32`-only; on the server build the control still lists and
//! selects (the dialog is inert without a user anyway).

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::files::{self, FileItem};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::components::ui::icon::Icon;
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::skeleton::Skeleton;
use crate::components::ui::spinner::Spinner;

/// Dialog bound to `open`; `on_pick` receives the chosen file.
#[component]
pub fn FilePicker(
    open: RwSignal<bool>,
    #[prop(into)] on_pick: Callback<FileItem>,
    /// Optional `accept` for the file input (`image/*`, `.pdf,image/*`, …).
    /// Empty means "any file" (the prop is a plain string because a Leptos
    /// `optional` prop cannot be handed an `Option` from the caller).
    #[prop(optional, into)]
    accept: String,
) -> impl IntoView {
    let items = RwSignal::new(Vec::<FileItem>::new());
    let query = RwSignal::new(String::new());
    let loading = RwSignal::new(false);
    let uploading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let seq = StoredValue::new(0u64);
    let file_input = NodeRef::<leptos::html::Input>::new();
    let accept = StoredValue::new(if accept.is_empty() {
        None
    } else {
        Some(accept)
    });

    // The list follows the dialog and the query. Reopening always starts from
    // the current store instead of a stale page.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        let search = query.get().trim().to_string();
        let generation = seq.try_with_value(|value| *value).unwrap_or(0) + 1;
        seq.set_value(generation);
        loading.set(true);
        error.set(None);
        set_timeout(
            move || {
                spawn_local(async move {
                    let answer = files::list(&search, 50).await;
                    if seq.try_with_value(|value| *value) != Some(generation) {
                        return;
                    }
                    match answer {
                        Ok(found) => items.set(found),
                        Err(message) => {
                            items.set(Vec::new());
                            error.set(Some(message));
                        }
                    }
                    loading.set(false);
                });
            },
            std::time::Duration::from_millis(200),
        );
    });

    let pick = Callback::new(move |item: FileItem| {
        on_pick.run(item);
        open.set(false);
    });

    let choose = Callback::new(move |_: ()| {
        if let Some(input) = file_input.get_untracked() {
            let _ = input.click();
        }
    });

    let on_file = move |event: leptos::ev::Event| {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            use wasm_bindgen_futures::JsFuture;

            let Some(input) = event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                return;
            };
            let Some(file) = input.files().and_then(|list| list.get(0)) else {
                return;
            };
            let name = file.name();
            let content_type = {
                let declared = file.type_();
                if declared.is_empty() {
                    "application/octet-stream".to_string()
                } else {
                    declared
                }
            };
            uploading.set(true);
            error.set(None);
            let input_for_reset = input.clone();
            spawn_local(async move {
                let Ok(buffer) = JsFuture::from(file.array_buffer()).await else {
                    error.set(Some("No se pudo leer el archivo.".to_string()));
                    uploading.set(false);
                    return;
                };
                let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                match files::upload(&name, &content_type, bytes).await {
                    Ok(item) => {
                        input_for_reset.set_value("");
                        uploading.set(false);
                        pick.run(item);
                    }
                    Err(message) => {
                        error.set(Some(message));
                        uploading.set(false);
                    }
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = event;
        }
    };

    view! {
        <Dialog>
            <DialogContent
                open=open
                class="max-w-lg max-h-[85dvh] overflow-y-auto"
                close_on_backdrop_click=true
            >
                <DialogHeader>
                    <DialogTitle>"Elegir archivo"</DialogTitle>
                    <DialogDescription>
                        "Selecciona uno ya subido o sube uno nuevo."
                    </DialogDescription>
                </DialogHeader>

                <div class="flex flex-wrap items-center gap-2">
                    <div class="min-w-0 flex-1">
                        <Input
                            id="file-picker-search"
                            r#type=InputType::Search
                            placeholder="Buscar archivo…"
                            prop:value=move || query.get()
                            on_input=Callback::new(move |text: String| query.set(text))
                        />
                    </div>
                    <Button
                        variant=ButtonVariant::Secondary
                        size=ButtonSize::Sm
                        on:click=move |_| choose.run(())
                        attr:disabled=move || uploading.get()
                    >
                        {move || {
                            if uploading.get() {
                                view! { <Spinner class="size-4" /> }.into_any()
                            } else {
                                view! { <Icon name="upload" class="size-4" /> }.into_any()
                            }
                        }}
                        "Subir archivo"
                    </Button>
                    <input
                        type="file"
                        class="hidden"
                        node_ref=file_input
                        accept=move || accept.get_value().unwrap_or_default()
                        on:change=on_file
                    />
                </div>

                {move || {
                    error.get().map(|message| {
                        view! {
                            <div class="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive" role="alert">
                                <Icon name="circle-alert" class="mt-0.5 size-4 shrink-0" />
                                <span class="min-w-0 break-words">{message}</span>
                            </div>
                        }
                    })
                }}

                <div class="flex flex-col gap-1">
                    {move || {
                        if loading.get() {
                            return view! {
                                <div class="flex flex-col gap-2">
                                    <Skeleton class="h-10 w-full" />
                                    <Skeleton class="h-10 w-full" />
                                    <Skeleton class="h-10 w-full" />
                                </div>
                            }
                            .into_any();
                        }
                        let rows = items.get();
                        if rows.is_empty() {
                            return view! {
                                <p class="py-4 text-center text-sm text-muted-foreground">
                                    "No hay archivos. Sube el primero."
                                </p>
                            }
                            .into_any();
                        }
                        rows.into_iter()
                            .map(|item| {
                                let href = files::url_of(&item);
                                let image = item.content_type.starts_with("image/");
                                let name = if item.name.is_empty() {
                                    "archivo".to_string()
                                } else {
                                    item.name.clone()
                                };
                                let size = files::human_size(item.size);
                                let created = item
                                    .created_at
                                    .as_deref()
                                    .map(|stamp| stamp.get(..16).unwrap_or(stamp).replace('T', " "))
                                    .unwrap_or_default();
                                let pick = pick;
                                view! {
                                    <button
                                        type="button"
                                        class="flex w-full min-w-0 items-center gap-3 rounded-md border bg-card px-3 py-2 text-start text-sm transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 max-md:min-h-12"
                                        on:click=move |_| pick.run(item.clone())
                                    >
                                        <span class="flex size-9 shrink-0 items-center justify-center overflow-hidden rounded-md bg-muted text-muted-foreground">
                                            {if image {
                                                view! { <img src=href.clone() alt="" class="size-9 object-cover" /> }
                                                    .into_any()
                                            } else {
                                                view! { <Icon name="paperclip" class="size-4" /> }.into_any()
                                            }}
                                        </span>
                                        <span class="flex min-w-0 flex-1 flex-col">
                                            <span class="truncate font-medium">{name}</span>
                                            <span class="text-xs text-muted-foreground">
                                                {size} " · " {created}
                                            </span>
                                        </span>
                                    </button>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </div>
            </DialogContent>
        </Dialog>
    }
}
