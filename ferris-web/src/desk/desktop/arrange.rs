//! The arrangement itself.
//!
//! These functions are the whole of the grid's behaviour: each takes a grid and
//! answers with the next one. That is what makes the drag, the form and the hide
//! button the same amount of code, and what makes the interesting cases testable
//! without a browser.

use crate::desk::session::{DeskIcon, DeskIconPlacement};

use super::tiles::DEFAULT_FOLDER_ICON;

/// The grid as the server stores it: one placement per icon, in the order it is
/// drawn.
///
/// Hidden icons are sent, not dropped: they are off the grid but still named by
/// the arrangement, which is what keeps a hidden workspace one click from coming
/// back instead of gone for good. One is sent as a workspace placement, whether
/// it was in a folder or at the root — a folder's `items` are names only, with
/// nowhere to record that one of them is hidden, and the alternative (keeping it
/// in the folder and losing the flag) would silently un-hide it on the next load.
///
/// So hiding does not promise to restore where the icon was. It promises to take
/// it off the grid and keep it reachable; where it comes back to is the root,
/// which is the one place every arrangement can name it from.
pub(super) fn placements_of(icons: &[DeskIcon]) -> Vec<DeskIconPlacement> {
    let mut out: Vec<DeskIconPlacement> = Vec::with_capacity(icons.len());

    for icon in icons {
        if icon.is_folder() {
            out.push(DeskIconPlacement {
                folder: icon.id.clone(),
                label: icon.label.clone(),
                icon: icon.icon.clone().unwrap_or_default(),
                color: icon.color.clone().unwrap_or_default(),
                items: icon
                    .items
                    .iter()
                    .filter(|item| !item.hidden)
                    .map(|item| item.id.clone())
                    .collect(),
                ..Default::default()
            });
            continue;
        }
        out.push(DeskIconPlacement {
            workspace: icon.id.clone(),
            hidden: icon.hidden,
            ..Default::default()
        });
    }

    // A workspace hidden inside a folder was left out of that folder's items, so
    // it is named here instead — as its own placement, since that is the only
    // place the hidden flag can live.
    for icon in icons {
        if !icon.is_folder() {
            continue;
        }
        for member in icon.items.iter().filter(|member| member.hidden) {
            out.push(DeskIconPlacement {
                workspace: member.id.clone(),
                hidden: true,
                ..Default::default()
            });
        }
    }
    out
}

/// The folder with this id, if the grid has one.
pub(super) fn folder_of(icons: &[DeskIcon], id: &str) -> Option<DeskIcon> {
    icons
        .iter()
        .find(|icon| icon.id == id && icon.is_folder())
        .cloned()
}

/// Pull the icon `dragged` out of wherever it is, and answer with it plus the
/// grid without it.
///
/// Out of a folder too, which is why this walks the folder members rather than
/// only the root: a workspace is one icon wherever the user filed it.
fn take_icon(icons: &[DeskIcon], dragged: &str) -> Option<(DeskIcon, Vec<DeskIcon>)> {
    let mut taken = None;
    let mut rest = Vec::with_capacity(icons.len());

    for icon in icons {
        if icon.id == dragged {
            taken = Some(icon.clone());
            continue;
        }
        if icon.is_folder() {
            match take_icon(&icon.items, dragged) {
                Some((found, items)) => {
                    taken = Some(found);
                    rest.push(DeskIcon {
                        items,
                        ..icon.clone()
                    });
                }
                None => rest.push(icon.clone()),
            }
            continue;
        }
        rest.push(icon.clone());
    }

    taken.map(|taken| (taken, rest))
}

/// Where a dropped icon belongs: which list, in front of what, and on which side.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Drop {
    /// The folder whose dialog the icon landed in. `None` is the desk itself.
    inside: Option<String>,
    /// The icon the pointer was over, if it was over one.
    target: Option<String>,
    /// Behind `target` rather than in front of it.
    after: bool,
}

impl Drop {
    /// A drop in a list, relative to the icon it landed on.
    pub(super) fn onto(target: Option<String>, after: bool) -> Self {
        Drop {
            inside: None,
            target,
            after,
        }
    }

    /// A drop inside `folder`, relative to the icon it landed on.
    pub(super) fn within(folder: impl Into<String>, target: Option<String>, after: bool) -> Self {
        Drop {
            inside: Some(folder.into()),
            target,
            after,
        }
    }
}

/// Move `dragged` where the drop says it goes.
///
/// The drop is read as one thing — a list, an icon, and a side of it — because
/// reading them separately is what made the target stop mattering: the side of
/// the tile the pointer was on decided nothing, so every drop on the same icon
/// produced the same grid, and the user's aim was worth nothing.
///
///   - `inside` names the folder whose dialog the icon landed in, so the drop
///     reorders within one folder instead of across the desk. A drop on the
///     dialog's backdrop lands in no folder, which is how an icon leaves a
///     folder without closing it first.
///   - `target` is whatever the pointer was over: a folder files the icon, a
///     workspace puts it in order, and no icon at all means the end of the list.
///   - `after` picks the side of `target` the pointer was on, so the tile reads
///     as a seam rather than as a slot — aiming at the right half of an icon
///     puts the icon behind it, which is what a reorder in any list means.
///
/// A folder is a list of workspaces by construction, so a folder dropped on
/// another one is placed next to it rather than filed inside: there is no folder
/// in a folder, and the arrangement would be dropped by the server on the next
/// save, taking the folder with it.
///
/// Dropping an icon on itself, or on a target that is gone, still runs: it costs
/// a save and leaves the grid in the order the user pointed at, which is simpler
/// than guessing which drops were meant as no-ops.
pub(super) fn move_icon(icons: &[DeskIcon], dragged: &str, drop: &Drop) -> Vec<DeskIcon> {
    let Some((moved, mut next)) = take_icon(icons, dragged) else {
        return icons.to_vec();
    };
    let target = drop.target.as_deref().filter(|target| *target != dragged);

    if let Some(folder) = drop.inside.as_deref() {
        return match next
            .iter_mut()
            .find(|icon| icon.id == folder && icon.is_folder())
        {
            // A folder holds workspaces only, so inside one there is no filing to
            // do: the drop either puts the icon in order or at the end.
            Some(folder) => {
                insert_at(&mut folder.items, moved, target, drop.after);
                next
            }
            None => {
                next.push(moved);
                next
            }
        };
    }

    let Some(target) = target else {
        next.push(moved);
        return next;
    };

    if let Some(at) = next
        .iter()
        .position(|icon| icon.id == target && icon.is_folder())
    {
        if moved.is_folder() {
            insert_at(&mut next, moved, Some(target), drop.after);
            return next;
        }
        next[at].items.push(moved);
        return next;
    }

    insert_at(&mut next, moved, Some(target), drop.after);
    next
}

/// Put `moved` in front of or behind the icon named by `target`, or at the end
/// when there is no target — the three outcomes of a drop inside one list.
fn insert_at(items: &mut Vec<DeskIcon>, moved: DeskIcon, target: Option<&str>, after: bool) {
    match target.and_then(|target| items.iter().position(|icon| icon.id == target)) {
        Some(at) => items.insert(if after { at + 1 } else { at }, moved),
        None => items.push(moved),
    }
}

/// Rename a folder and dress it, which is what the folder form submits.
///
/// Label, icon and colour are one action because the form edits a folder in
/// place: three saves for three fields would send three requests and show three
/// intermediate grids for what the user did as one edit.
///
/// An empty field is ignored rather than erasing what the folder has, so
/// clearing the label to retype it cannot leave the folder nameless on the grid.
pub(super) fn edit_folder(
    icons: &[DeskIcon],
    id: &str,
    label: &str,
    icon: &str,
    color: &str,
) -> Vec<DeskIcon> {
    icons
        .iter()
        .map(|found| {
            if found.id != id {
                return found.clone();
            }
            let mut folder = found.clone();
            let label = label.trim();
            if !label.is_empty() {
                folder.label = label.to_string();
            }
            if !icon.trim().is_empty() {
                folder.icon = Some(icon.trim().to_string());
            }
            folder.color = Some(color.trim().to_string()).filter(|color| !color.is_empty());
            folder
        })
        .collect()
}

/// Delete a folder. Its members go back to the root rather than with it: a
/// workspace the user can no longer reach is worse than one they have to move.
///
/// Any folder can be deleted, which keeps this a plain edit; the members simply
/// end up ungrouped and can be filed somewhere else.
pub(super) fn delete_folder(icons: &[DeskIcon], id: &str) -> Vec<DeskIcon> {
    let mut out = Vec::with_capacity(icons.len());
    for icon in icons {
        if icon.id == id {
            out.extend(icon.items.iter().cloned());
            continue;
        }
        out.push(icon.clone());
    }
    out
}

/// Whether `id` names a workspace filed inside one of these folders.
fn filed_in(icons: &[DeskIcon], id: &str) -> bool {
    icons.iter().any(|icon| {
        icon.is_folder()
            && (icon.items.iter().any(|item| item.id == id) || filed_in(&icon.items, id))
    })
}

/// Keep a workspace off the grid, or bring it back.
///
/// Hiding undeclares nothing: the workspace stays in the arrangement and in the
/// sidebar, so unhiding is a click away and a save puts it back.
///
/// Hiding a workspace that was filed in a folder takes it out of that folder and
/// parks it at the root. The row of hidden icons under the grid is the one place
/// a hidden workspace is drawn, and a folder's members are not: a workspace left
/// filed away would be off the desk, off its folder's dialog, and not in the row
/// either — hidden with no way back, which is the opposite of what hiding is for.
/// It also has to leave, because a folder names its members as a plain list of
/// names with nowhere to record that one of them is hidden.
pub(super) fn hide_icon(icons: &[DeskIcon], id: &str, hidden: bool) -> Vec<DeskIcon> {
    if !filed_in(icons, id) {
        return icons
            .iter()
            .map(|icon| {
                if icon.id == id && !icon.is_folder() {
                    DeskIcon {
                        hidden,
                        ..icon.clone()
                    }
                } else {
                    icon.clone()
                }
            })
            .collect();
    }

    let Some((mut icon, mut next)) = take_icon(icons, id) else {
        return icons.to_vec();
    };
    icon.hidden = hidden;
    // At the end either way: a workspace that was filed comes back where the
    // folder left room for it, and the row of hidden icons reads last.
    next.push(icon);
    next
}

/// An id no icon is using, for a folder that does not exist yet.
pub(super) fn new_folder_id(icons: &[DeskIcon]) -> String {
    let mut n = icons.len() + 1;
    loop {
        let candidate = format!("folder-{n}");
        if !icons.iter().any(|icon| icon.id == candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// The grid with an empty folder added at the end, ready to be named, and the id
/// it was given. `label` is the default name the desk's catalogue supplies; the
/// user replaces it in the form before the save leaves the browser.
pub(super) fn add_folder(icons: &[DeskIcon], label: &str) -> (Vec<DeskIcon>, String) {
    let id = new_folder_id(icons);
    let mut next = icons.to_vec();
    next.push(DeskIcon {
        id: id.clone(),
        kind: "folder".to_string(),
        label: label.to_string(),
        icon: Some(DEFAULT_FOLDER_ICON.to_string()),
        color: None,
        items: Vec::new(),
        hidden: false,
    });
    (next, id)
}
