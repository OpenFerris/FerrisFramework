//! `Widget`: a widget definition rendered inside the form layout.
//!
//! A form can reserve a place for a widget the same way a dashboard does — a
//! custom field of type `Widget` whose `options` names the widget
//! (`<app>/<widget>`, or a bare `<widget>` id resolved against the apps the
//! feed declares). The body is the **same host the dashboard grid and the slot
//! system use** ([`WidgetHost`]): the native `host.*` renderers, a plugin's
//! custom element, and the same unresolved placeholder when the reference does
//! not match any catalog. Nothing here knows a widget by name, so a field stays
//! metadata-driven like every other one.
//!
//! The field carries no value: the document payload and `validate_form` skip it
//! (`Widget` is in both lists next to `Button`), so adding one never changes a
//! stored document.
//!
//! In the layout editor the ordinary field overlay covers it like any other
//! field — click selects it for the inspector, drag moves it — which is why the
//! widget is never interactive while the form's shape is being edited.

use leptos::prelude::*;

use crate::components::ui::icon::Icon;
use crate::ext::UiCtx;
use crate::views::widget_host::WidgetHost;

/// How a `Widget` field's `options` names its widget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum WidgetRef {
    /// `<app>/<widget>` (also accepted with `:` or `.` as the separator).
    Named { owner: String, widget: String },
    /// A bare `<widget>`: resolved against the feed's catalogs, first match.
    Bare(String),
    /// No options: nothing to render.
    Empty,
}

/// Parse the field's `options` into a [`WidgetRef`].
///
/// `WidgetHost` resolves `<owner>.<name>` against a default owner, so the
/// `<app>/<widget>` spelling the field documents is normalized to
/// `owner = app`, `reference = "<app>.<widget>"`.
pub(super) fn parse_widget_ref(options: &str) -> WidgetRef {
    let trimmed = options.trim();
    if trimmed.is_empty() {
        return WidgetRef::Empty;
    }
    match trimmed.split_once(['/', ':', '.']) {
        Some((owner, widget)) if !owner.trim().is_empty() && !widget.trim().is_empty() => {
            WidgetRef::Named {
                owner: owner.trim().to_string(),
                widget: widget.trim().to_string(),
            }
        }
        _ => WidgetRef::Bare(trimmed.to_string()),
    }
}

/// The `(default_owner, reference)` pair [`WidgetHost`] needs, resolving a bare
/// id against the catalogs the feed declares.
fn widget_target(ctx: &UiCtx, reference: &WidgetRef) -> Option<(String, String)> {
    match reference {
        WidgetRef::Empty => None,
        WidgetRef::Named { owner, widget } => Some((owner.clone(), format!("{owner}.{widget}"))),
        WidgetRef::Bare(name) => ctx.apps.with_untracked(|apps| {
            apps.iter()
                .find(|app| app.widgets.iter().any(|widget| &widget.name == name))
                .map(|app| (app.owner.clone(), format!("{}.{}", app.owner, name)))
        }),
    }
}

/// A widget placed in the form layout.
#[component]
pub fn WidgetField(
    #[prop(into)] options: String,
    /// The field's label, painted by the host as the widget's heading. Empty
    /// falls back to the widget definition's own label.
    #[prop(into, optional)]
    label: String,
) -> impl IntoView {
    let i18n = crate::i18n::use_i18n();
    let reference = parse_widget_ref(&options);
    if reference == WidgetRef::Empty {
        return placeholder(i18n.t("components.form.widget.missing_reference")).into_any();
    }
    // `use_context`, not `expect_context`: the field is also rendered by the
    // dashboard parameter panel and in standalone tests, where the OpenUI feed
    // is not provided. A missing host is a notice, never a panic.
    let Some(ctx) = use_context::<UiCtx>() else {
        return placeholder(i18n.t("components.form.widget.no_catalog")).into_any();
    };
    // Resolved once; the render closure below re-resolves when the feed lands.
    let reference = StoredValue::new(reference);
    // `options` is read by the notice closure and by the `data-widget` hook; the
    // label is owned by this view (`StoredValue`), not borrowed from the caller.
    let declared = options.clone();
    let missing_i18n = i18n;
    let missing = move || {
        missing_i18n.t_with(
            "components.form.widget.not_in_catalog",
            &[("widget", declared.trim())],
        )
    };
    let label = StoredValue::new(label);
    let title = {
        let text = label.with_value(|label| label.trim().to_string());
        (!text.is_empty()).then_some(text)
    };

    view! {
        <div
            class="flex min-h-44 min-w-0 flex-col rounded-xl border bg-card p-3"
            data-name="WidgetField"
            data-widget=options
        >
            {move || {
                // Tracked on purpose: the feed lands after the first paint, and
                // an untracked read would leave the unresolved placeholder on
                // screen for a widget the catalog already knows.
                let _ = ctx.apps.get();
                match widget_target(&ctx, &reference.get_value()) {
                    Some((owner, reference)) => view! {
                        <WidgetHost
                            default_owner=owner
                            reference=reference
                            label=title.clone()
                            show_title=Some(true)
                        />
                    }
                    .into_any(),
                    None => view! {
                        <div class="flex min-h-32 flex-1 flex-col items-center justify-center gap-1 text-center">
                            <Icon name="layout-dashboard" class="size-5 text-muted-foreground" />
                            <p class="text-xs text-muted-foreground">{missing()}</p>
                        </div>
                    }
                    .into_any(),
                }
            }}
        </div>
    }
    .into_any()
}

/// The dashed notice a `Widget` field renders when it cannot host anything.
fn placeholder(message: impl Into<String>) -> AnyView {
    let message = message.into();
    view! {
        <div
            class="flex min-h-24 flex-col items-center justify-center gap-1 rounded-xl border border-dashed bg-muted/30 p-3 text-center"
            data-name="WidgetFieldNotice"
        >
            <Icon name="layout-dashboard" class="size-5 text-muted-foreground" />
            <p class="text-xs text-muted-foreground">{message}</p>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::{WidgetRef, parse_widget_ref};

    /// The documented spelling, plus the two separators a hand-written
    /// `Custom Field` may carry, all normalize to one owner + widget pair.
    #[test]
    fn an_app_widget_reference_names_its_owner() {
        for spelling in ["mi-app/tablero", "mi-app:tablero", "mi-app.tablero"] {
            assert_eq!(
                parse_widget_ref(spelling),
                WidgetRef::Named {
                    owner: "mi-app".to_string(),
                    widget: "tablero".to_string(),
                },
                "{spelling}"
            );
        }
        assert_eq!(
            parse_widget_ref("  mi-app / tablero  "),
            WidgetRef::Named {
                owner: "mi-app".to_string(),
                widget: "tablero".to_string(),
            }
        );
    }

    #[test]
    fn a_bare_id_is_resolved_by_the_feed_and_an_empty_option_is_nothing() {
        assert_eq!(
            parse_widget_ref("host.stat"),
            WidgetRef::Named {
                owner: "host".to_string(),
                widget: "stat".to_string(),
            },
            "a dotted host widget is an owner + name too"
        );
        assert_eq!(
            parse_widget_ref("tablero"),
            WidgetRef::Bare("tablero".to_string())
        );
        assert_eq!(parse_widget_ref("   "), WidgetRef::Empty);
        assert_eq!(
            parse_widget_ref("/tablero"),
            WidgetRef::Bare("/tablero".to_string())
        );
    }
}
