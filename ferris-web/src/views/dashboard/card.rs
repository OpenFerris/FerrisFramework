//! The dashboard tile: one grid cell in both faces.
//!
//! [`DashboardTile`] is the *only* cell the grid draws. In view mode it is
//! just the placed box plus the body [`WidgetHost`] resolves; in edit mode
//! the same box grows the chrome (drag, settings, remove) and the resize
//! corner, which write to the grid's gesture signals ([`ActiveDrag`] /
//! `preview`) while the pointer is captured and only confirm on `pointerup`.
//!
//! The tile used to come in two components (`WidgetCell` / `BuilderTile`)
//! that the grid rebuilt wholesale on every list or mode change — which
//! remounted every `WidgetHost` and re-fetched every widget body on, say, a
//! mere resize. Now the grid keeps one tile per id (`<For key=id>`), the box
//! is a derived style signal (geometry changes only re-place the node), and
//! the body is gated by a content digest: moving, resizing or entering edit
//! mode touches no widget body at all.
//!
//! The placement is not a Tailwind class (`col-span-N`), which cannot express
//! "starts at column 3 and measures 4×2": it is [`super::model::tile_style`],
//! the only place in the Desk that writes `style=` and only with geometry.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;

use ferris_schema::contract::layout::{CELL_HEIGHT_PX, COLUMN, MAX_ROWS, Placement};
use ferris_schema::contract::{AppUi, GridSize, WidgetInstance};

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::ext::UiCtx;
use crate::views::widget_host::WidgetHost;

use super::{model, style};

/// The gesture a tile is currently running.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Gesture {
    /// Move: changes `col`/`row` and keeps `span`/`rows`.
    Move,
    /// Resize: changes `span`/`rows` and keeps `col`/`row`.
    Resize,
}

/// A pointer gesture in flight: where the pointer started, how the box
/// was and how big a cell is, to translate pixels into columns/rows.
#[derive(Clone)]
pub(super) struct ActiveDrag {
    /// The instance being manipulated.
    pub(super) id: String,
    /// What the gesture changes.
    pub(super) gesture: Gesture,
    origin_x: f64,
    origin_y: f64,
    /// Placement at the start: the base the delta is added to.
    start: Placement,
    /// Width of one column, in pixels (the tile divided by its `span`).
    cell_w: f64,
    /// Height of one row step, in pixels (the cell plus the board's gap):
    /// captured at gesture start so the arithmetic cannot change under the
    /// pointer when the board's configuration reloads mid-drag.
    row_step: f64,
    /// The definition's minimum: resizing does not go below it.
    min: GridSize,
}

/// One dashboard cell, in both faces.
///
/// The tile owns no list data: it finds itself in `instances` by id, so the
/// grid can hand it the same signals every other tile shares and keep the
/// tile mounted across list edits, mode changes and gestures.
#[component]
pub(super) fn DashboardTile(
    /// App that owns the dashboard: resolves the loose reference and the box.
    owner: String,
    /// The instance's id: the tile's identity (`<For key=id>`), stable across
    /// every list edit.
    id: String,
    /// The effective list: the tile's source of truth for its own instance.
    instances: RwSignal<Vec<WidgetInstance>>,
    /// The board's effective gap, in pixels: part of the box geometry and of
    /// the drag arithmetic.
    gap: Signal<u8>,
    /// Whether the builder chrome is shown.
    editing: Signal<bool>,
    /// The in-flight placement (`preview`), shared with the grid.
    preview: RwSignal<Option<(String, Placement)>>,
    /// The in-flight gesture, shared with the grid.
    dragging: RwSignal<Option<ActiveDrag>>,
    /// Instance whose settings are open (`Some` = dialog open).
    selected: RwSignal<Option<String>>,
    /// Remove the instance by id.
    on_remove: Callback<String>,
    /// Tick that native widgets follow to reload.
    refresh: Signal<u32>,
) -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let apps = ctx.apps;
    let tile_ref = NodeRef::<html::Div>::new();

    // The tile's own appearance: its instance's style over its definition's,
    // resolved by the one function that owns that precedence (`super::style`).
    // It tracks `instances` — editing a style must reach the node — and reads
    // the catalog untracked for the same reason the geometry below does: a
    // catalog change always arrives together with a list change.
    let appearance = {
        let id = id.clone();
        let owner = owner.clone();
        Memo::new(move |_| {
            instances
                .get()
                .iter()
                .find(|widget| widget.id == id)
                .map(|widget| style::widget_style(&apps.get_untracked(), &owner, widget))
                .unwrap_or_default()
        })
    };

    // The tile's style reads `preview` (in flight) or, failing that, its
    // confirmed box: a `Signal::derive` toggles the attribute without
    // recreating the node, which is what lets the CSS transition glide the
    // tile into its released spot while the widget body stays mounted.
    //
    // `instances` is read **tracked**, on purpose. `<For key=id>` keeps this
    // tile mounted across every list edit, so this signal is the *only* thing
    // that can re-place the box, and a derive notifies its observers only
    // through the reads it tracks: with an untracked read the GET landing, a
    // normalize pushing this box or a commit would change the list and the
    // tile would keep drawing its first geometry (regression test below).
    // The catalog stays untracked: sizing only *defaults* from it and any
    // catalog change arrives together with a list change (the load), so
    // tracking it would only clone the whole feed on every drag pixel.
    //
    // The configured appearance rides the same attribute (custom properties,
    // never colours: see `super::style`), so one node update carries both.
    let style = {
        let id = id.clone();
        let owner = owner.clone();
        Signal::derive(move || {
            let list = instances.get();
            let catalog = apps.get_untracked();
            let placed = preview
                .get()
                .filter(|(preview_id, _)| *preview_id == id)
                .map_or_else(
                    || tile_box(&list, &catalog, &owner, &id),
                    |(_, placed)| placed,
                );
            let geometry = model::tile_style(placed, gap.get());
            let appearance = style::tile_vars(&appearance.get());
            if appearance.is_empty() {
                geometry
            } else {
                format!("{geometry} {appearance}")
            }
        })
    };

    // While this tile is the one being dragged its box follows the pointer
    // with no transition (see `.ferris-dashboard-tile[data-dragging]`).
    let drag_active = {
        let id = id.clone();
        Memo::new(move |_| dragging.get().is_some_and(|drag| drag.id == id))
    };

    // What the body *shows* — reference, heading configuration, parameters.
    // Geometry, the preview and the edit chrome are other signals, so a drag, a
    // resize or entering edit mode never rebuilds the `WidgetHost`: the widget
    // keeps its state and its fetched data while the box moves under it. Only a
    // real content change remounts it (the host header is part of that
    // content: a title/subtitle edit must reach the body).
    let body_key = {
        let id = id.clone();
        Memo::new(move |_| {
            instances
                .get()
                .iter()
                .find(|widget| widget.id == id)
                .map(|widget| {
                    (
                        widget.widget.clone(),
                        widget.label.clone(),
                        widget.show_title,
                        widget.subtitle.clone(),
                        widget.show_subtitle,
                        widget.params.clone(),
                    )
                })
        })
    };

    // The gesture start is shared between the move handle and the resize
    // handle, so it captures its own clones: `owner`/`instances` remain
    // available for the rest of the tile and for `commit`.
    let begin = {
        let id = id.clone();
        let owner = owner.clone();
        move |gesture: Gesture, ev: ev::PointerEvent| {
            if let Some(tile) = tile_ref.get() {
                // Capturing on the tile sends the following
                // `pointermove`/`pointerup` here even if the pointer leaves
                // the box.
                let _ = tile.set_pointer_capture(ev.pointer_id());
            }
            ev.prevent_default();
            let start = confirmed_placement(&instances, &apps, &owner, &id);
            let cell_w = cell_width(&tile_ref, start.span);
            let min = instances
                .get_untracked()
                .iter()
                .find(|widget| widget.id == id)
                .map_or_else(
                    // No instance (the tile is being unmounted): the generic
                    // minimum, one cell.
                    || GridSize::resolve("", None, None).min,
                    |widget| model::sizing_of(&apps.get_untracked(), &owner, &widget.widget).min,
                );
            dragging.set(Some(ActiveDrag {
                id: id.clone(),
                gesture,
                origin_x: f64::from(ev.client_x()),
                origin_y: f64::from(ev.client_y()),
                start,
                cell_w,
                row_step: f64::from(CELL_HEIGHT_PX) + f64::from(gap.get_untracked()),
                min,
            }));
            preview.set(None);
        }
    };

    let on_move_down = {
        let begin = begin.clone();
        move |ev: ev::PointerEvent| begin(Gesture::Move, ev)
    };
    let on_resize_down = {
        let begin = begin.clone();
        move |ev: ev::PointerEvent| {
            // The handle is inside the tile: without this the pointerdown
            // would also start a move.
            ev.stop_propagation();
            begin(Gesture::Resize, ev);
        }
    };

    let on_pointer_move = {
        let id = id.clone();
        move |ev: ev::PointerEvent| {
            let Some(drag) = dragging.get_untracked() else {
                return;
            };
            if drag.id != id {
                return;
            }
            let dcols = ((f64::from(ev.client_x()) - drag.origin_x) / drag.cell_w).round() as i32;
            let drows = ((f64::from(ev.client_y()) - drag.origin_y) / drag.row_step).round() as i32;
            let placed = match drag.gesture {
                Gesture::Move => Placement {
                    col: (i32::from(drag.start.col) + dcols)
                        .clamp(0, i32::from(COLUMN.saturating_sub(drag.start.span)))
                        as u8,
                    row: (i32::from(drag.start.row) + drows).max(0) as u16,
                    ..drag.start
                },
                Gesture::Resize => {
                    // The column cap widens up to the minimum if the box is
                    // stuck to the edge: `placement_of` will re-place the
                    // column if it does not fit, instead of leaving an
                    // inverted `clamp`.
                    let span_hi = model::max_span_from(drag.start.col).max(drag.min.span);
                    Placement {
                        span: (i32::from(drag.start.span) + dcols)
                            .clamp(i32::from(drag.min.span), i32::from(span_hi))
                            as u8,
                        rows: (i32::from(drag.start.rows) + drows)
                            .clamp(i32::from(drag.min.rows), i32::from(MAX_ROWS))
                            as u8,
                        ..drag.start
                    }
                }
            };
            preview.set(Some((id.clone(), placed)));
        }
    };

    let on_pointer_up = {
        let id = id.clone();
        let owner = owner.clone();
        move |ev: ev::PointerEvent| {
            if let Some(tile) = tile_ref.get() {
                let _ = tile.release_pointer_capture(ev.pointer_id());
            }
            let Some(drag) = dragging.get_untracked() else {
                return;
            };
            if drag.id != id {
                return;
            }
            dragging.set(None);
            let Some((_, placed)) = preview
                .get_untracked()
                .filter(|(preview_id, _)| *preview_id == id)
            else {
                // Released without moving: there is nothing to confirm.
                preview.set(None);
                return;
            };
            preview.set(None);
            commit(&instances, &apps, &owner, &id, placed);
        }
    };

    let open_props = {
        let id = id.clone();
        move |_| selected.set(Some(id.clone()))
    };
    let remove = {
        let id = id.clone();
        move |_| on_remove.run(id.clone())
    };

    let reference_for_chrome = {
        let id = id.clone();
        Memo::new(move |_| {
            instances
                .get()
                .iter()
                .find(|widget| widget.id == id)
                .map_or_else(String::new, |widget| widget.widget.clone())
        })
    };

    let body_owner = owner.clone();
    let body_id = id.clone();

    view! {
        <div
            node_ref=tile_ref
            // `ferris-dashboard-tile` is `catalog::dashboard::DASHBOARD_TILE`
            // (`ferris-web-ui`), whose rule in `components.css` owns the tile's
            // motion and its configured appearance (`--ferris-tile-*`); the
            // rest here is structure. `absolute` is not optional: the box from
            // `model::tile_style` (`left`/`top`/`width`/`height` in `calc()`)
            // only places a tile against the board's inner box when the tile is
            // out of flow — as `relative` it would keep stacking in normal flow
            // and just shift sideways from its static position, which is exactly
            // how the grid ends up misplaced.
            class="ferris-dashboard-tile group/tile absolute flex min-w-0 flex-col overflow-hidden"
            // The edit chrome is a class, not a handful of colour utilities:
            // `components.css` only removes the top edge and its corners (the
            // seam for the header) and adds no surface, so a styled tile keeps
            // exactly the look it has in read mode while it is being moved.
            class=(
                ferris_web_ui::catalog::dashboard::DASHBOARD_TILE_EDITING,
                move || editing.get(),
            )
            style=style
            data-dragging=move || drag_active.get().then_some("true")
            // A tile that declares a background, border or shadow owns the
            // surface: any `.ferris-card` a body still renders must not paint a
            // second one over it (see `.ferris-dashboard-tile[data-surface]` in
            // `components.css`). The host's own bodies no longer use `Card`:
            // the tile is the card and `WidgetHost` paints the heading.
            data-surface=move || style::owns_surface(&appearance.get()).then_some("true")
            on:pointermove=on_pointer_move
            on:pointerup=on_pointer_up
        >
            {move || {
                // The handlers are cloned per run: the inner closure owns
                // its copies, so this render closure stays `FnMut`.
                let on_move_down = on_move_down.clone();
                let open_props = open_props.clone();
                let remove = remove.clone();
                editing.get().then(move || {
                    view! {
                        // `ferris-dashboard-tile__chrome` owns the strip's
                        // colour and rule (`components.css`); the utilities are
                        // structure only.
                        <div class="ferris-dashboard-tile__chrome flex items-center gap-0.5 border-b px-1 py-0.5">
                            <span
                                class="flex cursor-grab touch-none items-center px-0.5 text-muted-foreground active:cursor-grabbing"
                                on:pointerdown=on_move_down
                                aria-hidden="true"
                            >
                                <icons::GripVertical class="size-3.5" />
                            </span>
                            <span class="min-w-0 flex-1 truncate px-1 text-[0.7rem] text-muted-foreground">
                                <code>{reference_for_chrome.get()}</code>
                            </span>
                            <Button
                                size=ButtonSize::IconXs
                                variant=ButtonVariant::Ghost
                                on:click=open_props
                                attr:aria-label="Ajustes del widget"
                                attr:title="Ajustes"
                            >
                                <icons::Settings class="size-3.5" />
                            </Button>
                            <Button
                                size=ButtonSize::IconXs
                                variant=ButtonVariant::Ghost
                                on:click=remove
                                attr:aria-label="Quitar widget"
                                attr:title="Quitar"
                            >
                                <icons::Trash2 class="size-3.5" />
                            </Button>
                        </div>
                    }
                })
            }}
            <div class="min-h-0 min-w-0 flex-1 overflow-hidden">
                {move || {
                    let Some((reference, label, show_title, subtitle, show_subtitle, params)) =
                        body_key.get()
                    else {
                        // The tile outlived its instance by one frame
                        // (`<For>` unmounts it on the same change): draw an
                        // empty box rather than a stale widget.
                        return view! { <div class="h-full min-w-0"></div> }.into_any();
                    };
                    view! {
                        <WidgetHost
                            default_owner=body_owner.clone()
                            reference=reference
                            instance_id=body_id.clone()
                            label=label
                            show_title=show_title
                            subtitle=subtitle
                            show_subtitle=show_subtitle
                            params=params
                            refresh=Some(refresh)
                        />
                    }
                    .into_any()
                }}
            </div>
            {move || {
                let on_resize_down = on_resize_down.clone();
                editing.get().then(move || {
                    view! {
                        // `ferris-dashboard-tile__resize` owns the handle's
                        // surface and colour; the rest is placement/structure.
                        <button
                            type="button"
                            class="ferris-dashboard-tile__resize absolute right-0 bottom-0 size-4 cursor-nwse-resize touch-none rounded-tl-sm text-muted-foreground opacity-0 transition-opacity group-hover/tile:opacity-100 focus-visible:opacity-100"
                            on:pointerdown=on_resize_down
                            aria-label="Redimensionar widget"
                        >
                            <icons::Maximize2 class="size-3" />
                        </button>
                    }
                })
            }}
        </div>
    }
}

/// The tile's confirmed box right now: the instance's own placement resolved
/// against the catalog. The in-flight `preview` is deliberately *not* part of
/// it — the preview is the box on its way, this is the one that was committed.
///
/// Untracked on purpose: this runs inside pointer handlers, where there is no
/// observer to subscribe. The reactive path (the tile's `style`) tracks
/// `instances` itself — see its comment.
fn confirmed_placement(
    instances: &RwSignal<Vec<WidgetInstance>>,
    apps: &RwSignal<Vec<AppUi>>,
    owner: &str,
    id: &str,
) -> Placement {
    let list = instances.get_untracked();
    let catalog = apps.get_untracked();
    tile_box(&list, &catalog, owner, id)
}

/// The committed box of the instance `id` inside an already-read list: its
/// declared placement resolved against the catalog, or one cell when the
/// instance is already gone (`<For>` unmounts it in the same change, so this
/// box never really shows).
fn tile_box(list: &[WidgetInstance], catalog: &[AppUi], owner: &str, id: &str) -> Placement {
    list.iter().find(|widget| widget.id == id).map_or_else(
        || Placement {
            col: 0,
            row: 0,
            span: 1,
            rows: 1,
        },
        |widget| model::placement_of(catalog, owner, widget),
    )
}

/// Width of a column in pixels: the tile's rect divided by its `span`.
fn cell_width(node: &NodeRef<html::Div>, span: u8) -> f64 {
    node.get()
        .map(|tile| tile.get_bounding_client_rect().width() / f64::from(span.max(1)))
        .filter(|width| *width > 1.0)
        .unwrap_or(64.0)
}

/// Confirms a gesture: writes the released box, moves the instance to the front
/// so it keeps the chosen cell, and normalizes the rest.
///
/// Moving it to the front is the "what I move pushes the others" semantics (not
/// "the moving one bounces to where it fits"): [`ferris_schema::contract::layout::normalize`]
/// places in order, so the first in the list keeps its cell and those that
/// overlap go to the first free slot.
fn commit(
    instances: &RwSignal<Vec<WidgetInstance>>,
    apps: &RwSignal<Vec<AppUi>>,
    owner: &str,
    id: &str,
    placed: Placement,
) {
    let catalog = apps.get_untracked();
    instances.update(|list| {
        if let Some(widget) = list.iter_mut().find(|widget| widget.id == id) {
            widget.col = Some(placed.col);
            widget.row = Some(placed.row);
            widget.span = Some(placed.span);
            widget.rows = Some(placed.rows);
        }
        if let Some(position) = list.iter().position(|widget| widget.id == id) {
            let moved = list.remove(position);
            list.insert(0, moved);
        }
        model::normalize(&catalog, owner, list);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use leptos::prelude::Owner;

    /// Regression: the tile's box must re-place when the *list* changes.
    ///
    /// `<For key=id>` keeps a tile mounted across list edits, so its `style`
    /// derive is the only thing that can move it — and a derive notifies its
    /// observers only through the reads it tracks. Reading `instances` with
    /// `get_untracked` (the shape this test mirrors) leaves the box frozen at
    /// its first placement: the layout `GET` landing, a normalize pushing the
    /// box or a commit would change the list and the tile would keep drawing
    /// the geometry it first resolved — every widget of a cell-less seed
    /// piled at `(0, 0)`.
    #[test]
    fn the_style_derive_replaces_the_box_when_the_list_changes() {
        let owner = Owner::new();
        owner.set();

        let instances = RwSignal::new(vec![WidgetInstance {
            id: "a".to_string(),
            widget: "host.stat".to_string(),
            ..Default::default()
        }]);
        let apps = RwSignal::new(Vec::<AppUi>::new());

        // The same shape `DashboardTile`'s `style` has: the list tracked, the
        // catalog untracked, no preview in flight.
        let style = {
            let owner = String::new();
            Signal::derive(move || {
                let list = instances.get();
                model::tile_style(tile_box(&list, &apps.get_untracked(), &owner, "a"), 12)
            })
        };

        // The DOM attribute behaves like a memo over the derive: it caches the
        // box and only recomputes through the dependencies the derive tracks.
        let drawn = Memo::new(move |_| style.get());

        assert!(
            drawn.get().contains("top: calc(0 * 84px)"),
            "first placement: {}",
            drawn.get()
        );

        instances.update(|list| list[0].row = Some(2));
        assert!(
            drawn.get().contains("top: calc(2 * 84px)"),
            "the tile must follow the list: {}",
            drawn.get()
        );
    }

    /// An instance that is already gone collapses to one cell instead of
    /// panicking: `<For>` removes the node in the same change.
    #[test]
    fn a_missing_instance_collapses_to_one_cell() {
        let list = Vec::new();
        assert_eq!(
            tile_box(&list, &[], "host", "gone"),
            Placement {
                col: 0,
                row: 0,
                span: 1,
                rows: 1,
            }
        );
    }
}
