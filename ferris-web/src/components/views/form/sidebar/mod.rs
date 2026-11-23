//! The form's static right sidebar.
//!
//! The layout follows the document apps the Desk is modelled on: a card with the
//! **open document's** picture (or initials), title and subtitle — a User shows
//! that user's full name and email, a task its title and id — plus the document
//! actions at the top right; then one row per collaboration surface — Assign,
//! Attachments, Tags, Share — each with a `+` that opens just that surface; then
//! the provenance footer (who edited it and who created it, with relative
//! times); then the declared connections.
//!
//! Everything here is backed by a real endpoint: assignments, attachments and
//! shares come from `/api/engagement/{doctype}/{name}/...`, tags are the
//! document's own `tags` field written through the ordinary update, and the
//! provenance is read from the raw document the service already holds.
//!
//! The pickers are mounted only while their row is open (`+`), so the sidebar is
//! a short list instead of four always-visible search boxes. The tag chips stay
//! visible when the search box is closed: they are the section's content, the
//! box is only how more get added.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::files::{self, FileItem};
use crate::api::types::{Assignment, Attachment, ConnectionGroup, Share};
use crate::components::pickers::{DocPicker, FilePicker, PickerMode};
use crate::components::ui::select_native::SelectNative;
use crate::components::ui::separator::Separator;
use crate::components::views::form::sidebar_extra::{ConnectionsPanel, DocTypeActions};
use crate::views::services::FormService;

mod header;
mod helpers;
mod items;
mod row;

use crate::i18n::{t, use_i18n};
use header::{SidebarHeader, SidebarProvenance};
use helpers::{
    document_image, document_subtitle, document_title, edited_by, owner_of, parse_tags, stamp_of,
};
use items::{AssignmentsList, AttachmentsList, SharesList, TagChips};
use row::SidebarRow;

#[component]
pub fn FormSidebar(service: FormService) -> impl IntoView {
    let attachment_i18n = use_i18n();
    let print_i18n = use_i18n();
    let service = StoredValue::new(service);
    let svc = service.get_value();
    let doctype = svc.doctype;
    let docname = svc.docname;
    let is_new = svc.is_new;

    let assignments = RwSignal::new(Vec::<Assignment>::new());
    let attachments = RwSignal::new(Vec::<Attachment>::new());
    let shares = RwSignal::new(Vec::<Share>::new());
    let liked = RwSignal::new(false);
    let like_count = RwSignal::new(0u64);
    let error = RwSignal::new(None::<String>);
    // Connections come from the stored `DocType Link` declarations; the server
    // resolves the documents and applies the caller's read permission.
    let connections = RwSignal::new(Vec::<ConnectionGroup>::new());
    let connections_error = RwSignal::new(None::<String>);
    // `link_doctype` → module, so a connected document links to its real form
    // route (`/desk/:module/:doctype/:name`) instead of guessing the segment.
    let connection_modules = RwSignal::new(HashMap::<String, String>::new());
    // DocType Actions are declared metadata, not a per-document fetch.
    let meta_actions = svc.meta;
    // The action payload is built from the live form fields, so a Server Action
    // sees the same values the user sees (Frappe passes the doc as kwargs).
    let fields_signal = svc.fields;

    // Which section has its editor open. One at a time: two open pickers in a
    // 18rem column overlap their dropdowns.
    let assign_open = RwSignal::new(false);
    let tags_open = RwSignal::new(false);
    let share_open = RwSignal::new(false);
    let attach_picker = RwSignal::new(false);

    // Inputs.
    let assign_pick = RwSignal::new(Vec::<String>::new());
    let share_pick = RwSignal::new(Vec::<String>::new());
    // Share level: read-only unless the user explicitly grants editing.
    let share_level = RwSignal::new("read".to_string());
    let tag_pick = RwSignal::new(Vec::<String>::new());

    // Load everything the sidebar shows, per document.
    let reload = Callback::new(move |()| {
        // A callback, not a render: `get` here warned about reading outside a
        // reactive context on every picker action.
        if is_new.get_untracked() {
            return;
        }
        let dt = doctype.get();
        let name = docname.get();
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            if let Ok(list) = store.list_assignments(&dt, &name).await {
                assignments.set(list);
            }
            if let Ok(list) = store.list_attachments(&dt, &name).await {
                attachments.set(list);
            }
            if let Ok(list) = store.list_shares(&dt, &name).await {
                shares.set(list);
            }
            let names = vec![name.clone()];
            if let Ok(page) = store.engagement(&dt, &names).await {
                liked.set(page.is_liked(&name));
                like_count.set(page.like_counts.get(&name).copied().unwrap_or(0));
            }
            match store.connections(&dt, &name).await {
                Ok(payload) => {
                    let mut modules = HashMap::new();
                    for group in &payload.connections {
                        if let Ok(Some(meta)) = store.meta(&group.link_doctype).await {
                            modules.insert(group.link_doctype.clone(), meta.module);
                        }
                    }
                    connection_modules.set(modules);
                    connections.set(payload.connections);
                    connections_error.set(None);
                }
                // A server that predates the endpoint answers 404; that is
                // "no declared connections", not a failure the user can act on.
                Err(message)
                    if message.contains("404") || message.to_lowercase().contains("not found") =>
                {
                    connections.set(Vec::new());
                    connections_error.set(None);
                }
                Err(message) => {
                    connections.set(Vec::new());
                    connections_error.set(Some(message));
                }
            }
        });
    });
    Effect::new(move |_| {
        // Subscribed to the docname (and the first run, when it is still empty).
        let _ = docname.get();
        let _ = is_new.get();
        reload.run(());
    });

    // ---- tags, derived from the raw document ----
    // The standard `tags` field is the source of truth the engine derives the
    // Tag catalogue from (`ferris_api::tags`); `_user_tags` was a second,
    // invisible copy that the catalogue ignored and the form never showed.
    let tags = Memo::new(move |_| parse_tags(service.get_value().doc.get().get("tags")));
    // The tags picker follows the document: a reload or another writer must not
    // leave it showing a stale list.
    Effect::new(move |_| {
        let stored = tags.get();
        if tag_pick.get_untracked() != stored {
            tag_pick.set(stored);
        }
    });

    // ---- document identity, as signals the header/footer read ----
    // The header describes the open document (a User shows that user's name and
    // email, a task shows its title and id); the owner is the footer's business.
    let title = Memo::new(move |_| {
        let svc = service.get_value();
        let doc = svc.doc.get();
        let meta = svc.meta.get();
        document_title(&doc, meta.as_ref())
    });
    let subtitle = Memo::new(move |_| {
        let svc = service.get_value();
        let doc = svc.doc.get();
        let meta = svc.meta.get();
        let title = document_title(&doc, meta.as_ref());
        document_subtitle(&doc, &svc.doctype.get(), &title)
    });
    let image = Memo::new(move |_| document_image(&service.get_value().doc.get()));
    let owner = Signal::derive(move || owner_of(&service.get_value().doc.get()));
    let editor = Signal::derive(move || edited_by(&service.get_value().doc.get()));
    let creation = Signal::derive(move || stamp_of(&service.get_value().doc.get(), "creation"));
    let modified = Signal::derive(move || stamp_of(&service.get_value().doc.get(), "modified"));

    let add_assignment = move |user: String| {
        let user = user.trim().to_string();
        if user.is_empty() {
            return;
        }
        let (dt, name) = (doctype.get(), docname.get());
        error.set(None);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            // The server answers with the row it stored (or the existing one):
            // push it into the list instead of re-reading five endpoints, which
            // used to blank the section and refill it on every click.
            match store.add_assignment(&dt, &name, &user).await {
                Ok(row) => assignments.update(|list| {
                    if !list.iter().any(|item| item.assigned_to == row.assigned_to) {
                        list.push(row);
                    }
                }),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let add_attachment = move |item: FileItem| {
        let (dt, name) = (doctype.get(), docname.get());
        let file_name = if item.name.is_empty() {
            attachment_i18n.t("components.file_picker.unnamed")
        } else {
            item.name.clone()
        };
        let url = files::url_of(&item);
        error.set(None);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.add_attachment(&dt, &name, &file_name, &url, 0).await {
                Ok(row) => attachments.update(|list| list.push(row)),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let add_share = move |user: String, write: bool| {
        let user = user.trim().to_string();
        if user.is_empty() {
            return;
        }
        let (dt, name) = (doctype.get(), docname.get());
        error.set(None);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            // Read-only by default: a share that silently granted write would be
            // a permission change the user did not ask for. The form lets the
            // caller pick the level through `add_share_with`.
            match store.add_share(&dt, &name, &user, true, write, false).await {
                Ok(row) => shares.update(|list| {
                    if !list.iter().any(|item| item.user == row.user) {
                        list.push(row);
                    }
                }),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // The picker reports the whole selection: write it in one update, which is
    // the same `tags` field the engine derives the Tag catalogue from.
    let set_tags = Callback::new(move |next: Vec<String>| {
        let (dt, name) = (doctype.get(), docname.get());
        let joined = next.join(", ");
        let body = serde_json::json!({ "tags": joined });
        // Clone the service here: reading the `StoredValue` after the await can
        // outlive it (tab switch, navigation) and the signals are `Copy`.
        let svc = service.get_value();
        error.set(None);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.update(&dt, &name, body).await {
                Ok(stored) => {
                    // `dirty` is read before touching the field: an unsaved edit
                    // in another control must keep the form dirty.
                    let was_dirty = svc.dirty.get_untracked();
                    svc.apply_stored_doc(&stored);
                    svc.set_field_value("tags", &joined);
                    if !was_dirty {
                        svc.accept_current_as_saved();
                    }
                }
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let remove_assignment = Callback::new(move |user: String| {
        let (dt, name) = (doctype.get(), docname.get());
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.remove_assignment(&dt, &name, &user).await {
                Ok(()) => assignments.update(|list| list.retain(|item| item.assigned_to != user)),
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let remove_attachment = Callback::new(move |id: String| {
        let (dt, name) = (doctype.get(), docname.get());
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.remove_attachment(&dt, &name, &id).await {
                Ok(()) => attachments.update(|list| list.retain(|item| item.id != id)),
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let remove_share = Callback::new(move |user: String| {
        let (dt, name) = (doctype.get(), docname.get());
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.remove_share(&dt, &name, &user).await {
                Ok(()) => shares.update(|list| list.retain(|item| item.user != user)),
                Err(e) => error.set(Some(e)),
            }
        });
    });

    let remove_tag = Callback::new(move |tag: String| {
        let next: Vec<String> = tags
            .get_untracked()
            .into_iter()
            .filter(|current| current != &tag)
            .collect();
        set_tags.run(next);
    });

    let toggle_like = move || {
        let (dt, name) = (doctype.get(), docname.get());
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            match store.toggle_like(&dt, &name).await {
                Ok(state) => {
                    liked.set(state.liked);
                    like_count.set(state.count);
                }
                // A like the server rejects is not "nothing happened": say so
                // instead of leaving the heart unchanged with no explanation.
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // Only one editor at a time: an open picker's dropdown must not overlap the
    // next section's.
    let toggle_assign = Callback::new(move |()| {
        let next = !assign_open.get_untracked();
        assign_open.set(next);
        if next {
            tags_open.set(false);
            share_open.set(false);
        }
    });
    let toggle_tags = Callback::new(move |()| {
        let next = !tags_open.get_untracked();
        tags_open.set(next);
        if next {
            assign_open.set(false);
            share_open.set(false);
        }
    });
    let toggle_share = Callback::new(move |()| {
        let next = !share_open.get_untracked();
        share_open.set(next);
        if next {
            assign_open.set(false);
            tags_open.set(false);
        }
    });

    let form_reload = Callback::new(move |()| service.get_value().reload());

    let print_document = Callback::new(move |()| {
        let (dt, name) = (doctype.get_untracked(), docname.get_untracked());
        error.set(None);
        let i18n = print_i18n;
        spawn_local(async move {
            // The server's print engine renders the document's print format and
            // checks `Permission::Print`; the browser only shows the PDF.
            if let Err(message) = crate::api::print::open(&dt, &name, &i18n).await {
                error.set(Some(message));
            }
        });
    });

    view! {
        <aside
            class="flex w-full shrink-0 flex-col gap-4 lg:w-72"
            data-name="FormSidebar"
        >
            <SidebarHeader
                title=title
                subtitle=subtitle
                image=image
                is_new=is_new
                liked=liked
                like_count=like_count
                on_like=Callback::new(move |()| toggle_like())
                on_print=print_document
            />

            <Separator />

            <Show when=move || error.get().is_some()>
                <div
                    class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
                    role="alert"
                >
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            <DocTypeActions
                meta=meta_actions
                doctype=doctype
                docname=docname
                fields=fields_signal
                error=error
                // A Server Action's effects are unknown, so the form is re-read
                // (the sidebar lists are not: they do not change).
                reload=form_reload
            />

            // ---- Assign ----
            <section class="flex flex-col gap-2">
                <SidebarRow
                    icon="user-plus"
                    label=t("components.form.sidebar.assign")
                    aria_label=t("components.form.sidebar.assign_user")
                    on_add=toggle_assign
                />
                <AssignmentsList assignments=assignments on_remove=remove_assignment />
                <Show when=move || assign_open.get()>
                    <DocPicker
                        id="sidebar-assign".to_string()
                        target="User".to_string()
                        values=assign_pick
                        mode=PickerMode::Single
                        display_field="full_name"
                        placeholder=t("components.form.sidebar.search_user")
                        autofocus=true
                        on_select=Callback::new(move |next: Vec<String>| {
                            if let Some(user) = next.first().cloned() {
                                add_assignment(user);
                            }
                            assign_pick.set(Vec::new());
                            assign_open.set(false);
                        })
                    />
                </Show>
            </section>

            // ---- Attachments ----
            <section class="flex flex-col gap-2">
                <SidebarRow
                    icon="paperclip"
                    label=t("components.form.sidebar.attachments")
                    aria_label=t("components.form.sidebar.attach_file")
                    on_add=Callback::new(move |()| attach_picker.set(true))
                />
                <AttachmentsList attachments=attachments on_remove=remove_attachment />
                <FilePicker
                    open=attach_picker
                    on_pick=Callback::new(move |item: FileItem| add_attachment(item))
                />
            </section>

            // ---- Tags ----
            <section class="flex flex-col gap-2">
                <SidebarRow
                    icon="tag"
                    label=t("components.form.sidebar.tags")
                    aria_label=t("components.form.sidebar.add_tag")
                    on_add=toggle_tags
                />
                <TagChips tags=tags on_remove=remove_tag />
                <Show when=move || tags_open.get()>
                    <DocPicker
                        id="sidebar-tags".to_string()
                        target="Tag".to_string()
                        values=tag_pick
                        mode=PickerMode::Multiple
                        placeholder=t("components.form.field.tags_placeholder")
                        allow_free_text=true
                        show_chips=false
                        autofocus=true
                        on_change=Callback::new(move |next: Vec<String>| set_tags.run(next))
                    />
                </Show>
            </section>

            // ---- Share ----
            <section class="flex flex-col gap-2">
                <SidebarRow
                    icon="share-2"
                    label=t("components.form.sidebar.share")
                    aria_label=t("components.form.sidebar.share_with_user")
                    on_add=toggle_share
                />
                <SharesList shares=shares on_remove=remove_share />
                <Show when=move || share_open.get()>
                    <SelectNative
                        id="sidebar-share-level".to_string()
                        value=share_level.read_only()
                        aria_label=t("components.form.sidebar.share_permission")
                        on_change=Callback::new(move |ev: leptos::ev::Event| {
                            share_level.set(event_target_value(&ev));
                        })
                    >
                        <option value="read">{t("components.form.sidebar.can_view")}</option>
                        <option value="write">{t("components.form.sidebar.can_edit")}</option>
                    </SelectNative>
                    <DocPicker
                        id="sidebar-share".to_string()
                        target="User".to_string()
                        values=share_pick
                        mode=PickerMode::Single
                        display_field="full_name"
                        placeholder=t("components.form.sidebar.search_user")
                        autofocus=true
                        on_select=Callback::new(move |next: Vec<String>| {
                            if let Some(user) = next.first().cloned() {
                                let can_edit = share_level.get_untracked() == "write";
                                add_share(user, can_edit);
                            }
                            share_pick.set(Vec::new());
                            share_open.set(false);
                        })
                    />
                </Show>
            </section>

            <Separator />

            <SidebarProvenance
                owner=owner
                edited_by=editor
                creation=creation
                modified=modified
            />

            <ConnectionsPanel
                connections=connections
                error=connections_error
                modules=connection_modules
            />
        </aside>
    }
}
