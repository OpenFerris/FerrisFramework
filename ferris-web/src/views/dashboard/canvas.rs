//! The dashboard surface: the 12-column board and the gesture state.
//!
//! The board draws one [`DashboardTile`] per instance with a *keyed* `<For>`
//! (`key = id`): a list edit, a gesture preview or a mode change no longer
//! rebuilds every cell. The old un-keyed map did exactly that, and each
//! rebuild remounted every `WidgetHost` — so confirming a resize re-fetched
//! the whole dashboard. Now the tiles keep themselves placed from the shared
//! signals and this component only owns the state and the board's height.
//!
//! The gesture signals live here (not in each tile) because they are shared:
//! only one box moves at a time, and the whole list is normalized on release.

use leptos::prelude::*;

use ferris_schema::contract::{DashboardStyle, Placement, WidgetInstance};

use crate::ext::UiCtx;

use super::card::{ActiveDrag, DashboardTile};
use super::{model, style};

/// The board: a positioned box, one tile per instance and a height that
/// covers the last row — including the in-flight drop preview, so dragging
/// the last tile down scrolls the board instead of clipping it.
#[component]
pub(super) fn DashboardBoard(
    /// App that owns the dashboard.
    owner: String,
    /// The effective list (source of truth for the grid).
    instances: RwSignal<Vec<WidgetInstance>>,
    /// The board's resolved appearance (global → declared → override): its gap
    /// drives the geometry, its background/padding/radius the frame.
    board: Signal<DashboardStyle>,
    /// Whether the builder is active.
    editing: Signal<bool>,
    /// In-flight placement of a tile being dragged or resized.
    preview: RwSignal<Option<(String, Placement)>>,
    /// The pointer gesture in progress.
    dragging: RwSignal<Option<ActiveDrag>>,
    /// Instance with settings open.
    selected: RwSignal<Option<String>>,
    /// Remove an instance by id.
    on_remove: Callback<String>,
    /// Reload tick for native widgets.
    refresh: Signal<u32>,
) -> impl IntoView {
    let apps = expect_context::<UiCtx>().apps;

    // The gap every consumer shares: the boxes, the board's height and the
    // drag arithmetic in `card.rs` all read it from here.
    let gap = Signal::derive(move || board.get().gap());

    // One pass over the resolved boxes decides how tall the board is: the
    // lowest `row + rows`, or the preview's if a gesture reaches further.
    let board_height = {
        let owner = owner.clone();
        Signal::derive(move || {
            let catalog = apps.get();
            let end_rows = instances
                .get()
                .iter()
                .map(|widget| {
                    let placed = model::placement_of(&catalog, &owner, widget);
                    placed.row + u16::from(placed.rows)
                })
                .max()
                .unwrap_or(0)
                .max(
                    preview
                        .get()
                        .map_or(0, |(_, placed)| placed.row + u16::from(placed.rows)),
                );
            model::board_height(end_rows, gap.get())
        })
    };

    // The frame's appearance lives on the outer element: a padding there would
    // otherwise change what `100%` means for the absolutely positioned tiles,
    // which are placed against the *inner* box on purpose.
    let frame = Signal::derive(move || {
        let vars = style::board_vars(&board.get());
        (!vars.is_empty()).then_some(vars)
    });

    view! {
        <div class="ferris-dashboard-board min-h-0 flex-1" style=frame>
            <div class="relative min-w-0" style=board_height>
                <For
                    each=move || instances.get()
                    key=|widget| widget.id.clone()
                    children=move |widget| {
                        view! {
                            <DashboardTile
                                owner=owner.clone()
                                id=widget.id.clone()
                                instances=instances
                                gap=gap
                                editing=editing
                                preview=preview
                                dragging=dragging
                                selected=selected
                                on_remove=on_remove
                                refresh=refresh
                            />
                        }
                    }
                />
            </div>
        </div>
    }
}
