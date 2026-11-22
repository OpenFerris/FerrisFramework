//! The appearance controls the builder shares between the board's
//! configuration panel and an instance's settings.
//!
//! One implementation per *kind* of decision, so the board panel and the
//! widget panel cannot drift apart:
//!
//! - [`NumberField`] — a bounded number where an empty input means "inherit"
//!   (`None`), never zero. That distinction is the whole precedence rule: a
//!   zero is a value, an absent field falls to the level below.
//! - [`TokenChoice`] — a closed list of semantic tokens plus "inherit", for a
//!   property that cannot take a free-form colour (the border names a theme
//!   token; `"none"` removes it).
//! - [`BackgroundFields`] — a surface, which *can* take a free-form colour:
//!   the select picks a token, "sin fondo" (`transparent`) or "personalizado",
//!   and the latter reveals a colour input. The contract keeps the token and
//!   the custom colour in two fields with the token winning; this control is
//!   the one place that mapping is written, so both panels store the same
//!   shape.
//!
//! Everything here writes into the caller's draft signal. The caller decides
//! what "Aplicar" means; these controls never touch the server, the list or
//! the saved configuration.
//!
//! The colour input is the browser's own `type="color"`, which yields a
//! `#rrggbb` string — one of the shapes the render accepts (see
//! [`super::style`]), so a custom colour still cannot smuggle a second CSS
//! declaration into the `style` attribute.

use leptos::prelude::*;

use ferris_schema::contract::WidgetInstance;
use ferris_schema::contract::style::{MAX_BORDER_WIDTH, MAX_PADDING, MAX_RADIUS, StyleSpec};

/// The semantic tokens offered as a surface. A closed list on purpose: a
/// free-form token name would emit `var(--…)` for a property no theme
/// defines, and the render would silently paint nothing.
pub(super) const SURFACE_TOKENS: &[(&str, &str)] = &[
    ("card", "Tarjeta"),
    ("muted", "Atenuado"),
    ("accent", "Acento"),
    ("secondary", "Secundario"),
    ("background", "Fondo"),
    ("primary", "Primario"),
    ("surface", "Superficie"),
    ("info", "Info"),
    ("success", "Éxito"),
    ("warning", "Aviso"),
    ("destructive", "Destructivo"),
];

/// The tokens offered for a border. `"none"` is the explicit "no border"; the
/// rest are the theme's line and accent colours.
pub(super) const BORDER_TOKENS: &[(&str, &str)] = &[
    ("none", "Sin borde"),
    ("border", "Borde"),
    ("input", "Campo"),
    ("primary", "Primario"),
    ("secondary", "Secundario"),
    ("muted", "Atenuado"),
    ("accent", "Acento"),
    ("destructive", "Destructivo"),
    ("success", "Éxito"),
    ("warning", "Aviso"),
    ("info", "Info"),
];

/// The elevation scale: the render maps exactly these tokens to a shadow and
/// ignores anything else, so the list the user sees is the list that works.
pub(super) const SHADOW_TOKENS: &[(&str, &str)] = &[
    ("none", "Sin sombra"),
    ("xs", "Muy suave"),
    ("sm", "Suave"),
    ("md", "Media"),
    ("lg", "Alta"),
];

/// The colour a "personalizado" surface starts from when the user has not
/// picked one yet. This is the value format `input[type="color"]` requires
/// (`#rrggbb`), not a design decision: the user's pick replaces it, and the
/// render treats it like any other free-form colour (validated at the edge).
const DEFAULT_CUSTOM: &str = "#ffffff";

/// A bounded number where an empty field means "inherit" (`None`).
#[component]
pub(super) fn NumberField(
    label: &'static str,
    value: Signal<Option<u8>>,
    max: u8,
    hint: &'static str,
    on_change: Callback<Option<u8>>,
) -> impl IntoView {
    view! {
        <label class="block space-y-1">
            <span class="block text-xs font-medium">{label}</span>
            <input
                type="number"
                min="0"
                max=max.to_string()
                class="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                prop:value=move || value.get().map_or_else(String::new, |n| n.to_string())
                on:input=move |ev| on_change.run(number(&event_target_value(&ev)))
            />
            <span class="block text-[0.65rem] text-muted-foreground">{hint}</span>
        </label>
    }
}

/// A closed list of semantic tokens, with "Por defecto" for "inherit".
#[component]
pub(super) fn TokenChoice(
    label: &'static str,
    value: Signal<Option<String>>,
    tokens: &'static [(&'static str, &'static str)],
    on_change: Callback<Option<String>>,
) -> impl IntoView {
    // A token this list does not know (an app's own declared default) stays
    // selectable: opening the panel must not silently rewrite it to "inherit"
    // just because the option was missing from the markup.
    let unknown = value
        .get_untracked()
        .filter(|current| !tokens.iter().any(|(token, _)| token == current));
    view! {
        <label class="block space-y-1">
            <span class="block text-xs font-medium">{label}</span>
            <select
                class="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                prop:value=move || value.get().unwrap_or_default()
                on:change=move |ev| {
                    let picked = event_target_value(&ev);
                    on_change.run((!picked.is_empty()).then_some(picked));
                }
            >
                <option value="">"Por defecto"</option>
                {tokens
                    .iter()
                    .map(|(token, label)| view! { <option value=*token>{*label}</option> })
                    .collect_view()}
                {unknown.map(|token| {
                    let label = token.clone();
                    view! { <option value=token>{label}</option> }
                })}
            </select>
        </label>
    }
}

/// A surface: semantic token, no surface, or a free-form colour.
#[component]
pub(super) fn BackgroundFields(
    label: &'static str,
    value: Signal<Option<String>>,
    custom: Signal<Option<String>>,
    on_change: Callback<(Option<String>, Option<String>)>,
) -> impl IntoView {
    let unknown = value
        .get_untracked()
        .filter(|token| token != "none" && !SURFACE_TOKENS.iter().any(|(known, _)| known == token));
    let mode = move || background_mode(value.get().as_deref(), custom.get().as_deref());
    view! {
        <label class="block space-y-1">
            <span class="block text-xs font-medium">{label}</span>
            <select
                class="h-8 w-full rounded-md border border-input bg-transparent px-2 text-sm"
                prop:value=mode
                on:change=move |ev| {
                    let picked = event_target_value(&ev);
                    let pair = match picked.as_str() {
                        "" => (None, None),
                        "none" => (Some("none".to_string()), None),
                        "custom" => (
                            None,
                            Some(
                                custom
                                    .get_untracked()
                                    .unwrap_or_else(|| DEFAULT_CUSTOM.to_string()),
                            ),
                        ),
                        token => (Some(token.to_string()), None),
                    };
                    on_change.run(pair);
                }
            >
                <option value="">"Por defecto"</option>
                <option value="none">"Sin fondo"</option>
                {SURFACE_TOKENS
                    .iter()
                    .map(|(token, label)| view! { <option value=*token>{*label}</option> })
                    .collect_view()}
                {unknown.map(|token| {
                    let label = token.clone();
                    view! { <option value=token>{label}</option> }
                })}
                <option value="custom">"Personalizado…"</option>
            </select>
        </label>
        {move || {
            (mode() == "custom")
                .then(|| {
                    view! {
                        <label class="block space-y-1">
                            <span class="block text-xs font-medium">"Color personalizado"</span>
                            <input
                                type="color"
                                class="h-8 w-full cursor-pointer rounded-md border border-input bg-transparent"
                                prop:value=move || {
                                    custom
                                        .get()
                                        .filter(|color| color.starts_with('#'))
                                        .unwrap_or_else(|| DEFAULT_CUSTOM.to_string())
                                }
                                on:input=move |ev| {
                                    on_change.run((None, Some(event_target_value(&ev))));
                                }
                            />
                        </label>
                    }
                })
        }}
    }
}

/// The `Estilo` section of an instance's settings: the widget's own override
/// over its definition's default.
///
/// Every control writes into the draft's [`StyleSpec`]; "Aplicar" (the panel's
/// own button) copies the draft into the list, exactly like the title and the
/// parameters above it.
#[component]
pub(super) fn WidgetStyleFields(draft: RwSignal<Option<WidgetInstance>>) -> impl IntoView {
    let style = Signal::derive(move || {
        draft.with(|slot| {
            slot.as_ref()
                .and_then(|widget| widget.style.clone())
                .unwrap_or_default()
        })
    });
    view! {
        <section class="space-y-3">
            <h3 class="text-[0.65rem] font-medium tracking-wide text-muted-foreground uppercase">
                "Estilo"
            </h3>
            <BackgroundFields
                label="Fondo"
                value=Signal::derive(move || style.get().background)
                custom=Signal::derive(move || style.get().background_custom)
                on_change=Callback::new(move |(background, custom)| {
                    update_style(draft, |style| {
                        style.background = background;
                        style.background_custom = custom;
                    });
                })
            />
            <TokenChoice
                label="Borde"
                value=Signal::derive(move || style.get().border_color)
                tokens=BORDER_TOKENS
                on_change=Callback::new(move |border_color| {
                    update_style(draft, |style| style.border_color = border_color);
                })
            />
            <NumberField
                label="Ancho del borde"
                value=Signal::derive(move || style.get().border_width)
                max=MAX_BORDER_WIDTH
                hint="Vacío = sin borde."
                on_change=Callback::new(move |width| {
                    update_style(draft, |style| style.border_width = width);
                })
            />
            <NumberField
                label="Radio"
                value=Signal::derive(move || style.get().border_radius)
                max=MAX_RADIUS
                hint="Vacío = el radio del tema."
                on_change=Callback::new(move |radius| {
                    update_style(draft, |style| style.border_radius = radius);
                })
            />
            <NumberField
                label="Padding"
                value=Signal::derive(move || style.get().padding)
                max=MAX_PADDING
                hint="Vacío = sin padding."
                on_change=Callback::new(move |padding| {
                    update_style(draft, |style| style.padding = padding);
                })
            />
            <TokenChoice
                label="Sombra"
                value=Signal::derive(move || style.get().shadow)
                tokens=SHADOW_TOKENS
                on_change=Callback::new(move |shadow| {
                    update_style(draft, |style| style.shadow = shadow);
                })
            />
        </section>
    }
}

/// The select's mode for a surface: the token, `"custom"` for a free-form
/// colour, or empty for "inherit".
///
/// The contract's rule is "the custom colour is used only when the token is
/// absent", so the control always writes the *pair*, never one field alone.
fn background_mode(background: Option<&str>, custom: Option<&str>) -> String {
    if let Some(token) = background {
        return token.to_string();
    }
    if custom.is_some() {
        return "custom".to_string();
    }
    String::new()
}

/// An empty field is "inherit"; anything else is the number the user typed.
/// The ceiling is enforced by the server's clamp, not by rewriting the input
/// while it is being typed into.
fn number(text: &str) -> Option<u8> {
    text.trim().parse().ok()
}

/// Edits the draft's style, keeping the invariant that an all-empty style is
/// stored as `None` (the instance declares nothing and inherits everything).
fn update_style(draft: RwSignal<Option<WidgetInstance>>, edit: impl FnOnce(&mut StyleSpec)) {
    draft.update(|slot| {
        let Some(widget) = slot.as_mut() else {
            return;
        };
        let mut style = widget.style.clone().unwrap_or_default();
        edit(&mut style);
        widget.style = (!style.is_empty()).then_some(style);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The select's mode: the token wins over a custom colour, a custom
    /// colour shows as "custom" only when there is no token, and nothing is
    /// "inherit".
    #[test]
    fn the_surface_mode_follows_the_contract_rule() {
        assert_eq!(background_mode(Some("card"), Some("#123")), "card");
        assert_eq!(background_mode(None, Some("#123")), "custom");
        assert_eq!(background_mode(Some("none"), None), "none");
        assert_eq!(background_mode(None, None), "");
    }

    /// An empty or malformed input is "inherit", not zero: zero is a value the
    /// user can choose on purpose.
    #[test]
    fn an_empty_number_is_inherit() {
        assert_eq!(number(""), None);
        assert_eq!(number("  "), None);
        assert_eq!(number("abc"), None);
        assert_eq!(number("0"), Some(0));
        assert_eq!(number("64"), Some(64));
    }

    /// Writing a style and then clearing every field leaves the instance
    /// declaring nothing, so the definition's default applies again.
    #[test]
    fn an_emptied_style_becomes_none() {
        let _owner = crate::test_support::owner();
        let draft = RwSignal::new(Some(WidgetInstance {
            id: "a".to_string(),
            widget: "host.stat".to_string(),
            ..Default::default()
        }));
        update_style(draft, |style| style.padding = Some(8));
        assert_eq!(
            draft.with(|slot| slot.as_ref().and_then(|widget| widget.style.clone())),
            Some(StyleSpec {
                padding: Some(8),
                ..Default::default()
            })
        );
        update_style(draft, |style| style.padding = None);
        assert_eq!(
            draft.with(|slot| slot.as_ref().and_then(|widget| widget.style.clone())),
            None
        );
    }
}
