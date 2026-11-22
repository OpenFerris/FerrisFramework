//! Arrangement cases: the pure grid edits, exercised without a browser.

use super::arrange::*;
use crate::desk::session::DeskIcon;

fn workspace(name: &str) -> DeskIcon {
    DeskIcon {
        id: name.into(),
        kind: "workspace".into(),
        label: name.into(),
        ..Default::default()
    }
}

fn folder_with(id: &str, members: &[&str]) -> DeskIcon {
    DeskIcon {
        items: members.iter().map(|member| workspace(member)).collect(),
        ..folder_icon(id)
    }
}

fn folder_icon(id: &str) -> DeskIcon {
    DeskIcon {
        id: id.into(),
        kind: "folder".into(),
        label: id.into(),
        icon: Some("Folder".into()),
        ..Default::default()
    }
}

/// The grid as the server sends it: two workspaces at the root and a folder
/// holding two more.
fn seed() -> Vec<DeskIcon> {
    vec![
        workspace("stock"),
        workspace("billing"),
        folder_with("ops", &["sales", "tasks"]),
    ]
}

fn ids(icons: &[DeskIcon]) -> Vec<&str> {
    icons.iter().map(|icon| icon.id.as_str()).collect()
}

/// The root's workspaces, in order — a folder is not one of them.
fn root(icons: &[DeskIcon]) -> Vec<&str> {
    icons
        .iter()
        .filter(|icon| !icon.is_folder())
        .map(|icon| icon.id.as_str())
        .collect()
}

fn members_of(icons: &[DeskIcon], id: &str) -> Vec<String> {
    let folder = folder_of(icons, id).unwrap_or_else(|| panic!("folder {id}"));
    folder.items.iter().map(|icon| icon.id.clone()).collect()
}

/// Dropping on a workspace puts the icon on the side the pointer was on. This
/// is the fix for a grid where the aim did not matter: the tile used to be one
/// slot, so aiming anywhere on it produced the same arrangement and the
/// pointer's position carried nothing.
#[test]
fn a_tile_is_a_seam_and_the_pointer_chooses_the_side() {
    let before = move_icon(&seed(), "billing", &Drop::onto(Some("stock".into()), false));
    let after = move_icon(&seed(), "billing", &Drop::onto(Some("stock".into()), true));
    assert_eq!(
        root(&before),
        vec!["billing", "stock"],
        "left half, it goes in front"
    );
    assert_eq!(
        root(&after),
        vec!["stock", "billing"],
        "right half, it goes behind"
    );

    // The same tile, both halves, opposite results — which is the whole point.
    assert_ne!(
        root(&before),
        root(&after),
        "aiming must be able to change the result"
    );
}

/// Every icon can be moved to either end of the list. A grid where the tile is
/// one slot instead of a seam cannot do this: aiming at the icon immediately to
/// the right put the dragged one back exactly where it was, so half the aims on
/// screen were no-ops and the arrangement never seemed to move.
#[test]
fn every_icon_can_be_moved_to_either_end_of_the_list() {
    let seed = [
        DeskIcon {
            id: "a".into(),
            kind: "workspace".into(),
            ..Default::default()
        },
        DeskIcon {
            id: "b".into(),
            kind: "workspace".into(),
            ..Default::default()
        },
        DeskIcon {
            id: "c".into(),
            kind: "workspace".into(),
            ..Default::default()
        },
        DeskIcon {
            id: "d".into(),
            kind: "workspace".into(),
            ..Default::default()
        },
    ];
    for dragged in ["a", "b", "c", "d"] {
        // Aimed at the first and last of the *other* icons: a drop on an icon
        // on itself cannot move it past itself, and asking it to would be
        // asking for a no-op to be a move.
        let other: Vec<&str> = ["a", "b", "c", "d"]
            .into_iter()
            .filter(|id| *id != dragged)
            .collect();
        let leads = move_icon(&seed, dragged, &Drop::onto(Some(other[0].into()), false));
        assert_eq!(
            ids(&leads)[0],
            dragged,
            "{dragged} leads, aimed at the front of {}",
            other[0]
        );

        let last = other[other.len() - 1];
        let trails = move_icon(&seed, dragged, &Drop::onto(Some(last.into()), true));
        assert_eq!(
            ids(&trails).last().copied(),
            Some(dragged),
            "{dragged} trails, aimed at the back of {}",
            other[other.len() - 1]
        );
    }
}

/// Aiming at the icon immediately to the right has to move something. It used
/// to be the drop that silently did nothing, and it is the one a user tries
/// first when reordering.
#[test]
fn aiming_at_the_next_icon_along_moves_it() {
    let seed = [
        DeskIcon {
            id: "a".into(),
            kind: "workspace".into(),
            ..Default::default()
        },
        DeskIcon {
            id: "b".into(),
            kind: "workspace".into(),
            ..Default::default()
        },
        DeskIcon {
            id: "c".into(),
            kind: "workspace".into(),
            ..Default::default()
        },
    ];
    let next = move_icon(&seed, "a", &Drop::onto(Some("b".into()), true));
    assert_eq!(ids(&next), vec!["b", "a", "c"], "a swaps with b");
}

/// Dropping on a folder files the icon inside it and takes it off the root.
#[test]
fn dropping_on_a_folder_files_the_icon_inside_it() {
    let next = move_icon(&seed(), "stock", &Drop::onto(Some("ops".into()), false));
    assert_eq!(root(&next), vec!["billing"], "it is no longer at the root");
    assert_eq!(members_of(&next, "ops"), ["sales", "tasks", "stock"]);
}

/// A folder is a list of workspaces by construction, so a folder dropped on
/// another one is placed next to it and never inside it. Filing one would be
/// an arrangement the server has no shape for: it keeps a folder's members
/// only if they are workspaces, so the nested folder would be dropped on the
/// next save, taking the icon the user was moving with it.
#[test]
fn a_folder_is_never_filed_inside_another_folder() {
    let seed = [
        folder_icon("ops"),
        folder_with("docs", &["sales"]),
        folder_with("stock", &["billing"]),
    ];

    let next = move_icon(&seed, "ops", &Drop::onto(Some("docs".into()), false));
    assert_eq!(
        ids(&next),
        vec!["ops", "docs", "stock"],
        "it sits in front of it"
    );
    assert_eq!(members_of(&next, "docs"), ["sales"], "and it filed nothing");

    let next = move_icon(&seed, "ops", &Drop::onto(Some("docs".into()), true));
    assert_eq!(ids(&next), vec!["docs", "ops", "stock"], "or behind it");

    // A folder dragged over another folder is ordered against it like anything
    // else, so the aim still decides.
    let next = move_icon(&seed, "ops", &Drop::onto(Some("docs".into()), true));
    assert_eq!(ids(&next), vec!["docs", "ops", "stock"]);

    let folders: Vec<&DeskIcon> = next.iter().filter(|icon| icon.is_folder()).collect();
    assert_eq!(folders.len(), 3, "all three folders are still on the desk");
    assert!(
        folders
            .iter()
            .all(|f| f.items.iter().all(|i| !i.is_folder())),
        "and no folder ended up inside another one"
    );
}

/// Dropping on empty grid space is the way out of a folder: no target at all.
#[test]
fn dropping_on_empty_space_files_the_workspace_back_at_the_root() {
    let inside = move_icon(&seed(), "stock", &Drop::onto(Some("ops".into()), false));
    let next = move_icon(&inside, "sales", &Drop::default());
    assert_eq!(root(&next), vec!["billing", "sales"]);
    assert_eq!(members_of(&next, "ops"), ["tasks", "stock"]);
}

/// A folder's dialog arranges its own members: dropping there reorders inside
/// that folder and nowhere else, which is what makes the dialog worth opening
/// in edit mode rather than a read-only list.
#[test]
fn a_drop_inside_a_folder_reorders_only_that_folder() {
    let next = move_icon(
        &seed(),
        "tasks",
        &Drop::within("ops", Some("sales".into()), false),
    );
    assert_eq!(members_of(&next, "ops"), ["tasks", "sales"]);
    assert_eq!(
        root(&next),
        vec!["stock", "billing"],
        "the rest of the desk does not move"
    );

    let next = move_icon(
        &seed(),
        "tasks",
        &Drop::within("ops", Some("sales".into()), true),
    );
    assert_eq!(
        members_of(&next, "ops"),
        ["sales", "tasks"],
        "and the same tile, the other half, moves it behind instead"
    );

    let next = move_icon(
        &seed(),
        "stock",
        &Drop::within("ops", Some("tasks".into()), false),
    );
    assert_eq!(
        members_of(&next, "ops"),
        ["sales", "stock", "tasks"],
        "a workspace from the root joins the folder it was dropped in"
    );
    assert_eq!(root(&next), vec!["billing"]);
}

/// Dropping on the space of a folder's dialog — the backdrop — is not inside
/// any folder, so the icon goes back to the desk rather than to the end of the
/// folder it was being arranged in.
#[test]
fn a_drop_on_a_folder_dialogs_backdrop_returns_the_icon_to_the_root() {
    let next = move_icon(&seed(), "sales", &Drop::default());
    assert_eq!(root(&next), vec!["stock", "billing", "sales"]);
    assert_eq!(members_of(&next, "ops"), ["tasks"]);
}

/// An icon dropped on itself, or on something the grid does not have, must
/// not be lost or duplicated — the grid is one icon per workspace, whatever
/// was dragged onto it.
#[test]
fn a_drop_that_goes_nowhere_still_leaves_one_of_the_icon() {
    let seed = seed();
    for target in [None, Some("billing"), Some("ghost")] {
        for after in [false, true] {
            let next = move_icon(
                &seed,
                "billing",
                &Drop::onto(target.map(String::from), after),
            );
            assert_eq!(next.len(), seed.len(), "no icon lost or duplicated");
            assert_eq!(ids(&next).iter().filter(|id| **id == "billing").count(), 1);
        }
    }
}

/// A workspace leaves its folder behind on its own, and a folder dragged as a
/// whole keeps its members.
#[test]
fn a_workspace_leaves_its_folder_and_a_folder_keeps_its_members() {
    let next = move_icon(&seed(), "sales", &Drop::onto(Some("stock".into()), false));
    assert_eq!(members_of(&next, "ops"), vec!["tasks"]);
    assert_eq!(
        root(&next),
        vec!["sales", "stock", "billing"],
        "it lands in front of the icon it was dropped on"
    );

    let next = move_icon(&seed(), "ops", &Drop::onto(Some("stock".into()), true));
    assert_eq!(ids(&next), vec!["stock", "ops", "billing"]);
    assert_eq!(members_of(&next, "ops"), ["sales", "tasks"]);
}

/// A drag that outlives the icon it names — the arrangement reloaded
/// mid-drag — leaves the grid as it was instead of losing an icon.
#[test]
fn moving_an_icon_the_grid_does_not_have_changes_nothing() {
    let seed = seed();
    let next = move_icon(&seed, "ghost", &Drop::onto(Some("stock".into()), true));
    assert_eq!(ids(&next), ids(&seed));
}

/// The folder form edits a folder in place and submits as one edit.
#[test]
fn the_folder_form_renames_and_dresses_one_folder() {
    let next = edit_folder(&seed(), "ops", "  Operations  ", "Boxes", "blue");
    let folder = folder_of(&next, "ops").expect("the folder");
    assert_eq!(folder.label, "Operations");
    assert_eq!(folder.icon.as_deref(), Some("Boxes"));
    assert_eq!(folder.color.as_deref(), Some("blue"));
    assert_eq!(members_of(&next, "ops"), ["sales", "tasks"]);
    assert_eq!(root(&next), vec!["stock", "billing"], "nothing else moved");
}

/// Clearing a field to retype it must not leave the folder nameless on the
/// grid, and clearing the colour puts it back to the desk default.
#[test]
fn clearing_the_fields_falls_back_to_what_the_folder_already_had() {
    let dressed = edit_folder(&seed(), "ops", "Operations", "Boxes", "blue");
    let folder =
        folder_of(&edit_folder(&dressed, "ops", "   ", "", ""), "ops").expect("the folder");
    assert_eq!(folder.label, "Operations");
    assert_eq!(folder.icon.as_deref(), Some("Boxes"));
    assert_eq!(folder.color, None);
}

/// Deleting a folder files what was in it back on the grid: a workspace the
/// user can no longer reach is worse than one out of place.
#[test]
fn deleting_a_folder_files_its_members_back_on_the_grid() {
    assert_eq!(
        ids(&delete_folder(&seed(), "ops")),
        vec!["stock", "billing", "sales", "tasks"]
    );
}

/// Hiding takes an icon off the grid without forgetting it, so unhiding brings
/// it back. Where it comes back to is the root, not the folder it was filed
/// in: a folder names its members, with nowhere to say one of them is hidden,
/// so hiding a member has to move it out to name it at all.
#[test]
fn hiding_keeps_the_workspace_in_the_arrangement() {
    let hidden = hide_icon(&seed(), "billing", true);
    assert!(
        hidden
            .iter()
            .find(|i| i.id == "billing")
            .expect("still there")
            .hidden
    );
    let back = hide_icon(&hidden, "billing", false);
    assert!(back.iter().all(|icon| !icon.hidden));
    assert!(
        placements_of(&back)
            .iter()
            .any(|p| p.workspace == "billing" && !p.hidden),
        "and it is saved as visible again"
    );
}

/// A workspace hidden inside a folder survives a round trip as hidden. The
/// failure this guards against is silent: the folder still names it, the flag
/// is dropped on the way out, and it reappears on the next load.
#[test]
fn a_workspace_hidden_inside_a_folder_stays_hidden_after_a_round_trip() {
    let grid = hide_icon(&seed(), "sales", true);
    let sent = placements_of(&grid);

    assert!(
        !sent
            .iter()
            .find(|p| p.folder == "ops")
            .expect("the folder")
            .items
            .contains(&"sales".to_string()),
        "it is not listed as a visible member"
    );
    assert!(
        sent.iter().any(|p| p.workspace == "sales" && p.hidden),
        "it is named once, as hidden"
    );
    assert_eq!(
        sent.iter().filter(|p| p.workspace == "sales").count(),
        1,
        "exactly once, or it would come back as a second icon"
    );
}

/// A hidden workspace is still named by the arrangement, which is what makes
/// it one click away instead of gone for good.
#[test]
fn a_hidden_workspace_is_still_saved() {
    let placements = placements_of(&hide_icon(&seed(), "billing", true));
    assert!(
        placements
            .iter()
            .any(|placement| placement.workspace == "billing" && placement.hidden),
        "it is sent as hidden, not dropped"
    );
}

/// Hiding reaches into a folder, and works on the root workspaces too. A
/// filed one comes out of its folder to be parked at the root, because that
/// is where the row of hidden workspaces is drawn.
#[test]
fn hiding_reaches_into_a_folder_and_the_root() {
    let hidden = hide_icon(&seed(), "tasks", true);
    assert_eq!(
        members_of(&hidden, "ops"),
        ["sales"],
        "a hidden member leaves the folder, so the folder stops naming it"
    );
    let parked = hidden
        .iter()
        .find(|i| i.id == "tasks")
        .expect("parked at the root");
    assert!(parked.hidden);

    assert!(
        hide_icon(&seed(), "stock", true)
            .iter()
            .find(|icon| icon.id == "stock")
            .expect("at the root")
            .hidden
    );
}

/// Hiding a filed workspace is reversible: the row under the grid is the only
/// place a hidden workspace is drawn, so the button that brings one back has
/// to be able to find it again.
#[test]
fn a_workspace_parked_out_of_a_folder_can_come_back() {
    let parked = hide_icon(&seed(), "tasks", true);
    let back = hide_icon(&parked, "tasks", false);
    assert!(
        back.iter().all(|icon| !icon.hidden),
        "and it is visible again"
    );
    assert_eq!(ids(&back), vec!["stock", "billing", "ops", "tasks"]);
    assert_eq!(
        back.iter()
            .find(|icon| icon.id == "tasks")
            .expect("at the root")
            .id,
        "tasks"
    );
}

/// Folders can be made one after another, and the second must not collide on
/// an id — it would overwrite the first, and both would answer to one icon.
#[test]
fn folders_can_be_made_one_after_another() {
    let (first, first_id) = add_folder(&seed());
    let (second, second_id) = add_folder(&first);
    assert_eq!(first.len(), 4);
    assert_eq!(second.len(), 5);
    assert_ne!(first_id, second_id);
    assert_eq!(
        ids(&second).iter().filter(|id| **id == second_id).count(),
        1
    );
    assert!(
        folder_of(&second, &second_id)
            .expect("the second")
            .items
            .is_empty()
    );
}

/// A folder id never lands on one the grid is already using.
#[test]
fn a_new_folder_id_collides_with_nothing() {
    let mut grid = seed();
    grid.push(DeskIcon {
        id: "folder-4".into(),
        kind: "folder".into(),
        ..Default::default()
    });
    let id = new_folder_id(&grid);
    assert_ne!(id, "folder-4");
    assert!(grid.iter().all(|icon| icon.id != id));
}

/// The arrangement sent is the grid drawn: order, grouping, hidden icons and
/// the folder's own fields all survive the round trip.
#[test]
fn the_arrangement_sent_is_the_grid_drawn() {
    let mut grid = seed();
    grid = move_icon(&grid, "stock", &Drop::onto(Some("ops".into()), false));
    grid = edit_folder(&grid, "ops", "Operations", "Boxes", "emerald");
    grid = hide_icon(&grid, "billing", true);

    let sent = placements_of(&grid);
    let folder = sent
        .iter()
        .find(|placement| placement.folder == "ops")
        .expect("the folder is sent");
    assert_eq!(folder.label, "Operations");
    assert_eq!(folder.icon, "Boxes");
    assert_eq!(folder.color, "emerald");
    assert_eq!(folder.items, ["sales", "tasks", "stock"]);

    let hidden = sent
        .iter()
        .find(|placement| placement.workspace == "billing")
        .expect("the hidden workspace is sent");
    assert!(hidden.hidden);
    assert!(!hidden.is_folder());
    assert_eq!(
        sent.len(),
        2,
        "one placement per icon, and no workspace sent twice"
    );
}
