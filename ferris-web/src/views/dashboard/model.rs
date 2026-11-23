//! Dashboard geometry: the Desk half of the placement rules.
//!
//! Everything that decides *where* and *how much* a widget occupies comes from
//! [`ferris_schema::contract::layout`] — the same function the server
//! runs on `PUT`. Here only the reference is resolved against the feed's
//! catalog (once, not once per widget) to give that rule the `Sizing` it
//! is due.

use std::collections::HashMap;

use ferris_schema::contract::layout::{self, CELL_HEIGHT_PX, COLUMN, Placement, Sizing};
use ferris_schema::contract::{AppUi, GridSize, WidgetDef, WidgetInstance};

/// Effective size of a reference: whatever the catalog declares (`size`/`min`)
/// or the `kind` default. Resolved via `contract::layout::GridSize::resolve`,
/// so the definition only declares and the shared rule clamps.
pub(super) fn sizing_of(apps: &[AppUi], owner: &str, reference: &str) -> Sizing {
    crate::ext::resolve_ref_in(apps, owner, reference).map_or_else(
        || GridSize::resolve("", None, None),
        |(_, def)| GridSize::resolve(&def.kind, def.size, def.min),
    )
}

/// Sizing per reference for a whole list, with **one** pass over the
/// catalog: normalizing 64 widgets must not re-resolve the catalog 64 times.
pub(super) fn sizing_map(
    apps: &[AppUi],
    owner: &str,
    widgets: &[WidgetInstance],
) -> HashMap<String, Sizing> {
    let mut map = HashMap::with_capacity(widgets.len());
    for widget in widgets {
        map.entry(widget.widget.clone())
            .or_insert_with(|| sizing_of(apps, owner, &widget.widget));
    }
    map
}

/// Normalizes the whole list against the catalog (fills cells, clamps and
/// re-places overlaps) with a single sizing map.
pub(super) fn normalize(apps: &[AppUi], owner: &str, widgets: &mut Vec<WidgetInstance>) {
    let sizes = sizing_map(apps, owner, widgets);
    layout::normalize(widgets, |reference| {
        sizes
            .get(reference)
            .copied()
            .unwrap_or_else(|| GridSize::resolve("", None, None))
    });
}

/// The resolved cell of an instance (with the feed's catalog).
pub(super) fn placement_of(apps: &[AppUi], owner: &str, widget: &WidgetInstance) -> Placement {
    layout::placement_of(widget, sizing_of(apps, owner, &widget.widget))
}

/// One grid row step in pixels: the cell height plus the board's gap. Row `n`
/// starts at `n * row_step(gap)`.
///
/// The gap is a parameter, not a constant: it is part of the board's saved
/// configuration (`DashboardStyle::gap`), and every consumer — the tile's box,
/// the board's height and the drag arithmetic — must read the *same* one, or a
/// dragged tile lands on a different row than the one it was dropped on.
const fn row_step(gap: u8) -> u32 {
    CELL_HEIGHT_PX as u32 + gap as u32
}
/// Total width of the *gaps* across the 12 columns: `left`/`width` are
/// expressed as a share of the board's inner width (`100%` includes the
/// gaps), so this is what one column's share has to give back.
const fn col_gaps(gap: u8) -> u32 {
    (COLUMN as u32 - 1) * gap as u32
}

/// Absolute box of a cell, as an inline style.
///
/// It is a *deliberate* exception to the rule "colors only via tokens, never
/// `style=`": here there is no color, there is data-dependent geometry (a cell
/// 3×2 in row 4 cannot be written as a literal class, and dynamic classes
/// do not survive Tailwind's purge). The board is a plain block whose height
/// is [`board_height`]; this only says where each box falls.
///
/// The box is *not* `grid-column`/`grid-row`: a grid placement snaps, and a
/// snap cannot animate. Percentage-based `calc()` positions do, so dragging
/// and resizing glide (see `.ferris-dashboard-tile` in `components.css`).
pub(super) fn tile_style(placement: Placement, gap: u8) -> String {
    let Placement {
        col,
        row,
        span,
        rows,
    } = placement;
    let (row_step, col_gaps) = (row_step(gap), col_gaps(gap));
    format!(
        "left: calc((100% - {col_gaps}px) / {COLUMN} * {col} + {col} * {gap}px); \
         top: calc({row} * {row_step}px); \
         width: calc((100% - {col_gaps}px) / {COLUMN} * {span} + {span_minus_1} * {gap}px); \
         height: calc({rows} * {row_step}px - {gap}px);",
        span_minus_1 = span.saturating_sub(1),
    )
}

/// The board's height as an inline style, so the absolutely positioned tiles
/// are contained and the page scrolls to the last row (including a drop
/// preview that extends past every tile). Never below one row.
pub(super) fn board_height(end_rows: u16, gap: u8) -> String {
    let end_rows = end_rows.max(1);
    let (row_step, gap) = (row_step(gap), u32::from(gap));
    format!("height: calc({end_rows} * {row_step}px - {gap}px);")
}

/// A unique instance id within the dashboard, derived from the definition's
/// name (the seed's ids are already taken).
pub(super) fn fresh_instance_id(existing: &[WidgetInstance], base: &str) -> String {
    let taken: std::collections::HashSet<&str> = existing.iter().map(|w| w.id.as_str()).collect();
    let clean: String = base.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let clean = if clean.is_empty() {
        "widget".to_string()
    } else {
        clean
    };
    let mut n = 0;
    loop {
        let candidate = if n == 0 {
            clean.clone()
        } else {
            format!("{clean}-{n}")
        };
        if !taken.contains(candidate.as_str()) {
            return candidate;
        }
        n += 1;
    }
}

/// The feed's widget catalog grouped by widget **category** (the free-form
/// `WidgetDef::category` — `"Metrics"`, `"Charts"`, …), so the palette is a
/// shelf per category instead of a flat list per publisher. This is D3 of
/// the dashboard widget: `category` was previously unread by the Desk.
/// (`kind` still decides sizing via [`sizing_of`] — it drives geometry, not
/// browsing.)
///
/// Groups come out in alphabetical order (a `BTreeMap`), definitions with no
/// `category` (or an empty one) land in `Otros` always at the end, and the
/// entries inside a group are ordered by `(owner, name)`. Each entry keeps its
/// `owner` so the palette can badge which app provides it.
pub(super) const UNGROUPED: &str = "views.dashboard.palette.others";

pub(super) fn catalog_by_category(apps: &[AppUi]) -> Vec<(String, Vec<(String, WidgetDef)>)> {
    let mut named: std::collections::BTreeMap<String, Vec<(String, WidgetDef)>> =
        std::collections::BTreeMap::new();
    let mut untagged: Vec<(String, WidgetDef)> = Vec::new();
    for app in apps {
        for def in &app.widgets {
            let entry = (app.owner.clone(), def.clone());
            match def
                .category
                .as_deref()
                .filter(|category| !category.is_empty())
            {
                Some(category) => named.entry(category.to_string()).or_default().push(entry),
                None => untagged.push(entry),
            }
        }
    }
    let mut groups: Vec<(String, Vec<(String, WidgetDef)>)> = named.into_iter().collect();
    for (_, entries) in &mut groups {
        entries.sort_by(|(owner_a, def_a), (owner_b, def_b)| {
            (owner_a, &def_a.name).cmp(&(owner_b, &def_b.name))
        });
    }
    if !untagged.is_empty() {
        untagged.sort_by(|(owner_a, def_a), (owner_b, def_b)| {
            (owner_a, &def_a.name).cmp(&(owner_b, &def_b.name))
        });
        groups.push((UNGROUPED.to_string(), untagged));
    }
    groups
}

/// A new instance for `reference`, placed *after* the existing ones
/// (cell absent: `normalize` gives it the first free slot).
pub(super) fn new_instance(
    existing: &[WidgetInstance],
    owner: &str,
    def: &WidgetDef,
) -> WidgetInstance {
    WidgetInstance {
        id: fresh_instance_id(existing, &def.name),
        widget: format!("{owner}.{}", def.name),
        label: None,
        // The host's chrome defaults: title shown (label/definition/reference),
        // subtitle absent until the editor enables it. The app declares no
        // title or subtitle of its own — it only contributes the body.
        show_title: None,
        subtitle: None,
        show_subtitle: None,
        params: Vec::new(),
        span: None,
        col: None,
        row: None,
        rows: None,
        // The instance's style starts absent: it inherits its definition's
        // declared default, and the settings panel can override it later.
        style: None,
    }
}

/// The maximum width (in columns) a box can occupy starting from `col`.
pub(super) fn max_span_from(col: u8) -> u8 {
    COLUMN.saturating_sub(col).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_schema::contract::layout::DEFAULT_ROWS;

    fn app(owner: &str, defs: Vec<WidgetDef>) -> AppUi {
        AppUi {
            owner: owner.to_string(),
            widgets: defs,
            ..Default::default()
        }
    }

    fn def(name: &str, kind: &str) -> WidgetDef {
        WidgetDef {
            name: name.to_string(),
            kind: kind.to_string(),
            ..Default::default()
        }
    }

    /// The `kind` decides the default size, and the declared catalog wins.
    #[test]
    fn sizing_uses_declared_size_then_kind_default() {
        let mut stat = def("stat", "stat");
        stat.size = Some(GridSize { span: 4, rows: 2 });
        stat.min = Some(GridSize { span: 2, rows: 2 });
        let apps = vec![app("host", vec![stat, def("chart", "chart")])];

        assert_eq!(
            sizing_of(&apps, "host", "host.stat").size,
            GridSize { span: 4, rows: 2 }
        );
        assert_eq!(
            sizing_of(&apps, "host", "host.chart").size,
            GridSize {
                span: 6,
                rows: DEFAULT_ROWS
            }
        );
        // An unknown reference falls back to the generic default, not a panic.
        assert_eq!(
            sizing_of(&apps, "host", "host.nope").size,
            GridSize {
                span: 6,
                rows: DEFAULT_ROWS
            }
        );
    }

    /// `normalize` fills the placement and the tile style reflects it as an
    /// animatable absolute box.
    #[test]
    fn normalize_fills_cells_and_tile_style_places_the_box() {
        let apps = vec![app("host", vec![def("chart", "chart")])];
        let mut widgets = vec![WidgetInstance {
            id: "a".to_string(),
            widget: "host.chart".to_string(),
            ..Default::default()
        }];
        normalize(&apps, "host", &mut widgets);
        let placement = placement_of(&apps, "host", &widgets[0]);
        assert_eq!(placement.col, 0);
        assert_eq!(placement.row, 0);
        assert_eq!(
            tile_style(placement, 12),
            "left: calc((100% - 132px) / 12 * 0 + 0 * 12px); \
             top: calc(0 * 84px); \
             width: calc((100% - 132px) / 12 * 6 + 5 * 12px); \
             height: calc(3 * 84px - 12px);"
        );
    }

    /// The gap is the board's, not a constant: a wider gap widens every column
    /// gap and every row step in the same proportion, so the tiles stay on the
    /// same lattice the drag arithmetic uses.
    #[test]
    fn tile_style_follows_the_configured_gap() {
        let placement = Placement {
            col: 0,
            row: 2,
            span: 1,
            rows: 1,
        };
        assert_eq!(
            tile_style(placement, 24),
            "left: calc((100% - 264px) / 12 * 0 + 0 * 24px); \
             top: calc(2 * 96px); \
             width: calc((100% - 264px) / 12 * 1 + 0 * 24px); \
             height: calc(1 * 96px - 24px);"
        );
        assert_eq!(
            tile_style(placement, 0),
            "left: calc((100% - 0px) / 12 * 0 + 0 * 0px); \
             top: calc(2 * 72px); \
             width: calc((100% - 0px) / 12 * 1 + 0 * 0px); \
             height: calc(1 * 72px - 0px);"
        );
    }

    /// A box away from the origin: column and row both move the box by exact
    /// multiples of the column/row step.
    #[test]
    fn tile_style_scales_with_col_row_span_and_rows() {
        let placement = Placement {
            col: 6,
            row: 4,
            span: 3,
            rows: 2,
        };
        assert_eq!(
            tile_style(placement, 12),
            "left: calc((100% - 132px) / 12 * 6 + 6 * 12px); \
             top: calc(4 * 84px); \
             width: calc((100% - 132px) / 12 * 3 + 2 * 12px); \
             height: calc(2 * 84px - 12px);"
        );
    }

    /// The board grows to hold its last row and never collapses to a negative
    /// height when empty.
    #[test]
    fn board_height_covers_end_rows_and_clamps_to_one() {
        assert_eq!(board_height(5, 12), "height: calc(5 * 84px - 12px);");
        assert_eq!(board_height(0, 12), "height: calc(1 * 84px - 12px);");
        assert_eq!(
            board_height(2, 24),
            "height: calc(2 * 96px - 24px);",
            "the height uses the same row step as the tiles"
        );
    }

    #[test]
    fn fresh_ids_avoid_collisions_with_the_seed() {
        let seed = vec![
            WidgetInstance {
                id: "kpi".to_string(),
                ..Default::default()
            },
            WidgetInstance {
                id: "kpi-2".to_string(),
                ..Default::default()
            },
            WidgetInstance {
                id: "chart".to_string(),
                ..Default::default()
            },
        ];
        assert_eq!(fresh_instance_id(&seed, "kpi"), "kpi-1");
        assert_eq!(fresh_instance_id(&seed, "chart"), "chart-1");
        assert_eq!(fresh_instance_id(&seed, ""), "widget");
        assert_eq!(fresh_instance_id(&seed, "sólo-ñ"), "slo");
    }

    /// The palette groups by widget **category** in alphabetical order, the
    /// entries inside a group by `(owner, name)`, definitions with no
    /// `category` — or an empty one — fall into `Otros` at the end, and apps
    /// without widgets do not appear. `kind` no longer decides the grouping
    /// (it still decides sizing).
    #[test]
    fn catalog_groups_by_category_with_untagged_last() {
        let mut cards = def("z", "stat");
        cards.category = Some("Metrics".into());
        let mut also_metrics = def("raw", "");
        also_metrics.category = Some("Metrics".into());
        let mut charts = def("b", "chart");
        charts.category = Some("Charts".into());
        let mut also_charts = def("a", "chart");
        also_charts.category = Some("Charts".into());
        let mut blank = def("blank", "table");
        blank.category = Some(String::new());
        // A `kind`-only widget (no category) is *not* grouped by kind anymore.
        let kind_only = def("kindly", "table");

        let apps = vec![
            app("zeta", vec![cards, also_metrics]),
            app("empty", vec![]),
            app("alpha", vec![charts, also_charts, blank, kind_only]),
        ];
        let groups = catalog_by_category(&apps);
        assert_eq!(
            groups.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["Charts", "Metrics", UNGROUPED]
        );
        assert_eq!(
            groups[0]
                .1
                .iter()
                .map(|(owner, def)| (owner.as_str(), def.name.as_str()))
                .collect::<Vec<_>>(),
            [("alpha", "a"), ("alpha", "b")]
        );
        assert_eq!(
            groups[1]
                .1
                .iter()
                .map(|(owner, def)| (owner.as_str(), def.name.as_str()))
                .collect::<Vec<_>>(),
            [("zeta", "raw"), ("zeta", "z")]
        );
        // `blank` (empty category) and `kindly` (none) share `Otros`.
        let mut others: Vec<&str> = groups[2]
            .1
            .iter()
            .map(|(_, def)| def.name.as_str())
            .collect();
        others.sort();
        assert_eq!(others, ["blank", "kindly"]);
    }

    #[test]
    fn max_span_is_bounded_by_the_remaining_columns() {
        assert_eq!(max_span_from(0), 12);
        assert_eq!(max_span_from(9), 3);
        assert_eq!(max_span_from(12), 1);
    }
}
