use crate::ext::{AppUi, WidgetDef, resolve_ref_in};
use ferris_schema::contract::WidgetParam;

use super::host::header_of;
#[cfg(feature = "ssr")]
use super::native::{HostRenderer, native_renderer};
use super::params::{HOST_OWNER, number_param, param_or};

fn def(name: &str, kind: &str) -> WidgetDef {
    WidgetDef {
        name: name.to_string(),
        label: format!("{name} label"),
        component: "some-widget".to_string(),
        kind: kind.to_string(),
        ..Default::default()
    }
}

fn app(owner: &str) -> AppUi {
    AppUi {
        owner: owner.to_string(),
        ..Default::default()
    }
}

/// The one resolution rule: a bare name resolves in the dashboard's own
/// catalog, `owner.name` in the named app's, and `host.*` in the host's
/// own — while a reference nothing carries resolves to nothing (the
/// placeholder's case), not to an error.
#[test]
fn widget_references_resolve_own_foreign_and_host() {
    let mut own = app("dash");
    own.widgets.push(def("local-stat", "stat"));
    let mut other = app("charts");
    other.widgets.push(def("trend", "chart"));
    let mut host = app(HOST_OWNER);
    host.widgets.push(def("stat", "stat"));
    let apps = vec![own, other, host];

    let local = resolve_ref_in(&apps, "dash", "local-stat")
        .expect("a bare name resolves in the dashboard's own catalog");
    assert_eq!(local.0, "dash");

    let foreign =
        resolve_ref_in(&apps, "dash", "charts.trend").expect("owner.name resolves across apps");
    assert_eq!(foreign.0, "charts");
    assert_eq!(foreign.1.kind, "chart");

    let host_resolved = resolve_ref_in(&apps, "dash", "host.stat")
        .expect("the host catalog resolves through host.*");
    assert_eq!(host_resolved.0, HOST_OWNER);

    assert!(resolve_ref_in(&apps, "dash", "nope").is_none());
    assert!(resolve_ref_in(&apps, "dash", "otherapp.nope").is_none());
}

/// Instance values win over the definition's declared defaults; an
/// absent or non-numeric value falls back to the renderer's fallback.
#[test]
fn instance_params_win_over_the_catalog_defaults() {
    let mut definition = def("table", "table");
    definition.params = vec![WidgetParam {
        name: "limit".to_string(),
        default: Some("10".to_string()),
        ..Default::default()
    }];

    let set = vec![("limit".to_string(), "3".to_string())];
    assert_eq!(param_or(&definition, &set, "limit"), Some("3"));
    assert_eq!(param_or(&definition, &[], "limit"), Some("10"));
    assert_eq!(param_or(&definition, &[], "order"), None);

    assert_eq!(number_param(&definition, &[], "limit", 7), 10);
    assert_eq!(number_param(&definition, &[], "days", 7), 7);
    let broken = vec![("limit".to_string(), "mucha".to_string())];
    assert_eq!(number_param(&definition, &broken, "limit", 7), 7);
}

/// The host owns the heading: the instance label wins, then a host
/// widget's `title` param, then the definition, then the reference;
/// `show_title` defaults to on and `show_subtitle` to off, with the
/// definition's description as the subtitle's fallback text.
#[test]
fn the_host_resolves_title_and_subtitle_with_its_defaults() {
    let mut definition = def("stat", "stat");
    definition.description = Some("Count of documents.".to_string());

    let base = header_of("host.stat", None, None, Some(&definition), None, None, None);
    assert_eq!(base.title, "stat label");
    assert!(base.show_title);
    assert!(!base.show_subtitle);
    assert_eq!(base.subtitle.as_deref(), Some("Count of documents."));

    let label_wins = header_of(
        "host.stat",
        Some("KPI"),
        Some("param"),
        Some(&definition),
        None,
        None,
        None,
    );
    assert_eq!(label_wins.title, "KPI");

    let param_wins = header_of(
        "host.stat",
        None,
        Some("param"),
        Some(&definition),
        None,
        None,
        None,
    );
    assert_eq!(param_wins.title, "param");

    let hidden = header_of(
        "host.stat",
        Some("KPI"),
        None,
        Some(&definition),
        Some("Live"),
        Some(false),
        None,
    );
    assert!(!hidden.show_title);
    assert!(!hidden.show_subtitle);
    assert_eq!(hidden.title, "KPI");

    let shown = header_of(
        "host.stat",
        None,
        None,
        Some(&definition),
        Some("Live"),
        None,
        Some(true),
    );
    assert!(shown.show_title);
    assert!(shown.show_subtitle);
    assert_eq!(shown.subtitle.as_deref(), Some("Live"));

    // No text at all: the reference is the last-resort heading and the
    // subtitle switch has nothing to show.
    let bare = header_of(
        "other.kpi",
        Some("   "),
        None,
        None,
        Some("  "),
        None,
        Some(true),
    );
    assert_eq!(bare.title, "other.kpi");
    assert!(bare.subtitle.is_none());
    assert!(!bare.show_subtitle);
}

/// Fase 2, §6: the renderers and the `HOST_WIDGETS` catalog
/// (`ferris_core::ui_extensions::host_widget_defs`, owner `host`) stay
/// in lockstep both ways. A catalog entry without a renderer would ship
/// [`unknown_host_widget`](super::placeholder::unknown_host_widget) to
/// production; a renderer without a catalog entry could never be reached.
/// `ferris-core` is the engine dep, which the platform-neutral Desk only
/// pulls under `ssr` — same gate as the `shell.rs` route-list test.
#[cfg(feature = "ssr")]
#[test]
fn every_host_catalog_widget_has_a_native_renderer_and_vice_versa() {
    let defs = ferris_core::ui_extensions::host_widget_defs();

    // Catalog → renderer: every declared `host.*` name dispatches.
    for def in &defs {
        assert!(
            native_renderer(&def.name).is_some(),
            "HOST_WIDGETS entry `{}` has no native renderer in views::widget_host",
            def.name
        );
    }

    // Renderer → catalog: every table entry maps its name back to the
    // same renderer and to a widget that is actually declared.
    for (renderer, name) in [
        (HostRenderer::Stat, "stat"),
        (HostRenderer::Trend, "trend"),
        (HostRenderer::Table, "table"),
    ] {
        assert_eq!(
            native_renderer(name),
            Some(renderer),
            "the dispatch table lost `{name}`"
        );
        assert!(
            defs.iter().any(|def| def.name == name),
            "renderer `{name}` has no HOST_WIDGETS entry"
        );
    }
}
