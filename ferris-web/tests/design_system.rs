//! Design-system guard rails (F1).
//!
//! These tests are the "rigour" half of the CSS-catalog work: they fail when
//! the tokens, the client fallback themes and the class catalog drift apart, so
//! a component cannot silently reference a colour that no theme defines.
//!
//! They read the stylesheet sources at test time (not the minified `desk.css`)
//! and compare them with the Rust-side data.

// Integration tests link the crate's whole dependency set but reference only a
// handful; the workspace's `unused_crate_dependencies` lint would otherwise fire
// once per unreferenced dependency.
#![allow(unused_crate_dependencies)]

use std::collections::{BTreeMap, BTreeSet};

const TAILWIND: &str = include_str!("../style/tailwind.css");
const COMPONENTS: &str = include_str!("../style/components.css");

/// Extract the body of a top-level `selector { … }` block.
fn block(css: &str, selector: &str) -> String {
    let needle = format!("{selector} {{");
    let start = css
        .find(&needle)
        .unwrap_or_else(|| panic!("block `{selector}` not found"));
    let open = start + css[start..].find('{').unwrap();
    let end = open + css[open..].find('}').unwrap();
    css[open + 1..end].to_string()
}

/// Collect the `--token` names declared inside a block.
fn tokens(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("--") {
            let name: String = rest
                .chars()
                .take_while(|c| *c != ':' && !c.is_whitespace())
                .collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

/// `--`-prefixed keys of a theme's vars.
fn theme_keys(theme: &ferris_web::theme::ThemeSpec) -> BTreeSet<String> {
    theme
        .vars
        .iter()
        .filter_map(|(k, _)| k.strip_prefix("--").map(str::to_string))
        .collect()
}

/// Collect the `--token` values declared inside a block, keyed by the name
/// without the leading `--`. Only the first `:` splits name from value.
fn token_values(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in body.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("--") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim().trim_end_matches(';').trim();
        if !name.is_empty() {
            out.insert(name.to_string(), value.to_string());
        }
    }
    out
}

/// Collapse runs of whitespace so `oklch(1 0 0)` and `oklch(1  0  0)` compare
/// equal; the literal digits are what matters.
fn normalise(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Compare every literal palette value in `theme` against the CSS block `body`.
///
/// `var(--x)` indirection is skipped — there is no literal to compare, the CSS
/// resolves it locally — but every oklch/hex literal must match after whitespace
/// normalisation. Returns how many literals were actually compared, so the
/// caller can fail the test if the comparison went vacuous.
fn assert_palette_values(
    block_name: &str,
    body: &str,
    theme: &ferris_web::theme::ThemeSpec,
) -> usize {
    let css = token_values(body);
    let mut checked = 0;
    for (key, value) in &theme.vars {
        // Non custom properties (e.g. `color-scheme`) have no `:root`-block
        // equivalent to compare against.
        let Some(name) = key.strip_prefix("--") else {
            continue;
        };
        // Structural tokens (`--radius`, `--sidebar-width`, …) are deliberately
        // not part of a theme palette.
        let Some(css_value) = css.get(name) else {
            continue;
        };
        let expected = normalise(value);
        let actual = normalise(css_value);
        if expected.starts_with("var(") || actual.starts_with("var(") {
            continue;
        }
        assert_eq!(
            actual, expected,
            "{block_name}: `--{name}` drifts between tailwind.css and the built-in theme"
        );
        checked += 1;
    }
    checked
}

/// `.dark` must define exactly the same tokens as `:root`; otherwise a token
/// missing from `.dark` silently keeps its light value under a runtime dark
/// theme (runtime themes paint `:root`, so `.dark` is the parity contract).
#[test]
fn dark_block_mirrors_root_token_set() {
    let mut root = tokens(&block(TAILWIND, ":root"));
    // Structural tokens are deliberately not part of a theme palette.
    for structural in [
        "radius",
        "sidebar-width",
        "sidebar-width-icon",
        "bottom__nav__height",
    ] {
        root.remove(structural);
    }
    let dark = tokens(&block(TAILWIND, ".dark"));
    assert!(
        !root.is_empty(),
        ":root parsed as empty — did the stylesheet move?"
    );
    assert_eq!(dark, root, ".dark must define the same tokens as :root");
}

/// The client fallback themes are what the desk paints when `/api/themes` is
/// unreachable; every colour token in the stylesheet must be present there.
#[test]
fn tailwind_tokens_match_client_fallback_themes() {
    let mut root = tokens(&block(TAILWIND, ":root"));
    // Structural tokens are deliberately not part of a theme palette.
    for structural in [
        "radius",
        "sidebar-width",
        "sidebar-width-icon",
        "bottom__nav__height",
    ] {
        root.remove(structural);
    }
    let dark = tokens(&block(TAILWIND, ".dark"));

    let themes = ferris_web::theme::builtin_themes();
    let light = themes
        .iter()
        .find(|t| t.id == "light")
        .expect("light theme");
    let dark_theme = themes.iter().find(|t| t.id == "dark").expect("dark theme");

    assert_eq!(
        theme_keys(light),
        root,
        "client `light` theme keys must mirror `:root` (minus --radius)"
    );
    assert_eq!(
        theme_keys(dark_theme),
        dark,
        "client `dark` theme keys must mirror `.dark`"
    );
}

/// Key parity is not enough: the built-in *values* must equal the stylesheet,
/// or the Desk paints a colour the static CSS never shows. This is what makes
/// the single Rust palette definition (in the wire contract) safe — change
/// `:root` or `.dark` without the Rust palette and it fails.
#[test]
fn tailwind_values_match_client_fallback_themes() {
    let themes = ferris_web::theme::builtin_themes();
    let light = themes
        .iter()
        .find(|t| t.id == "light")
        .expect("light theme");
    let dark = themes.iter().find(|t| t.id == "dark").expect("dark theme");

    let light_checked = assert_palette_values(":root", &block(TAILWIND, ":root"), light);
    let dark_checked = assert_palette_values(".dark", &block(TAILWIND, ".dark"), dark);

    assert!(
        light_checked >= 40,
        "only {light_checked} literal light tokens compared — did a block move?"
    );
    assert!(
        dark_checked >= 40,
        "only {dark_checked} literal dark tokens compared — did a block move?"
    );
}

/// Every `--color-*` utility mapping in `@theme inline` must point at a token
/// that is actually declared, or the utility silently resolves to nothing.
#[test]
fn theme_inline_colors_reference_declared_tokens() {
    let root = tokens(&block(TAILWIND, ":root"));
    let theme = block(TAILWIND, "@theme inline");
    for line in theme.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("--color-") else {
            continue;
        };
        let Some((_, rhs)) = line.split_once(':') else {
            continue;
        };
        let rhs = rhs.trim().trim_end_matches(';').trim();
        let Some(token) = rhs.strip_prefix("var(--").and_then(|s| s.strip_suffix(")")) else {
            continue;
        };
        assert!(
            root.contains(token),
            "`@theme inline` maps `--color-{rest}` to undeclared token `--{token}`"
        );
    }
}

/// Strip `/* … */` comments so class names mentioned in prose don't count as
/// real rules.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Every catalog constant must exist as a real `.class` rule, and names must be
/// unique. This is what keeps `ferris_web_ui::catalog` and `components.css`
/// from drifting apart.
#[test]
fn catalog_classes_exist_in_components_css() {
    let css = strip_comments(COMPONENTS);
    let mut seen = BTreeSet::new();
    for class in ferris_web_ui::catalog::ALL {
        assert!(seen.insert(*class), "duplicate catalog class `{class}`");
        let selector = format!(".{class}");
        assert!(
            css.contains(&selector),
            "catalog class `{class}` has no `{selector}` rule in components.css"
        );
    }
    assert!(
        !ferris_web_ui::catalog::ALL.is_empty(),
        "catalog is empty — the guard test would be vacuous"
    );
}

// ---------------------------------------------------------------------------
// DS-F3/DS-F4 ratchet: no raw colour literals in `ferris-web-ui` components nor
// in the Desk chrome (`ferris-web/src/desk`).
// ---------------------------------------------------------------------------

/// Files that still carry a forbidden colour literal (Tailwind palette class,
/// `#hex`, `rgb()`/`oklch()` literal or a `dark:` colour variant). Paths are
/// relative to the workspace root, e.g. `ferris-web-ui/src/...` or
/// `ferris-web/src/desk/...`.
///
/// This is a **ratchet**: migrate a component to a `.ferris-*` class (its colour
/// living in `style/components.css` as `var(--token)`) and delete its entry.
/// The list must only ever shrink — the test fails if one of these files is
/// clean (stale entry) or if any *other* file grows a raw colour.
const RAW_COLOR_ALLOWLIST: &[&str] = &[];

/// Upper bound on the allowlist size, pinned so it can only shrink. Lower it
/// whenever the list shrinks — never raise it.
const RAW_COLOR_BASELINE: usize = 0;

/// Tailwind colour families that must never appear with a shade in a component
/// (they are the *palette*, not the semantic tokens defined in `tailwind.css`).
const PALETTE: &[&str] = &[
    "white", "black", "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber",
    "yellow", "lime", "green", "emerald", "teal", "cyan", "sky", "blue", "indigo", "violet",
    "purple", "fuchsia", "pink", "rose",
];

/// Utilities that colour *with* a palette family (`bg-red-500`, `text-white`,
/// `shadow-black/5`, `border-t-zinc-200`, …).
const COLOR_PREFIXES: &[&str] = &[
    "bg",
    "text",
    "border",
    "border-t",
    "border-r",
    "border-b",
    "border-l",
    "border-x",
    "border-y",
    "border-s",
    "border-e",
    "ring",
    "ring-offset",
    "from",
    "via",
    "to",
    "fill",
    "stroke",
    "divide",
    "divide-x",
    "divide-y",
    "outline",
    "shadow",
    "accent",
    "caret",
    "decoration",
    "placeholder",
];

/// Remove `//` line and `/* … */` block comments so prose (and commented-out
/// code) cannot trip the scan. Naive on purpose: component sources have no
/// `//` inside string literals.
fn strip_rust_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let (mut line, mut block) = (false, false);
    while let Some(c) = chars.next() {
        if line {
            if c == '\n' {
                line = false;
                out.push(c);
            }
            continue;
        }
        if block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block = false;
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            line = true;
            chars.next();
        } else if c == '/' && chars.peek() == Some(&'*') {
            block = true;
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Split source into class-ish tokens (runs of characters that can appear in a
/// Tailwind class or an arbitrary value).
fn class_words(src: &str) -> impl Iterator<Item = &str> {
    src.split(|c: char| {
        !(c.is_ascii_alphanumeric()
            || matches!(
                c,
                '_' | '-'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '.'
                    | ','
                    | '%'
                    | '/'
                    | '*'
                    | '?'
                    | '!'
                    | '#'
                    | ':'
                    | '@'
            ))
    })
    .filter(|w| !w.is_empty())
}

/// Does this token colour with a palette family? Variants are stripped up to the
/// last `:` before the utility (`hover:bg-red-500` → `bg-red-500`).
fn uses_palette(word: &str) -> bool {
    let util = word.rsplit(':').next().unwrap_or(word);
    let util = util.trim_start_matches('!');
    for family in PALETTE {
        let needle = format!("-{family}");
        let mut from = 0;
        while let Some(rel) = util[from..].find(&needle) {
            let idx = from + rel;
            let after = &util[idx + needle.len()..];
            let boundary = after.is_empty()
                || after.starts_with('/')
                || after.starts_with(']')
                || after.starts_with(')')
                || (after.starts_with('-')
                    && after[1..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_digit()));
            if boundary {
                let before = &util[..idx];
                if COLOR_PREFIXES.contains(&before) {
                    return true;
                }
            }
            from = idx + needle.len();
        }
    }
    false
}

/// Does this token carry a `dark:` colour variant? (Runtime themes paint
/// `:root`, so `dark:` colour is dead code — see `AGENTS.md`.) Only a leading
/// `:`-segment counts and the utility after the last `:` must be non-empty, so
/// Rust prose like `dark: bool` or `prefers-color-scheme: dark` cannot trip it.
fn uses_dark_variant(word: &str) -> bool {
    let mut segments = word.split(':');
    let utility = segments.next_back().expect("split always yields one item");
    if utility.is_empty() {
        return false;
    }
    segments.any(|segment| segment == "dark")
}

/// A `#hex` literal, a `rgb()`/`oklch()`-style colour function, or a palette /
/// `dark:` token.
fn has_raw_colour(src: &str) -> bool {
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j] as char).is_ascii_hexdigit() {
                j += 1;
            }
            let digits = j - i - 1;
            let boundary = j >= bytes.len() || !(bytes[j] as char).is_ascii_alphanumeric();
            if (3..=8).contains(&digits) && boundary {
                return true;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    for func in ["rgb(", "rgba(", "hsl(", "hsla(", "oklch(", "oklab("] {
        if src.contains(func) {
            return true;
        }
    }
    class_words(src).any(|w| uses_palette(w) || uses_dark_variant(w))
}

fn collect_rs(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("read components dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs(&path, base, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let rel = path
                .strip_prefix(base)
                .expect("path under base")
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).expect("read component source");
            out.push((rel, src));
        }
    }
}

/// The DS-F3/DS-F4 gate: no component may introduce a raw colour, and the
/// allowlist may only shrink (stale entries fail too).
#[test]
fn no_raw_colour_literals_outside_allowlist() {
    // DS-F3 walked `ferris-web-ui`; DS-F4 added the Desk chrome. Both share the
    // same empty allowlist: a component expresses colour through a `.ferris-*`
    // class or a semantic token utility.
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let roots = [
        crate_root.join("ferris-web-ui/src"),
        crate_root.join("ferris-web/src/desk"),
    ];
    let mut files = Vec::new();
    for root in &roots {
        collect_rs(root, &crate_root, &mut files);
    }
    assert!(
        files.len() > 50,
        "walked only {} sources — did the path move?",
        files.len()
    );

    let mut offenders = BTreeSet::new();
    for (rel, src) in &files {
        if has_raw_colour(&strip_rust_comments(src)) {
            offenders.insert(rel.clone());
        }
    }

    let allow: BTreeSet<String> = RAW_COLOR_ALLOWLIST.iter().map(|s| s.to_string()).collect();
    let unexpected: Vec<&String> = offenders.difference(&allow).collect();
    assert!(
        unexpected.is_empty(),
        "raw colour literals in files outside RAW_COLOR_ALLOWLIST: {unexpected:?}\n\
         Move the colour into a `.ferris-*` class (rule in style/components.css, token \
         `var(--…)`) instead of a Tailwind palette / `#hex` / `oklch()` / `dark:`."
    );
    assert!(
        allow.len() == RAW_COLOR_BASELINE,
        "RAW_COLOR_ALLOWLIST is not at the baseline {RAW_COLOR_BASELINE} ({} entries); it may only shrink",
        allow.len()
    );
    let names: BTreeSet<&str> = files.iter().map(|(rel, _)| rel.as_str()).collect();
    for entry in RAW_COLOR_ALLOWLIST {
        assert!(
            names.contains(entry),
            "RAW_COLOR_ALLOWLIST names `{entry}`, which is not a component source"
        );
        assert!(
            offenders.contains(*entry),
            "`{entry}` is on RAW_COLOR_ALLOWLIST but has no raw colour left — remove it"
        );
    }
}
