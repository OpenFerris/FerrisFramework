//! Pointer and DOM helpers for the grid's delegated drag-and-drop.
//!
//! Nothing here keeps state: every answer is read off the element the pointer is
//! on, so a re-render cannot leave a stale target behind. The pure arrangement
//! edits live in [`super::arrange`]; this module only translates a browser event
//! into one of their inputs.

use wasm_bindgen::JsCast;

use super::arrange::Drop;

/// An attribute on this element or the nearest ancestor carrying it, which is
/// what lets a control inside a tile name the tile without knowing about it.
pub(super) fn closest_attr(element: &web_sys::Element, name: &str) -> Option<String> {
    let closest = element.closest(&format!("[{name}]")).ok().flatten()?;
    closest.get_attribute(name)
}

/// This element or the nearest ancestor carrying it.
fn closest_element(element: &web_sys::Element, name: &str) -> Option<web_sys::Element> {
    element.closest(&format!("[{name}]")).ok().flatten()
}

/// Where a drag was let go: which list, in front of which icon, and on which side
/// of it.
///
/// All three are read off the element the drop landed on, in the drop itself,
/// rather than off the icon the last `dragover` happened to name. `dragover` is
/// the browser's own sampling of where the pointer is, tens of times a second,
/// while the drop is the one event that says where the icon was actually
/// released — and the difference between the two is the whole of "it does not
/// matter which one I aim at".
pub(super) fn landed_on(ev: &web_sys::DragEvent) -> Option<Drop> {
    let target = ev.target()?;
    let target: web_sys::Element = target.dyn_into().ok()?;
    let inside = closest_attr(&target, "data-folder-scope");
    let target_id = closest_attr(&target, "data-icon");
    let after = behind(&target, ev.client_x());

    Some(match inside {
        Some(folder) => Drop::within(folder, target_id, after),
        None => Drop::onto(target_id, after),
    })
}

/// The icon a drag is over, and the side of it the icon will land on.
///
/// Drawn as a bar on that side of the tile, because a ring alone says which icon
/// is the target and not where in it: with the two halves of a tile landing in
/// different places, a highlight that looks the same either way would leave the
/// user aiming blind.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Seam {
    pub(super) id: String,
    pub(super) after: bool,
}

/// Whether a drop at `pointer_x` lands behind the tile under the pointer, read
/// against its middle on the horizontal axis — the axis the grid reads along
/// within a row.
///
/// A tile is a seam rather than a slot, which is what a list is: aiming at the
/// right half puts the icon behind, the left half in front. Dropping on empty
/// grid space names no tile, so the icon goes to the end.
pub(super) fn behind(target: &web_sys::Element, pointer_x: i32) -> bool {
    closest_element(target, "data-icon")
        .map(|tile| {
            let rect = tile.get_bounding_client_rect();
            f64::from(pointer_x) > rect.x() + rect.width() / 2.0
        })
        .unwrap_or(false)
}
