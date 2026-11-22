//! Resolving the appearance of a board and its tiles.
//!
//! Three levels decide how a dashboard looks, and this module is the single
//! place the Desk resolves them:
//!
//! 1. the Desk's global style (one shared row, [`crate::api::dashboard_layout`]),
//! 2. the dashboard's declared style (an app's `describe().dashboards[].style`
//!    or the `style` field of a native `Dashboard` document),
//! 3. the saved override ([`DashboardConfig`], shared by every user).
//!
//! A tile has its own two levels — the definition's declared default and the
//! instance's override — and only those two: the Desk deliberately adds no
//! look of its own at this level. The tile is the **only** frame: a plugin
//! body paints no card of its own (`views::widget_host`), so an app that
//! declares nothing renders as the bare element inside a transparent,
//! padding-less tile and opts into a surface field by field when it wants one.
//!
//! ## Why the result is a set of CSS custom properties
//!
//! The values are runtime data, and a Tailwind class cannot be built from
//! runtime data (the purge never sees it). The plan's rule is therefore: a
//! semantic token becomes a `var(--token)` reference and a free-form colour
//! becomes its own value, both written into **one custom property** on the
//! element; `style/components.css` consumes them with the Desk default as the
//! fallback. So the literal never reaches a class name, and a theme that
//! repaints `--card` repaints every tile that asked for `"card"`.
//!
//! ## Why the values are validated
//!
//! The strings arrive from an app's metadata and from a user's saved config,
//! and they end up inside a `style` attribute. A token is therefore restricted
//! to a CSS identifier and a free-form colour to a colour grammar; anything
//! else is dropped (the next level's value applies) rather than injected. This
//! is the boundary validation the rest of the Desk applies to external input —
//! `background: "red; background-image: url(…)"` must never become a
//! declaration.

use ferris_schema::contract::{AppUi, DashboardConfig, DashboardStyle, StyleSpec, WidgetInstance};

use crate::components::desk::top_bar::TopBarChromeSpec;

/// Maximum length of a free-form colour. Long enough for `color(display-p3 …)`
/// or a long `oklch()` chain, short enough that a config row cannot smuggle a
/// stylesheet through it.
const MAX_COLOR_LEN: usize = 64;

/// The effective style of a board: the saved override over the declared style
/// over the Desk's global style, field by field.
///
/// The server stores and returns the **override only** (never a merged value),
/// so an explicit here-beats-there resolution is the client's job — and this is
/// the one function that does it. The global is a parameter, not a constant:
/// since phase C8 it is one shared row the Desk fetches
/// (`/api/dashboard-settings`), so all three layers are runtime data and the
/// board cannot resolve a different precedence than these tests.
pub(super) fn board_style(
    global: &DashboardStyle,
    declared: Option<&DashboardStyle>,
    config: &DashboardConfig,
) -> DashboardStyle {
    let declared = declared.cloned().unwrap_or_default().overlay(global);
    config.overlay(&declared)
}

/// The bar's chrome for a resolved board style and the visit's mode.
///
/// The contract models the title flag as `Option<bool>` (absent = keep the
/// title in full screen), the top bar as two plain booleans, so the mapping
/// lives here next to the precedence rule and has a test of its own. An
/// explicit `false` is *not* the same as inheriting: a dashboard can opt out of
/// a global decision that keeps the title, and `overlay` already resolved that.
#[must_use]
pub(super) fn chrome(style: &DashboardStyle, fullscreen: bool) -> TopBarChromeSpec {
    TopBarChromeSpec {
        fullscreen,
        title: style.title_full_screen.unwrap_or(true),
    }
}

/// The effective style of one instance: its own override over its definition's
/// declared default.
///
/// A reference the catalog does not know contributes nothing (the seed is
/// filtered by the server, and a stale tile is one frame from unmounting), and
/// neither level declaring anything yields the empty style — the common case,
/// which must emit no property at all.
pub(super) fn widget_style(apps: &[AppUi], owner: &str, instance: &WidgetInstance) -> StyleSpec {
    let declared = crate::ext::resolve_ref_in(apps, owner, &instance.widget)
        .and_then(|(_, def)| def.style)
        .unwrap_or_default();
    instance
        .style
        .clone()
        .unwrap_or_default()
        .overlay(&declared)
}

/// Whether the tile declares a surface of its own (background, border or
/// shadow), in which case a `.ferris-card` a body still renders must not paint
/// a second one over it.
///
/// Padding and radius alone do not count: they shape the tile, and the card
/// keeps its normal look inside the wider frame.
pub(super) fn owns_surface(style: &StyleSpec) -> bool {
    style.background.is_some()
        || style.background_custom.is_some()
        || style.border_width.is_some()
        || style.border_color.is_some()
        || style.shadow.is_some()
}

/// The board's CSS custom properties, or an empty string when it declares none
/// (the render then skips the `style` attribute entirely).
pub(super) fn board_vars(style: &DashboardStyle) -> String {
    let mut out = String::new();
    if let Some(background) = background(
        style.background.as_deref(),
        style.background_custom.as_deref(),
    ) {
        push_var(&mut out, "--ferris-board-bg", &background);
    }
    if let Some(radius) = style.border_radius {
        push_var(&mut out, "--ferris-board-radius", &px(radius));
    }
    if let Some(padding) = style.padding_px {
        push_var(&mut out, "--ferris-board-padding", &px(padding));
    }
    out.trim_end().to_string()
}

/// The tile's CSS custom properties, or an empty string when it declares none.
pub(super) fn tile_vars(style: &StyleSpec) -> String {
    let mut out = String::new();
    if let Some(background) = background(
        style.background.as_deref(),
        style.background_custom.as_deref(),
    ) {
        push_var(&mut out, "--ferris-tile-bg", &background);
    }
    if let Some(border) = style.border_color.as_deref() {
        let value = if border.trim().eq_ignore_ascii_case("none") {
            Some("transparent".to_string())
        } else {
            token_ref(border)
        };
        if let Some(value) = value {
            push_var(&mut out, "--ferris-tile-border-color", &value);
        }
    }
    if let Some(width) = style.border_width {
        push_var(&mut out, "--ferris-tile-border-width", &px(width));
    }
    if let Some(radius) = style.border_radius {
        push_var(&mut out, "--ferris-tile-radius", &px(radius));
    }
    if let Some(padding) = style.padding {
        push_var(&mut out, "--ferris-tile-padding", &px(padding));
    }
    if let Some(shadow) = style.shadow.as_deref().and_then(shadow) {
        push_var(&mut out, "--ferris-tile-shadow", shadow);
    }
    out.trim_end().to_string()
}

/// The surface of an element: the semantic token as `var(--token)`, the
/// free-form colour only when the token is absent, and `transparent` for the
/// explicit "no background" names.
///
/// A token that does not look like an identifier is treated like an absent
/// one, so a typo in an app's metadata degrades to the free-form colour (or to
/// nothing) instead of emitting a broken reference.
fn background(token: Option<&str>, custom: Option<&str>) -> Option<String> {
    if let Some(token) = token {
        let token = token.trim();
        if token.eq_ignore_ascii_case("none") || token.eq_ignore_ascii_case("transparent") {
            return Some("transparent".to_string());
        }
        if let Some(reference) = token_ref(token) {
            return Some(reference);
        }
    }
    custom.and_then(custom_color)
}

/// A semantic token as a `var(--token)` reference, or `None` when it is not a
/// plausible custom-property name (letters, digits and dashes, starting with a
/// letter).
fn token_ref(token: &str) -> Option<String> {
    let token = token.trim();
    let first = token.chars().next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(format!("var(--{token})"))
}

/// A free-form colour, accepted only if it matches one of the shapes a colour
/// can take: `#hex`, one of the colour functions, a `var(--token)` reference or
/// a plain keyword (`red`, `transparent`, `currentColor`).
///
/// Deliberately a whitelist: this string is written into a `style` attribute,
/// so anything that could carry a second declaration (`;`, `}`, `url(`, …) is
/// refused rather than escaped.
fn custom_color(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_COLOR_LEN {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        let valid = (3..=8).contains(&hex.len()) && hex.chars().all(|c| c.is_ascii_hexdigit());
        return valid.then(|| value.to_string());
    }
    if let Some(inner) = value.strip_prefix("var(").and_then(|v| v.strip_suffix(')')) {
        let name = inner.trim().strip_prefix("--")?;
        let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        return valid.then(|| value.to_string());
    }
    const FUNCTIONS: [&str; 9] = [
        "rgb(", "rgba(", "hsl(", "hsla(", "oklch(", "oklab(", "lab(", "lch(", "color(",
    ];
    let lower = value.to_ascii_lowercase();
    if FUNCTIONS.iter().any(|prefix| lower.starts_with(prefix)) {
        let safe = value.ends_with(')')
            && !lower.contains("url(")
            && value.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '(' | ')' | ',' | '.' | '%' | '/' | '-' | ' ' | '+')
            });
        return safe.then(|| value.to_string());
    }
    let keyword = value.chars().all(|c| c.is_ascii_alphabetic());
    keyword.then(|| value.to_string())
}

/// A token-driven elevation: the four scale steps the contract documents,
/// built from `--foreground` with a `color-mix` alpha — the same technique the
/// stylesheet uses for every opacity modifier, so no literal palette colour
/// enters a shadow.
fn shadow(token: &str) -> Option<&'static str> {
    match token.trim() {
        "none" => Some("none"),
        "xs" => Some("0 1px 2px 0 color-mix(in oklab, var(--foreground) 5%, transparent)"),
        "sm" => Some(
            "0 1px 3px 0 color-mix(in oklab, var(--foreground) 10%, transparent), \
             0 1px 2px -1px color-mix(in oklab, var(--foreground) 10%, transparent)",
        ),
        "md" => Some(
            "0 4px 6px -1px color-mix(in oklab, var(--foreground) 10%, transparent), \
             0 2px 4px -2px color-mix(in oklab, var(--foreground) 10%, transparent)",
        ),
        "lg" => Some(
            "0 10px 15px -3px color-mix(in oklab, var(--foreground) 10%, transparent), \
             0 4px 6px -4px color-mix(in oklab, var(--foreground) 10%, transparent)",
        ),
        _ => None,
    }
}

/// `n` pixels, as the CSS value.
fn px(n: u8) -> String {
    format!("{n}px")
}

/// Append one custom property to the inline style.
fn push_var(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push_str("; ");
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_schema::contract::{DashboardStyle, WidgetDef};

    fn app(owner: &str, defs: Vec<WidgetDef>) -> AppUi {
        AppUi {
            owner: owner.to_string(),
            widgets: defs,
            ..Default::default()
        }
    }

    /// The precedence is exactly the plan's: the saved override wins field by
    /// field, an absent field falls to the app's declaration, and an absent
    /// declaration falls to the Desk's default — never to a zero.
    #[test]
    fn the_board_style_layers_override_over_declaration_over_global() {
        let declared = DashboardStyle {
            gap_px: Some(20),
            padding_px: Some(16),
            background: Some("muted".to_string()),
            ..Default::default()
        };
        let config = DashboardConfig {
            dashboard_id: None,
            style: DashboardStyle {
                gap_px: Some(28),
                ..Default::default()
            },
        };
        let global = DashboardStyle {
            gap_px: Some(12),
            padding_px: Some(8),
            border_radius: Some(6),
            ..Default::default()
        };
        let style = board_style(&global, Some(&declared), &config);
        assert_eq!(style.gap_px, Some(28), "the override wins");
        assert_eq!(
            style.padding_px,
            Some(16),
            "the declaration beats the global"
        );
        assert_eq!(style.background.as_deref(), Some("muted"));
        assert_eq!(style.gap(), 28);
        assert_eq!(
            style.border_radius,
            Some(6),
            "a field only the global sets survives both layers"
        );

        let untouched = board_style(
            &DashboardStyle::default(),
            None,
            &DashboardConfig::default(),
        );
        assert!(
            untouched.is_empty(),
            "nothing declared and nothing overridden stays empty"
        );
        assert_eq!(
            untouched.gap(),
            ferris_schema::contract::layout::DEFAULT_GAP_PX,
            "the empty style keeps the Desk's default gap"
        );
    }

    /// The global can keep the title in full screen and a board can drop it:
    /// `false` is a value, not "inherit", so the precedence must carry it.
    #[test]
    fn a_board_can_opt_out_of_a_global_flag() {
        let global = DashboardStyle {
            title_full_screen: Some(true),
            ..Default::default()
        };
        let declared = DashboardStyle {
            title_full_screen: Some(false),
            ..Default::default()
        };
        let style = board_style(&global, Some(&declared), &DashboardConfig::default());
        assert_eq!(style.title_full_screen, Some(false));
        assert_eq!(
            chrome(&style, true),
            TopBarChromeSpec {
                fullscreen: true,
                title: false,
            }
        );
    }

    /// The tile: the instance's own style wins, the definition's default is the
    /// fallback, and neither declaring anything yields an empty style.
    #[test]
    fn the_widget_style_layers_instance_over_definition() {
        let mut def = WidgetDef {
            name: "stat".to_string(),
            kind: "stat".to_string(),
            ..Default::default()
        };
        def.style = Some(StyleSpec {
            background: Some("card".to_string()),
            padding: Some(8),
            ..Default::default()
        });
        let apps = vec![app("host", vec![def])];

        let bare = WidgetInstance {
            id: "a".to_string(),
            widget: "host.stat".to_string(),
            ..Default::default()
        };
        let inherited = widget_style(&apps, "host", &bare);
        assert_eq!(inherited.background.as_deref(), Some("card"));
        assert_eq!(inherited.padding, Some(8));

        let mut own = bare.clone();
        own.style = Some(StyleSpec {
            background: Some("accent".to_string()),
            ..Default::default()
        });
        let resolved = widget_style(&apps, "host", &own);
        assert_eq!(resolved.background.as_deref(), Some("accent"));
        assert_eq!(
            resolved.padding,
            Some(8),
            "the field the instance did not set still inherits"
        );

        let unknown = WidgetInstance {
            id: "b".to_string(),
            widget: "host.gone".to_string(),
            ..Default::default()
        };
        assert!(widget_style(&apps, "host", &unknown).is_empty());
    }

    /// A token becomes a `var()` reference; a free-form colour is used only
    /// when there is no token; `"none"` clears the surface.
    #[test]
    fn colours_resolve_to_tokens_before_literals() {
        let style = StyleSpec {
            background: Some("card".to_string()),
            background_custom: Some("#123456".to_string()),
            border_color: Some("none".to_string()),
            border_width: Some(2),
            border_radius: Some(10),
            padding: Some(4),
            shadow: Some("sm".to_string()),
        };
        let vars = tile_vars(&style);
        assert!(vars.contains("--ferris-tile-bg: var(--card);"), "{vars}");
        assert!(
            !vars.contains("#123456"),
            "the token wins over the free-form colour: {vars}"
        );
        assert!(
            vars.contains("--ferris-tile-border-color: transparent;"),
            "{vars}"
        );
        assert!(vars.contains("--ferris-tile-border-width: 2px;"), "{vars}");
        assert!(vars.contains("--ferris-tile-radius: 10px;"), "{vars}");
        assert!(vars.contains("--ferris-tile-padding: 4px;"), "{vars}");
        assert!(vars.contains("--ferris-tile-shadow: 0 1px 3px 0"), "{vars}");

        let custom = StyleSpec {
            background_custom: Some("oklch(0.7 0.1 200)".to_string()),
            ..Default::default()
        };
        assert!(
            tile_vars(&custom).contains("--ferris-tile-bg: oklch(0.7 0.1 200);"),
            "the free-form colour is used when no token is declared"
        );
    }

    /// Nothing declared emits nothing — the render then skips the attribute and
    /// the tile keeps the stylesheet's default look.
    #[test]
    fn an_empty_style_emits_no_property() {
        assert!(tile_vars(&StyleSpec::default()).is_empty());
        assert!(board_vars(&DashboardStyle::default()).is_empty());
    }

    /// Only a declared surface makes the tile take over the card inside it; a
    /// padding or a radius alone keeps the widget's own card.
    #[test]
    fn a_surface_is_owned_only_when_one_is_declared() {
        assert!(!owns_surface(&StyleSpec::default()));
        assert!(!owns_surface(&StyleSpec {
            padding: Some(16),
            border_radius: Some(12),
            ..Default::default()
        }));
        for declared in [
            StyleSpec {
                background: Some("card".to_string()),
                ..Default::default()
            },
            StyleSpec {
                background_custom: Some("#123".to_string()),
                ..Default::default()
            },
            StyleSpec {
                border_width: Some(1),
                ..Default::default()
            },
            StyleSpec {
                shadow: Some("sm".to_string()),
                ..Default::default()
            },
        ] {
            assert!(owns_surface(&declared), "{declared:?} owns a surface");
        }
    }

    /// Injection attempts and malformed values are dropped, not escaped: the
    /// string ends up in a `style` attribute.
    #[test]
    fn unsafe_colour_and_token_values_are_dropped() {
        for value in [
            "red; background-image: url(https://example.test/x)",
            "url(https://example.test/x)",
            "expression(alert(1))",
            "#12",
            "#12345g",
            "rgb(1,2,3); color: red",
            "var(--x); color: red",
            "",
        ] {
            assert!(
                custom_color(value).is_none(),
                "`{value}` must not become a declaration"
            );
        }
        for value in [
            "#123",
            "#11223344",
            "rgb(1 2 3 / 50%)",
            "var(--brand)",
            "red",
            "transparent",
        ] {
            assert!(
                custom_color(value).is_some(),
                "`{value}` is a colour and must be accepted"
            );
        }
        for token in ["card", "sidebar-accent", "background"] {
            assert_eq!(token_ref(token), Some(format!("var(--{token})")));
        }
        for token in ["", "1card", "card; color: red", "--card", "card)", "a b"] {
            assert!(token_ref(token).is_none(), "`{token}` is not a token");
        }
    }

    /// A shadow token outside the documented scale contributes nothing, and
    /// `"none"` is an explicit no-shadow, not an absent value.
    #[test]
    fn shadows_are_a_closed_scale() {
        assert_eq!(shadow("none"), Some("none"));
        for token in ["xs", "sm", "md", "lg"] {
            assert!(shadow(token).is_some(), "`{token}` is on the scale");
        }
        assert!(shadow("xl").is_none());
        assert!(shadow("0 0 1px #000").is_none());
    }

    /// The board properties are the board's own geometry, never the tile's.
    #[test]
    fn the_board_properties_are_separate_from_the_tile_ones() {
        let style = DashboardStyle {
            background_custom: Some("var(--brand)".to_string()),
            gap_px: Some(24),
            padding_px: Some(32),
            border_radius: Some(12),
            title_full_screen: Some(true),
            ..Default::default()
        };
        let vars = board_vars(&style);
        assert!(vars.contains("--ferris-board-bg: var(--brand);"), "{vars}");
        assert!(vars.contains("--ferris-board-padding: 32px;"), "{vars}");
        assert!(vars.contains("--ferris-board-radius: 12px;"), "{vars}");
        assert!(
            !vars.contains("--ferris-tile-") && !vars.contains("gap"),
            "the gap and the chrome flags are not CSS: {vars}"
        );
    }

    /// Full screen always follows the visit's mode; the title follows the board
    /// and defaults to shown when the board keeps quiet.
    #[test]
    fn chrome_keeps_the_title_in_full_screen_by_default() {
        // Out of full screen `title` is not consulted (the bar always draws the
        // title there); the flag is still resolved to its documented default,
        // which is *not* `TopBarChromeSpec::default()` — that type's zero value
        // means "no board published chrome", so it carries `title: false`.
        assert_eq!(
            chrome(&DashboardStyle::default(), false),
            TopBarChromeSpec {
                fullscreen: false,
                title: true,
            }
        );
        assert_eq!(
            chrome(&DashboardStyle::default(), true),
            TopBarChromeSpec {
                fullscreen: true,
                title: true,
            }
        );

        let opt_out = DashboardStyle {
            title_full_screen: Some(false),
            ..Default::default()
        };
        assert_eq!(
            chrome(&opt_out, true),
            TopBarChromeSpec {
                fullscreen: true,
                title: false,
            }
        );
    }
}
