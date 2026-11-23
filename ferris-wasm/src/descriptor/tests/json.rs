//! `DescriptorJson`: the wire shape `/plugins/{name}` serves.

use super::support::{bare, wit_doctype};
use crate::descriptor::json::DescriptorJson;
use crate::ferris::framework::descriptor::{
    Descriptor, Folder, Method, Route, RouteKind, Theme, WebsiteRoute,
};
use crate::ferris::framework::types::{
    ClientAuth, HttpPolicy, RedirectPolicy, RouteAuth, StreamListen, StreamTransport, TlsClient,
    TlsMaterial, TlsServer, TlsVersion,
};

/// The dev endpoint's projection keeps the pre-13 wire shape when no optional
/// section is used: unused `option<>` slots stay silent, and the network
/// surface is always present but empty.
#[test]
fn declared_locales_are_projected_and_absent_ones_stay_silent() {
    let mut described = filled_extension_fixture();
    described.locales = Some(vec!["en".to_string(), "es".to_string()]);
    let json = serde_json::to_value(DescriptorJson::from(&described)).expect("serializes");
    assert_eq!(json.get("locales"), Some(&serde_json::json!(["en", "es"])));

    described.locales = None;
    let json = serde_json::to_value(DescriptorJson::from(&described)).expect("serializes");
    assert!(json.get("locales").is_none());
}

fn descriptor_json_keeps_the_base_shape_when_slots_are_empty() {
    let described = Descriptor {
        locales: None,
        capabilities: Vec::new(),
        routes: Vec::new(),
        auth_mode: RouteAuth::None,
        http_policy: None,
        tls_client: None,
        tls_server: None,
        doctypes: vec![wit_doctype("Task", vec![bare("subject")])],
        themes: vec![Theme {
            id: "dark".into(),
            label: "Dark".into(),
            dark: true,
            vars: vec![("--background".into(), "oklch(0.1 0 0)".into())],
        }],
        folders: vec![Folder {
            name: "ops".into(),
            label: "Ops".into(),
            icon: None,
        }],
        workspaces: Vec::new(),
        methods: vec![Method {
            name: "ping".into(),
            params: vec![("a".into(), "string".into())],
            returns: "string".into(),
            description: "Ping".into(),
            overrides: false,
        }],
        ui_extensions: None,
        dashboards: None,
        widgets: None,
        website: None,
        assets: None,
        subscriptions: None,
        config: None,
        plugin_version: None,
        requires: None,
    };
    let json = serde_json::to_value(DescriptorJson::from(&described)).unwrap();
    let obj = json.as_object().expect("projection must be a JSON object");
    // The always-present section.
    assert!(obj.contains_key("capabilities"));
    assert!(obj.contains_key("routes"));
    assert!(obj.contains_key("auth_mode"));
    assert!(obj.contains_key("doctypes"));
    assert!(obj.contains_key("themes"));
    assert!(obj.contains_key("folders"));
    assert!(obj.contains_key("workspaces"));
    assert!(obj.contains_key("methods"));
    // The extension slots never appear unless the app fills them.
    assert!(!obj.contains_key("http_policy"));
    assert!(!obj.contains_key("tls_client"));
    assert!(!obj.contains_key("tls_server"));
    assert!(!obj.contains_key("ui_extensions"));
    assert!(!obj.contains_key("dashboards"));
    assert!(!obj.contains_key("widgets"));
    assert!(!obj.contains_key("website"));
    assert!(!obj.contains_key("assets"));
    assert!(!obj.contains_key("subscriptions"));
    assert!(!obj.contains_key("plugin_version"));
    assert!(!obj.contains_key("requires"));
}

/// A `Descriptor` with every optional section filled: capabilities, routes of
/// both kinds, the HTTP/TLS defaults, a dashboard with a widget instance, the
/// widget catalog definition, website routes, assets, subscriptions and version
/// metadata.
///
/// Built on demand so the literal does not live inside the test that reads it
/// back.
fn filled_extension_fixture() -> Descriptor {
    Descriptor {
        locales: None,
        capabilities: vec![
            crate::ferris::framework::types::Capability::Routes,
            crate::ferris::framework::types::Capability::Http,
        ],
        routes: vec![
            Route {
                id: "invoices".into(),
                method: "POST".into(),
                path: "/invoices".into(),
                kind: RouteKind::Http,
                auth: Some(RouteAuth::Session),
                body_limit: Some(1_048_576),
                tls_server: None,
                listener: None,
            },
            Route {
                id: "sensors".into(),
                method: "*".into(),
                path: "/sensors".into(),
                kind: RouteKind::Stream,
                auth: None,
                body_limit: None,
                tls_server: None,
                listener: Some(StreamListen {
                    transport: StreamTransport::Websocket,
                    address: Some("/sensors".into()),
                    port: None,
                    tls: None,
                    auth: Some(RouteAuth::Session),
                    max_frame: Some(65_536),
                    idle_timeout_seconds: None,
                }),
            },
        ],
        auth_mode: RouteAuth::Session,
        http_policy: Some(HttpPolicy {
            allowed_hosts: vec!["api.example.com".into()],
            allow_private: false,
            max_body: Some(16_777_216),
            timeout_ms: Some(5_000),
            redirects: RedirectPolicy::Limited(3),
            allow_credentials: false,
        }),
        tls_client: Some(TlsClient {
            cert: None,
            key: None,
            ca: Some(TlsMaterial::Secret("ca.pem".into())),
            server_name: Some("peer.example.com".into()),
            alpn: vec!["h2".into()],
            min_version: TlsVersion::Tls13,
            verify: true,
        }),
        tls_server: Some(TlsServer {
            cert: TlsMaterial::Pem("-----BEGIN CERTIFICATE-----".into()),
            key: TlsMaterial::Pem("-----BEGIN PRIVATE KEY-----".into()),
            ca: Some(TlsMaterial::Secret("client-ca.pem".into())),
            client_auth: ClientAuth::Required,
            sni: vec!["app.example.com".into()],
            alpn: Vec::new(),
            min_version: TlsVersion::Tls12,
        }),
        doctypes: Vec::new(),
        themes: Vec::new(),
        folders: Vec::new(),
        workspaces: Vec::new(),
        methods: Vec::new(),
        ui_extensions: Some(vec![crate::ferris::framework::descriptor::UiExtension {
            slot: "form.toolbar.right".into(),
            doctype: Some("Invoice".into()),
            component: "invoice-actions".into(),
            label: Some("Acciones".into()),
            props: vec![("mode".into(), "compact".into())],
            requires: None,
        }]),
        dashboards: Some(vec![crate::ferris::framework::descriptor::Dashboard {
            id: "sales-overview".into(),
            label: "Ventas".into(),
            route: "/sales-overview".into(),
            default: true,
            style: Some(crate::ferris::framework::descriptor::DashboardStyle {
                background: Some("muted".into()),
                background_custom: None,
                gap_px: Some(20),
                padding_px: None,
                border_radius: None,
                title_full_screen: None,
            }),
            widgets: vec![crate::ferris::framework::descriptor::WidgetInstance {
                id: "monthly".into(),
                widget: "sales-chart".into(),
                label: Some("Mes".into()),
                params: vec![("doctype".into(), "Invoice".into())],
                span: Some(6),
                col: Some(0),
                row: Some(1),
                rows: Some(4),
                style: Some(crate::ferris::framework::descriptor::StyleSpec {
                    background: None,
                    background_custom: Some("oklch(0.4 0.1 20)".into()),
                    border_color: None,
                    border_width: None,
                    border_radius: None,
                    padding: None,
                    shadow: None,
                }),
            }],
        }]),
        widgets: Some(vec![crate::ferris::framework::descriptor::WidgetDef {
            name: "sales-chart".into(),
            label: "Ventas".into(),
            component: "sales-chart".into(),
            code_path: None,
            script: None,
            stylesheet: None,
            ssr: None,
            renderer: None,
            version: None,
            kind: "chart".into(),
            params: vec![crate::ferris::framework::descriptor::WidgetParam {
                name: "doctype".into(),
                label: Some("DocType".into()),
                value_type: "doctype".into(),
                default: None,
                options: Vec::new(),
                required: true,
                description: None,
                group: Some("Data".into()),
                placeholder: None,
                depends_on: None,
                read_only: false,
                hidden: false,
            }],
            description: Some("Ventas por mes".into()),
            requires: None,
            events: Vec::new(),
            icon: Some("📈".into()),
            category: Some("Charts".into()),
            preview: None,
            size: Some(crate::ferris::framework::descriptor::GridSize { span: 6, rows: 4 }),
            min: Some(crate::ferris::framework::descriptor::GridSize { span: 3, rows: 3 }),
            style: Some(crate::ferris::framework::descriptor::StyleSpec {
                background: Some("card".into()),
                background_custom: None,
                border_color: None,
                border_width: None,
                border_radius: None,
                padding: Some(12),
                shadow: None,
            }),
        }]),
        // The website record is the route list itself.
        website: Some(vec![WebsiteRoute {
            path: "/blog".into(),
            component: "blog-index".into(),
            kind: "page".into(),
        }]),
        assets: Some(vec![
            "ui/invoice-actions.js".to_string(),
            "ui/ui.css".to_string(),
        ]),
        subscriptions: Some(vec![crate::ferris::framework::descriptor::Subscription {
            channel: "sensor:east".into(),
        }]),
        config: None,
        plugin_version: Some("0.1.0".into()),
        requires: Some(vec!["print".to_string()]),
    }
}

/// Filled extension slots serialize; fields mirror the typed record.
#[test]
fn descriptor_json_emits_filled_extension_slots() {
    let described = filled_extension_fixture();
    let json = serde_json::to_value(DescriptorJson::from(&described)).unwrap();
    let obj = json.as_object().unwrap();
    // The network surface, projected from the declaration.
    assert_eq!(obj["capabilities"][0], "routes");
    assert_eq!(obj["capabilities"][1], "http");
    assert_eq!(obj["auth_mode"], "session");
    assert_eq!(obj["routes"][0]["id"], "invoices");
    assert_eq!(obj["routes"][0]["kind"], "http");
    assert_eq!(obj["routes"][0]["auth"], "session");
    assert_eq!(obj["routes"][0]["body_limit"], 1_048_576u32);
    assert_eq!(obj["routes"][1]["kind"], "stream");
    assert_eq!(obj["routes"][1]["listener"], "/sensors");
    assert_eq!(obj["http_policy"]["allowed_hosts"][0], "api.example.com");
    assert_eq!(obj["http_policy"]["redirects"], "limited");
    assert_eq!(obj["tls_client"]["min_version"], "tls13");
    assert_eq!(obj["tls_server"]["client_auth"], "required");
    // The Desk surface is unchanged.
    assert_eq!(obj["ui_extensions"][0]["slot"], "form.toolbar.right");
    assert_eq!(obj["ui_extensions"][0]["doctype"], "Invoice");
    assert_eq!(obj["ui_extensions"][0]["component"], "invoice-actions");
    assert_eq!(obj["ui_extensions"][0]["props"][0][1], "compact");
    assert_eq!(obj["dashboards"][0]["id"], "sales-overview");
    assert_eq!(obj["dashboards"][0]["route"], "/sales-overview");
    assert_eq!(obj["dashboards"][0]["default"], true);
    // The instance carries the reference + config; the definition (with
    // the widget kind as `"type"`) lives in the global `widgets` catalog.
    assert_eq!(obj["dashboards"][0]["widgets"][0]["widget"], "sales-chart");
    assert_eq!(obj["dashboards"][0]["widgets"][0]["span"], 6);
    assert_eq!(obj["widgets"][0]["type"], "chart");
    assert_eq!(obj["widgets"][0]["params"][0]["value-type"], "doctype");
    assert_eq!(obj["widgets"][0]["params"][0]["required"], true);
    // The 10.0.0 style wave reaches the JSON: the definition's declared
    // default, the instance's override and the board's own style, each
    // under the wire key the Desk reads.
    assert_eq!(obj["widgets"][0]["style"]["background"], "card");
    assert_eq!(obj["widgets"][0]["style"]["padding"], 12);
    assert_eq!(
        obj["dashboards"][0]["widgets"][0]["style"]["background_custom"],
        "oklch(0.4 0.1 20)"
    );
    assert_eq!(obj["dashboards"][0]["style"]["background"], "muted");
    assert_eq!(obj["dashboards"][0]["style"]["gap_px"], 20);
    assert_eq!(obj["website"]["routes"][0]["path"], "/blog");
    assert_eq!(obj["website"]["routes"][0]["type"], "page");
    assert_eq!(obj["assets"][0], "ui/invoice-actions.js");
    assert_eq!(obj["subscriptions"][0]["channel"], "sensor:east");
    assert_eq!(obj["plugin_version"], "0.1.0");
    assert_eq!(obj["requires"][0], "print");
}

/// A private key declared inline is **redacted** from the projection: the WIT
/// contract promises `/plugins/{name}` cannot leak TLS material, so the JSON
/// only says where it comes from.
#[test]
fn descriptor_json_redacts_inline_tls_material() {
    let described = filled_extension_fixture();
    let json = serde_json::to_string(&DescriptorJson::from(&described)).unwrap();
    assert!(
        !json.contains("BEGIN PRIVATE KEY"),
        "inline key material leaked into the descriptor projection: {json}"
    );
    assert!(
        !json.contains("BEGIN CERTIFICATE"),
        "inline certificate material leaked into the descriptor projection: {json}"
    );
    let obj = serde_json::to_value(DescriptorJson::from(&described)).unwrap();
    assert_eq!(obj["tls_server"]["key"], "pem:<redacted>");
    assert_eq!(obj["tls_server"]["ca"], "secret:client-ca.pem");
}
