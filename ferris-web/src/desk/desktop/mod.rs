//! The desk grid (`/desk`): one square icon per workspace, with folders grouping
//! icons into dialogs, all arranged by the person signed in.
//!
//! The grid is the user's. What the apps declared seeds it, and everything after
//! that — the order, the folders, which icons are hidden — is their own
//! arrangement, stored server-side and read back on every load. Every edit goes
//! through one save, so what is drawn and what is stored cannot drift apart.
//!
//! Two things shape the code. Folders are one level deep by construction: a
//! folder holds workspaces, so opening one ends in a dialog with nothing below
//! it. And every control is a plain element carrying `data-action`, with the
//! grid listening once for all of them, rather than a `Callback` per control:
//! the grid is rebuilt whenever the arrangement reloads, and a `Callback`
//! created while it is being rebuilt belongs to that render effect's arena,
//! which is emptied on the next re-run, leaving a listener holding a disposed
//! value.
//!
//! The file is split by concern: this module owns the page and its delegated
//! handlers, [`arrange`] holds the pure grid edits (the part that is testable
//! without a browser), [`dom`] the pointer/target helpers, [`tiles`] the icon
//! square and its caption, and `tests` the arrangement cases.

mod arrange;
mod dom;
mod tiles;

#[cfg(test)]
mod tests;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::drive;
use crate::app::DeskNav;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
};
use crate::components::ui::icon::Icon;
use crate::components::ui::input::Input;
use crate::components::ui::label::Label;
use crate::desk::session::{self as desk_session, DeskIcon};
use crate::i18n::{DeskI18n, use_i18n};

use arrange::{
    add_folder, delete_folder, edit_folder, folder_of, hide_icon, move_icon, placements_of,
};
use dom::{Seam, behind, closest_attr, landed_on};
use tiles::{
    DEFAULT_FOLDER_ICON, FOLDER_COLORS, FOLDER_ICONS, IconGrid, folder_color_label,
    folder_icon_label,
};

/// The desk grid: every workspace as an icon, the folders grouping them, and an
/// edit mode that turns the icons into something the user can rearrange.
#[component]
pub fn Desktop() -> impl IntoView {
    let nav = expect_context::<DeskNav>();
    // `StoredValue` keeps the translator `Copy`, so the delegated `edit`
    // callback and the dialogs can share it.
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));

    // Editing is off by default: a click on an icon navigates, and only in edit
    // mode does an icon mean "pick this up".
    let editing = RwSignal::new(false);
    // The icon being dragged and the one it would land on. Plain values read by
    // the delegated handlers below, because a `Callback` built inside a
    // re-running reactive closure is disposed along with it.
    let dragging = RwSignal::new(None::<String>);
    let over = RwSignal::new(None::<Seam>);
    // Which folder's dialog is open. `DialogContent` is driven by a plain `bool`,
    // so the id is held next to it rather than inside it.
    let folder_open = RwSignal::new(false);
    let folder_id = RwSignal::new(None::<String>);
    // The folder the form is open on, and the three fields it drafts. The form
    // edits a draft and commits all three at once, so renaming a folder is one
    // save rather than three.
    let form_open = RwSignal::new(false);
    let form_id = RwSignal::new(None::<String>);
    // Deleting a folder moves its workspaces back to the desk and cannot be
    // undone, so it asks first instead of firing on a tap in arrange mode.
    let delete_open = RwSignal::new(false);
    let delete_id = RwSignal::new(None::<String>);
    let form_label = RwSignal::new(String::new());
    let form_icon = RwSignal::new(String::new());
    let form_color = RwSignal::new(String::new());
    // The grid the server last confirmed, so a save that fails puts the desk
    // back instead of leaving an arrangement that will never come back.
    let confirmed = RwSignal::new(None::<Vec<DeskIcon>>);

    // Persist an arrangement and draw what the server drew from it. Built here,
    // in the component body, so the `Callback`'s value belongs to this owner.
    let save = Callback::new(move |next: Vec<DeskIcon>| {
        let icons = nav.icons;
        let previous = icons.get_untracked();
        confirmed.update(|slot| {
            slot.get_or_insert_with(|| previous.clone());
        });
        let placements = placements_of(&next);
        // Draw it first, so the grid answers the drag that just finished; the
        // server's version then replaces it with the arrangement it accepted.
        icons.set(next);
        drive(async move {
            match desk_session::put_desk_layout(&placements).await {
                Ok(layout) => {
                    icons.set(layout.icons.clone());
                    confirmed.set(Some(layout.icons));
                }
                Err(_) => {
                    if let Some(previous) = confirmed.get_untracked() {
                        icons.set(previous);
                    }
                }
            }
        });
    });

    // One handler for every control on the grid. Each control says what it does
    // in `data-action` and, when it acts on one icon, which icon in the
    // `data-icon` of its tile; the swatches add `data-value`. That is the whole
    // of the dispatch, and it is why no control needs a `Callback` of its own.
    let edit = Callback::new(move |(action, id, value): (String, String, String)| {
        let current = || nav.icons.get_untracked();
        match action.as_str() {
            "new-folder" => {
                let label = tr("desk.desktop.new_folder");
                let (next, id) = add_folder(&current(), &label);
                save.run(next);
                form_id.set(Some(id));
                form_label.set(label);
                form_icon.set(DEFAULT_FOLDER_ICON.to_string());
                form_color.set(String::new());
                form_open.set(true);
            }
            "edit-folder" => {
                let Some(folder) = folder_of(&current(), &id) else {
                    return;
                };
                form_id.set(Some(id));
                form_label.set(folder.label);
                form_icon.set(folder.icon.unwrap_or_default());
                form_color.set(folder.color.unwrap_or_default());
                form_open.set(true);
            }
            // Submitting is the end of the edit, so the form closes on the way
            // out: leaving it open over the grid it just changed would ask the
            // user to dismiss a dialog whose work is already saved.
            "save-folder" => {
                let next = edit_folder(
                    &current(),
                    &id,
                    &form_label.get_untracked(),
                    &form_icon.get_untracked(),
                    &form_color.get_untracked(),
                );
                save.run(next);
                form_open.set(false);
            }
            "delete-folder" => {
                delete_id.set(Some(id));
                delete_open.set(true);
            }
            // Dismissing the form keeps whatever the folder already had: a new
            // folder was added with its default name on open, and renaming was a
            // draft that was never committed.
            "cancel-folder" => form_open.set(false),
            // The form's swatches are drafts, not saves: nothing is stored until
            // the form is submitted.
            "set-icon" => form_icon.set(value),
            "set-color" => form_color.set(value),
            "hide" | "unhide" => save.run(hide_icon(&current(), &id, action == "hide")),
            _ => {}
        }
    });

    view! {
        <div
            data-name="Desktop"
            class="flex h-full flex-col"
            on:click=move |ev: web_sys::MouseEvent| {
                let Some(target) = ev.target() else { return };
                let Ok(target) = target.dyn_into::<web_sys::Element>() else { return };

                // Controls come first: they sit inside a tile, and a tile is also
                // the drop target and the navigation target. The attribute is read
                // off the nearest ancestor carrying it rather than off the clicked
                // element, because the click usually lands on the glyph inside a
                // control — and a button that only works when you hit its padding
                // is a button that works sometimes.
                if let Some(action) = closest_attr(&target, "data-action") {
                    let icon = closest_attr(&target, "data-icon").unwrap_or_default();
                    let value = closest_attr(&target, "data-value").unwrap_or_default();
                    edit.run((action, icon, value));
                    return;
                }

                // The folder form is a dialog, not a tile: a click inside it must
                // not land on the grid behind it and navigate away mid-edit.
                if target.closest("[data-folder-form]").ok().flatten().is_some() {
                    return;
                }

                // A folder opens while arranging too. Arranging inside a folder is
                // the same gesture as arranging on the desk, so refusing to open
                // one in edit mode would lock its contents in place.
                if let Some(id) = closest_attr(&target, "data-folder") {
                    folder_id.set(Some(id));
                    folder_open.set(true);
                    return;
                }

                // While arranging, a click means nothing on a workspace. A drag is
                // the gesture that moves things, and letting a click navigate too
                // would send the user away by accident on every dropped icon.
                if editing.get_untracked() {
                    return;
                }

                if let Some(name) = closest_attr(&target, "data-workspace") {
                    // A workspace picked from inside a folder dialog leaves it
                    // open otherwise, floating over the page it navigated to.
                    if target.closest("[data-folder-scope]").ok().flatten().is_some() {
                        folder_open.set(false);
                    }
                    nav.select.run(name);
                }
            }
            on:dragstart=move |ev: web_sys::DragEvent| {
                let Some(target) = ev.target() else { return };
                let Ok(target) = target.dyn_into::<web_sys::Element>() else { return };
                if let Some(id) = closest_attr(&target, "data-icon") {
                    dragging.set(Some(id));
                }
            }
            on:dragover=move |ev: web_sys::DragEvent| {
                if !editing.get_untracked() || dragging.get_untracked().is_none() {
                    return;
                }
                // Without this the browser refuses the drop and cancels the drag.
                ev.prevent_default();
                let Some(target) = ev.target() else { return };
                let Ok(target) = target.dyn_into::<web_sys::Element>() else { return };
                let next = closest_attr(&target, "data-icon")
                    .map(|id| Seam { id, after: behind(&target, ev.client_x()) });
                // `dragover` fires many times a second: writing the signal only
                // when the target changed keeps the grid from re-rendering on
                // every one of them.
                if over.get_untracked() != next {
                    over.set(next);
                }
            }
            on:drop=move |ev: web_sys::DragEvent| {
                if !editing.get_untracked() {
                    return;
                }
                ev.prevent_default();
                let Some(source) = dragging.get_untracked() else { return };
                let Some(landed) = landed_on(&ev) else { return };
                dragging.set(None);
                over.set(None);
                let icons = nav.icons.get_untracked();
                save.run(move_icon(&icons, &source, &landed));
            }
            on:dragend=move |_| {
                dragging.set(None);
                over.set(None);
            }
        >
            // The desk is a page of icons, not a sidebar: it reads as a grid centred
            // in the window with air around it, rather than a list stretched from
            // edge to edge under the top bar.
            <div class="mx-auto flex w-full max-w-5xl flex-1 flex-col gap-4 p-4">
            <div class="flex items-center justify-between gap-2">
                <h1 class="text-2xl font-semibold tracking-tight text-foreground">{tr("nav.desk")}</h1>
                <button
                    type="button"
                    data-name="DesktopEditToggle"
                    aria-pressed=move || editing.get().to_string()
                    class="inline-flex h-9 max-md:h-10 items-center gap-2 rounded-md border px-3 text-sm transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                    class:ring-2=move || editing.get()
                    class:ring-ring=move || editing.get()
                    on:click=move |_| {
                        if !editing.get_untracked() {
                            // Leaving edit mode abandons a drag that was never
                            // finished, which would otherwise leave an icon
                            // highlighted as the drop target.
                            dragging.set(None);
                            over.set(None);
                        }
                        editing.update(|on| *on = !*on);
                    }
                >
                    {move || {
                        if editing.get() {
                            view! { <Icon name="check" class="size-4" /> }.into_any()
                        } else {
                            view! { <Icon name="pencil" class="size-4" /> }.into_any()
                        }
                    }}
                    {move || if editing.get() { tr("desk.desktop.done") } else { tr("desk.desktop.edit") }}
                </button>
            </div>

            {move || {
                let (shown, hidden): (Vec<DeskIcon>, Vec<DeskIcon>) = nav
                    .icons
                    .get()
                    .into_iter()
                    .partition(|icon| !icon.hidden);
                if shown.is_empty() && hidden.is_empty() {
                    return view! {
                        <p class="py-16 text-center text-sm text-muted-foreground">
                            {tr("desk.desktop.empty")}
                        </p>
                    }
                    .into_any();
                }

                // Hidden workspaces are drawn under the grid while the desk is
                // being arranged, so putting one back is a click on the icon that
                // is already showing. Off edit mode they are nowhere on the desk —
                // the one place they exist is here.
                let arranging = editing.get();
                view! {
                    <div class="flex flex-col gap-6">
                        <IconGrid
                            icons=shown
                            editing=editing.read_only()
                            over=over.read_only()
                        />
                        {if !arranging || hidden.is_empty() {
                            view! { <span></span> }.into_any()
                        } else {
                            view! {
                                <div class="flex flex-col gap-3">
                                    <p class="text-xs font-medium tracking-wide text-muted-foreground uppercase">
                                        {tr("desk.desktop.hidden")}
                                    </p>
                                    <IconGrid
                                        icons=hidden
                                        editing=editing.read_only()
                                        // Nothing lands here: a parked icon
                                        // cannot be dragged, so there is no drop
                                        // target under the pointer to record.
                                        over=RwSignal::new(None).read_only()
                                    />
                                </div>
                            }
                            .into_any()
                        }}
                    </div>
                }
                .into_any()
            }}

            {move || {
                if !editing.get() {
                    return view! { <span></span> }.into_any();
                }
                view! {
                    <button
                        type="button"
                        data-action="new-folder"
                        class="inline-flex h-9 max-md:h-10 w-fit items-center gap-2 rounded-md border border-dashed px-3 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                    >
                        <Icon name="plus" class="size-4" />
                        {tr("desk.desktop.new_folder")}
                    </button>
                }
                .into_any()
            }}
            </div>

            // The folder's dialog is a fixed box: a folder holding two icons and a
            // folder holding twenty open at the same size, so the desk does not
            // jump around as the pointer moves between them. What does not fit
            // scrolls inside that box rather than growing it.
            <Dialog>
                <DialogContent open=folder_open class="flex h-[26rem] max-h-[80dvh] w-[30rem] max-w-[calc(100vw-2rem)] flex-col gap-0 overflow-hidden p-0">
                    {move || {
                        let Some(id) = folder_id.get() else {
                            return view! { <span></span> }.into_any();
                        };
                        let folder = folder_of(&nav.icons.get(), &id);
                        let (label, members) = folder
                            .map(|folder| {
                                // A hidden member is off the desk, so it is off
                                // the folder's dialog too — the dialog is the
                                // folder's contents as the grid draws them.
                                let items =
                                    folder.items.into_iter().filter(|item| !item.hidden).collect();
                                (folder.label, items)
                            })
                            .unwrap_or_default();

                        view! {
                            <div
                                data-folder-scope=id
                                class="flex h-full min-h-0 flex-col gap-0"
                            >
                                <div class="px-6 pt-6 pb-4">
                                    <DialogHeader>
                                        <DialogTitle>{label}</DialogTitle>
                                    </DialogHeader>
                                </div>
                                <div class="min-h-0 flex-1 overflow-y-auto px-6 pb-6">
                                    // The dialog arranges as the desk does, since a
                                    // folder's contents are arranged on the desk
                                    // too — one grid, one edit mode, one gesture.
                                    <IconGrid
                                        icons=members
                                        editing=editing.read_only()
                                        over=over.read_only()
                                    />
                                </div>
                            </div>
                        }
                        .into_any()
                    }}
                </DialogContent>
            </Dialog>

            <Dialog>
                <DialogContent
                    open=delete_open
                    class="max-w-sm"
                    close_on_backdrop_click=true
                    data_name_prefix="AlertDialog"
                >
                    <DialogHeader>
                        <DialogTitle>{tr("desk.folder.delete_title")}</DialogTitle>
                        <DialogDescription>
                            {tr("desk.folder.delete_body")}
                        </DialogDescription>
                    </DialogHeader>
                    <DialogFooter>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Default
                            on:click=move |_| delete_open.set(false)
                        >
                            {tr("common.cancel")}
                        </Button>
                        <Button
                            variant=ButtonVariant::Destructive
                            size=ButtonSize::Default
                            on:click=move |_| {
                                if let Some(id) = delete_id.get_untracked() {
                                    save.run(delete_folder(&nav.icons.get_untracked(), &id));
                                }
                                delete_open.set(false);
                            }
                        >
                            {tr("desk.folder.delete")}
                        </Button>
                    </DialogFooter>
                </DialogContent>
            </Dialog>

            <FolderForm
                open=form_open
                id=form_id
                label=form_label
                icon=form_icon
                color=form_color
            />
        </div>
    }
}

/// The dialog that names a folder and dresses it.
///
/// The form root carries the folder's `data-icon`, so its controls are dispatched
/// like any other and none of them needs to know which folder it is editing. The
/// icon and colour pickers are buttons carrying the value they set, so a pick is
/// data on the element rather than a closure per swatch.
#[component]
fn FolderForm(
    open: RwSignal<bool>,
    id: RwSignal<Option<String>>,
    label: RwSignal<String>,
    icon: RwSignal<String>,
    color: RwSignal<String>,
) -> impl IntoView {
    // `StoredValue` keeps the translator `Copy`, so the swatch labels (built in
    // a per-icon closure) and the static fields can share it.
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));
    let icon_label =
        move |value: &str| i18n.with_value(|i18n: &DeskI18n| folder_icon_label(i18n, value));
    let color_label =
        move |value: &str| i18n.with_value(|i18n: &DeskI18n| folder_color_label(i18n, value));
    view! {
        <Dialog>
            // A fixed width, like the folder dialog: the form is the same three
            // fields for every folder, so it opens at the same size whatever the
            // folder is called instead of resizing with the name being typed.
            <DialogContent open=open show_close_button=false class="w-[26rem] max-w-[calc(100vw-2rem)]">
                <div data-folder-form data-icon=move || id.get().unwrap_or_default()>
                    <DialogHeader>
                        <DialogTitle>{tr("desk.folder.title")}</DialogTitle>
                    </DialogHeader>
                    <div class="flex flex-col gap-4">
                        <div class="flex flex-col gap-1.5">
                            <Label html_for="desk-folder-label">{tr("desk.folder.name")}</Label>
                            <Input id="desk-folder-label" bind_value=label placeholder=tr("desk.folder.name_placeholder") />
                        </div>
                        <div class="flex flex-col gap-1.5">
                            <Label>{tr("desk.folder.icon")}</Label>
                            <div class="flex flex-wrap gap-1">
                                {FOLDER_ICONS
                                    .into_iter()
                                    .map(|(key, name)| view! {
                                        <button
                                            type="button"
                                            data-action="set-icon"
                                            data-value=key
                                            aria-label=icon_label(key)
                                            class="flex size-9 items-center justify-center rounded-md border transition-colors hover:bg-accent max-md:size-10"
                                            class:border-primary=move || icon.get() == key
                                        >
                                            <Icon name=name class="size-4" />
                                        </button>
                                    })
                                    .collect_view()}
                            </div>
                        </div>
                        <div class="flex flex-col gap-1.5">
                            <Label>{tr("desk.folder.color")}</Label>
                            <div class="flex flex-wrap gap-1">
                                {FOLDER_COLORS
                                    .into_iter()
                                    .map(|(key, classes)| view! {
                                        <button
                                            type="button"
                                            data-action="set-color"
                                            data-value=key
                                            aria-label=color_label(key)
                                            class=format!("size-9 rounded-md border-2 max-md:size-10 {classes}")
                                            class:border-ring=move || color.get() == key
                                        />
                                    })
                                    .collect_view()}
                            </div>
                        </div>
                        <div class="flex justify-end gap-2">
                            <Button
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Default
                                attr:data-action="cancel-folder"
                            >
                                {tr("common.cancel")}
                            </Button>
                            <Button
                                variant=ButtonVariant::Default
                                size=ButtonSize::Default
                                attr:data-action="save-folder"
                                attr:disabled=move || label.get().trim().is_empty()
                            >
                                {tr("common.save")}
                            </Button>
                        </div>
                    </div>
                </div>
            </DialogContent>
        </Dialog>
    }
}
