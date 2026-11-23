//! One row of the list table.
//!
//! Extracted from `table.rs` so the table keeps the viewport/scroll concerns and
//! this file keeps the per-document rendering. The row is the largest click
//! target, but the controls inside it keep their own behaviour and the title
//! link stays the keyboard path.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::components::A;
use serde_json::Value;

use super::{interactive_target, stamp_age};
use crate::api::meta::{Column, display_value, docstatus_rank, record_name};
use crate::api::types::EngagementPage;
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::icon::Icon;
use crate::components::ui::table as t;
use crate::components::views::user_identity::UserName;
use crate::ext::{SLOT_LIST_ROW_ACTIONS, UiSlot};
use crate::i18n::{t, use_i18n};

#[component]
pub(super) fn ListRow(
    row: Value,
    #[prop(into)] visible_columns: Signal<Vec<Column>>,
    #[prop(into)] title_field_name: Signal<String>,
    #[prop(into)] module: Signal<String>,
    #[prop(into)] doctype: Signal<String>,
    #[prop(into)] selected: Signal<HashSet<String>>,
    #[prop(into)] engagement: Signal<EngagementPage>,
    #[prop(into)] pending_like: Signal<bool>,
    row_actions_declared: bool,
    on_select_row: Callback<(String, bool)>,
    on_toggle_like: Callback<String>,
    navigate: Callback<(String, NavigateOptions)>,
) -> impl IntoView {
    let i18n = use_i18n();
    let ds = row.get("docstatus").map(docstatus_rank).unwrap_or(0);
    let name = record_name(&row);
    // Derived, not captured: the router reuses this view when only the route
    // params change, and a row that kept a plain `String` would keep linking to
    // the previous doctype under the new URL.
    let url = {
        let name = name.clone();
        Signal::derive(move || {
            // Every segment is encoded: doctypes and document names may contain
            // spaces or `#`/`?`, and the form route reads them as path params.
            format!(
                "/desk/{}/{}/{}",
                crate::desk::session::path_segment(&module.get()),
                crate::desk::session::path_segment(&doctype.get()),
                crate::desk::session::path_segment(&name)
            )
        })
    };
    let (age, exact) = stamp_age("modified", &row, &i18n);

    // The row is the largest click target, but the controls inside it keep
    // their own behaviour and the title link stays the keyboard path. Enter on
    // a focused row opens it too.
    let on_row_click = move |ev: leptos::ev::MouseEvent| {
        if ev.ctrl_key()
            || ev.meta_key()
            || ev.shift_key()
            || ev.alt_key()
            || interactive_target(ev.target())
        {
            return;
        }
        navigate.run((url.get_untracked(), NavigateOptions::default()));
    };
    let on_row_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() != "Enter" || interactive_target(ev.target()) {
            return;
        }
        ev.prevent_default();
        navigate.run((url.get_untracked(), NavigateOptions::default()));
    };
    let row_label = i18n.t_with("components.list.row.open", &[("name", &name)]);

    let name_for_check = name.clone();
    let row_id = name.clone();
    let checked_row = Signal::derive(move || selected.get().contains(&name_for_check));
    let docname_of = name.clone();
    let docname = Signal::derive(move || docname_of.clone());
    let liked = Signal::derive(move || engagement.get().is_liked(&docname.get()));
    let toggle_name = name.clone();
    let toggle_row = move |on: bool| {
        on_select_row.run((toggle_name.clone(), on));
    };

    // Cells subscribe to the visible columns themselves; the table only has to
    // pass the signal once. Reusing the row value avoids cloning the document
    // for every column.
    let cells = {
        let row = row.clone();
        move || {
            let tf = title_field_name.get();
            visible_columns.with(|cols| {
                cols.iter()
                    .map(|c| {
                        let cell_class = "max-w-[120px] truncate sm:max-w-[200px] md:max-w-[300px]";

                        // Timestamps read as an age with the exact date on
                        // hover: the stored RFC 3339 stamp is unreadable in a
                        // dense list.
                        if c.fieldtype == "Datetime" {
                            let (age, exact) = stamp_age(&c.fieldname, &row, &i18n);
                            return view! {
                                <t::TableCell class=cell_class>
                                    <span title={exact}>{age}</span>
                                </t::TableCell>
                            }
                            .into_any();
                        }

                        let val = display_value(
                            &c.fieldtype,
                            row.get(&c.fieldname).unwrap_or(&Value::Null),
                        );
                        // A user column shows the person, never the id: the same
                        // rule the form's provenance rows follow.
                        if crate::api::users::is_user_field(&c.fieldname) && !val.is_empty() {
                            let user = val;
                            return view! {
                                <t::TableCell class=cell_class>
                                    <UserName id=Signal::derive(move || user.clone()) />
                                </t::TableCell>
                            }
                            .into_any();
                        }

                        if c.fieldname == tf && c.fieldname != "name" {
                            let href = url.get();
                            view! {
                                <t::TableCell class=format!("{} font-medium", cell_class)>
                                    <A href=href attr:class="text-primary hover:underline">
                                        {val}
                                    </A>
                                </t::TableCell>
                            }
                            .into_any()
                        } else {
                            view! { <t::TableCell class=cell_class>{val}</t::TableCell> }.into_any()
                        }
                    })
                    .collect_view()
            })
        }
    };

    // Fase 5: one plugin mount per row; the record's name travels as
    // `data-row`. Built eagerly here (not inside a `<Show>`, whose children
    // closure must be `Fn` and so rejects moving `row_id`). The row document is
    // owned (R-7) so a `$field` prop can be projected from it.
    let row_data = row.clone();
    let favorite_i18n = i18n;
    let actions_cell = row_actions_declared.then(move || {
        view! {
            <t::TableCell attr:data-row-action="true">
                <UiSlot
                    slot_name=SLOT_LIST_ROW_ACTIONS
                    doctype=doctype
                    row=row_id.clone()
                    row_data=row_data
                />
            </t::TableCell>
        }
        .into_any()
    });

    view! {
        <t::TableRow
            // A phone row is the primary navigation target: 36px is below the
            // 40px a finger needs, so it grows to 48px below `md` only.
            class="h-9 max-md:h-12 cursor-pointer outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:ring-inset"
            attr:tabindex="0"
            attr:aria-label=row_label
            attr:data-row-url=move || url.get()
            attr:data-state=move || if checked_row.get() { "selected" } else { "unselected" }
            on:click=on_row_click
            on:keydown=on_row_keydown
        >
            <t::TableCell class="w-[40px]">
                <Checkbox
                    aria_label=i18n.t_with("components.list.row.select", &[("name", &name)])
                    checked=checked_row
                    on_checked_change=Callback::new(toggle_row)
                />
            </t::TableCell>
            {cells}
            <t::TableCell class="w-[100px] whitespace-nowrap">
                <Badge variant=match ds {
                    1 => BadgeVariant::Success,
                    2 => BadgeVariant::Destructive,
                    _ => BadgeVariant::Secondary,
                }>
                    {match ds {
                        1 => t("components.list.status.submitted"),
                        2 => t("components.list.status.cancelled"),
                        _ => t("components.list.status.draft"),
                    }}
                </Badge>
            </t::TableCell>

            // Edad · comentarios · likes, en un solo flex.
            <t::TableCell class="w-[180px] whitespace-nowrap">
                <div class="flex items-center justify-end gap-3">
                    <span class="text-xs tabular-nums text-muted-foreground" title={exact}>
                        {age}
                    </span>

                    <span class=move || {
                        if engagement.get().comment_count(&docname.get()) > 0 {
                            "inline-flex items-center gap-1 text-xs tabular-nums text-muted-foreground"
                        } else {
                            "inline-flex items-center gap-1 text-xs tabular-nums text-muted-foreground/50"
                        }
                    }>
                        <Icon name="MessageSquare" class="size-3.5" />
                        <span>
                            {move || engagement.get().comment_count(&docname.get()).to_string()}
                        </span>
                    </span>

                    <button
                        type="button"
                        class=move || {
                            let mut classes = String::from(
                                // `max-md:size-10`: the heart is an icon-only
                                // control and its hit box was the glyph itself
                                // (~14px) on a phone.
                                "inline-flex items-center gap-1 rounded-md text-xs tabular-nums outline-none transition-colors focus-visible:ring-1 focus-visible:ring-ring max-md:size-10 max-md:justify-center",
                            );
                            classes.push_str(if liked.get() {
                                " text-destructive"
                            } else {
                                " text-muted-foreground hover:text-destructive"
                            });
                            if pending_like.get() {
                                classes.push_str(" opacity-50");
                            }
                            classes
                        }
                        aria-pressed=move || liked.get().to_string()
                        aria-label=move || {
                            let name = docname.get();
                            if liked.get() {
                                favorite_i18n
                                    .t_with("components.list.row.unfavorite", &[("name", &name)])
                            } else {
                                favorite_i18n
                                    .t_with("components.list.row.favorite", &[("name", &name)])
                            }
                        }
                        disabled=move || pending_like.get()
                        on:click=move |_| on_toggle_like.run(docname.get())
                    >
                        <Icon
                            name="Heart"
                            class=move || {
                                if liked.get() {
                                    "size-3.5 fill-current"
                                } else {
                                    "size-3.5"
                                }
                            }
                        />
                    </button>
                </div>
            </t::TableCell>
            {actions_cell}
        </t::TableRow>
    }
}
