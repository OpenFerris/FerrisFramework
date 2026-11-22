//! [`WidgetHost`]: one widget instance → its body in the dashboard grid.

use leptos::prelude::*;

use crate::ext::{UiCtx, WidgetDef};

use super::code::{CodeWidget, ResolvedWidget};
use super::native::host_widget;
use super::params::{HOST_OWNER, param_or};
use super::placeholder::{missing_code_widget, unresolved_widget};

/// One widget instance in a dashboard grid: resolve `reference` against the
/// feed's catalog and render whichever body its owner implies — the host's
/// native renderer, the plugin's custom element, or the unresolved
/// placeholder. The tile that positions this body is the caller's
/// (`views::dashboard::DashboardTile`), which is why the root is just a
/// full-height box.
///
/// The host also owns the **heading**: the card frame is the tile
/// (`--ferris-tile-*`), and the title/subtitle are painted here, above the
/// body, from the instance's configuration ([`WidgetInstance::show_title`],
/// [`WidgetInstance::subtitle`], [`WidgetInstance::show_subtitle`]). The body
/// is pure content: a plugin element, a native renderer or a placeholder,
/// never a second card and never its own title. That keeps the app from
/// declaring padding/background/border (the card's job) and keeps the title
/// consistent across native and plugin widgets.
///
/// [`WidgetInstance::show_title`]: ferris_schema::contract::WidgetInstance::show_title
/// [`WidgetInstance::subtitle`]: ferris_schema::contract::WidgetInstance::subtitle
/// [`WidgetInstance::show_subtitle`]: ferris_schema::contract::WidgetInstance::show_subtitle
#[component]
pub fn WidgetHost(
    /// App that owns the dashboard: a bare `reference` resolves in its catalog.
    default_owner: String,
    /// The reference as declared: `owner.name`, or bare `name` against
    /// `default_owner`.
    reference: String,
    /// Instance id, unique within the dashboard (`data-instance` on the box).
    #[prop(optional)]
    instance_id: String,
    /// Display label: the instance's override, else the definition's.
    #[prop(optional_no_strip)]
    label: Option<String>,
    /// Subtitle text the host paints under the title. Absent or empty falls
    /// back to the definition's description; it is only drawn when
    /// `show_subtitle` is on.
    #[prop(optional_no_strip)]
    subtitle: Option<String>,
    /// Whether to paint the title. Absent means yes.
    #[prop(optional_no_strip)]
    show_title: Option<bool>,
    /// Whether to paint the subtitle. Absent means no (subtitles are opt-in);
    /// requesting one without a text falls back to the definition's
    /// description, and with neither the header simply omits it.
    #[prop(optional_no_strip)]
    show_subtitle: Option<bool>,
    /// Instance parameter values; the definition supplies its own defaults
    /// for the ones the instance left out.
    #[prop(optional)]
    params: Vec<(String, String)>,
    /// Bumped by the dashboard on a timer: a host renderer re-runs its fetch
    /// whenever it changes. `None` (a static embed) never refreshes.
    #[prop(optional_no_strip)]
    refresh: Option<Signal<u32>>,
) -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let resolved = ctx.resolve_widget_ref(&default_owner, &reference);

    // Host widgets have a `title` presentation param. It is the fallback
    // between the instance's label (the settings panel's "Título") and the
    // definition's label, so a seed that only set the param keeps its heading
    // while the Desk's title control keeps working.
    let host_title = resolved
        .as_ref()
        .filter(|(owner, _)| owner == HOST_OWNER)
        .and_then(|(_, def)| param_or(def, &params, "title"))
        .filter(|title| !title.is_empty());
    let header = header_of(
        &reference,
        label.as_deref(),
        host_title,
        resolved.as_ref().map(|(_, def)| def),
        subtitle.as_deref(),
        show_title,
        show_subtitle,
    );

    let body: AnyView = match resolved {
        None => unresolved_widget(&reference),
        Some((owner, def)) if owner == HOST_OWNER => host_widget(&def, &params, refresh),
        Some((owner, def)) => {
            if def.code_url.is_none() {
                missing_code_widget(&reference)
            } else {
                view! {
                    <CodeWidget
                        widget=ResolvedWidget {
                            owner,
                            reference: reference.clone(),
                            instance_id: instance_id.clone(),
                            label: header.title.clone(),
                            kind: def.kind.clone(),
                            component: def.component.clone(),
                            code_url: def.code_url.clone(),
                            style_url: def.style_url.clone(),
                            params: params.clone(),
                        }
                    />
                }
                .into_any()
            }
        }
    };

    let show_head = header.show_title || header.show_subtitle;
    let head_title = header.title;
    let head_subtitle = header.subtitle;
    let show_title = header.show_title;
    let show_subtitle = header.show_subtitle;

    view! {
        <div class="flex h-full min-w-0 flex-col" data-widget=reference data-instance=instance_id>
            {show_head.then(|| {
                let title = head_title.clone();
                let subtitle = head_subtitle.clone().unwrap_or_default();
                view! {
                    <header class="flex shrink-0 flex-col gap-0.5 pb-2">
                        {show_title.then(|| view! {
                            <h2
                                class="truncate text-sm font-semibold leading-tight text-foreground"
                                title=title.clone()
                            >
                                {title.clone()}
                            </h2>
                        })}
                        {show_subtitle.then(|| view! {
                            <p
                                class="truncate text-xs leading-tight text-muted-foreground"
                                title=subtitle.clone()
                            >
                                {subtitle.clone()}
                            </p>
                        })}
                    </header>
                }
            })}
            <div class="min-h-0 min-w-0 flex-1">{body}</div>
        </div>
    }
}

/// The host-painted heading of one instance: the text it resolved and whether
/// each line is visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WidgetHeader {
    pub(super) title: String,
    pub(super) subtitle: Option<String>,
    pub(super) show_title: bool,
    pub(super) show_subtitle: bool,
}

/// Resolve the host-owned heading of one instance.
///
/// The host — never the app — decides the chrome. Precedence for the title is
/// the instance's label, then the host widget's `title` param, then the
/// definition's label, then the reference. The subtitle is the instance's text
/// or, when absent, the definition's description. `show_title` defaults to
/// true (the host has always painted a heading) and `show_subtitle` to false
/// (a subtitle is opt-in; the definition's description is its default text).
/// Empty strings are treated as absent, so a blank input never renders a blank
/// line or silently hides the fallback.
pub(super) fn header_of(
    reference: &str,
    label: Option<&str>,
    host_title: Option<&str>,
    def: Option<&WidgetDef>,
    subtitle: Option<&str>,
    show_title: Option<bool>,
    show_subtitle: Option<bool>,
) -> WidgetHeader {
    let clean = |text: Option<&str>| {
        text.map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let title = clean(label)
        .or_else(|| clean(host_title))
        .or_else(|| def.and_then(|def| clean(Some(def.label.as_str()))))
        .unwrap_or_else(|| reference.to_string());
    let subtitle =
        clean(subtitle).or_else(|| def.and_then(|def| clean(def.description.as_deref())));
    WidgetHeader {
        title,
        show_title: show_title.unwrap_or(true),
        show_subtitle: show_subtitle.unwrap_or(false) && subtitle.is_some(),
        subtitle,
    }
}
