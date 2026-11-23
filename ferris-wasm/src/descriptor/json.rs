//! The stable JSON projection the dev endpoint `/plugins/{name}` serves.
//!
//! Every mirror here is serialization-only: the plugin boundary is the typed
//! WIT record, and this module is what `/plugins/{name}` prints. It projects
//! the network surface (`capabilities`, `routes`, `auth-mode`, the HTTP/TLS
//! defaults). It is projected here with **every piece of inline TLS material
//! redacted**, because the WIT contract promises a key cannot leak through a
//! descriptor read.

use serde::Serialize;

use ferris_core::themes::ThemeSpec;
use ferris_core::ui_extensions::{DashboardSpec, UiExtensionSpec, WebsiteSpec, WidgetDefSpec};
use ferris_core::workspaces::{WorkspaceFolderSpec, WorkspaceSpec};

use super::doctype::ConfigDoctype;
use super::method::MethodSpec;
use super::registries::{
    dashboard_spec, ui_extension_spec, website_spec, widget_def_spec, workspace_spec,
};
use crate::ferris::framework::descriptor::{
    ConfigItem, Descriptor, Route, RouteKind, TlsClient, TlsServer,
};
use crate::ferris::framework::types::{
    ClientAuth, HttpPolicy, RedirectPolicy, RouteAuth, TlsMaterial, TlsVersion,
};

/// Extension slot: a pubsub channel the app participates in.
#[derive(Debug, Clone, Serialize)]
pub struct SubscriptionJson {
    pub channel: String,
}

/// One declared config key (mirror of `descriptor::config-item`).
///
/// This is the *schema only* — the projected JSON never carries stored values,
/// so a `secret` key's value cannot leak through a descriptor read.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigJson {
    pub key: String,
    pub label: Option<String>,
    pub value_type: String,
    pub required: bool,
    pub default: Option<String>,
    pub description: Option<String>,
}

impl From<&ConfigItem> for ConfigJson {
    fn from(item: &ConfigItem) -> Self {
        Self {
            key: item.key.clone(),
            label: item.label.clone(),
            value_type: item.value_type.clone(),
            required: item.required,
            default: item.default.clone(),
            description: item.description.clone(),
        }
    }
}

/// One mounted route (mirror of `descriptor::route`). The listener and the TLS
/// material are summarized as booleans/ids: the endpoint advertises the
/// surface, not the secrets behind it.
#[derive(Debug, Clone, Serialize)]
pub struct RouteJson {
    pub id: String,
    pub method: String,
    pub path: String,
    pub kind: String,
    pub auth: String,
    pub body_limit: Option<u32>,
    pub tls_server: bool,
    pub listener: Option<String>,
}

/// The app-wide outbound policy (mirror of `types::http-policy`).
#[derive(Debug, Clone, Serialize)]
pub struct HttpPolicyJson {
    pub allowed_hosts: Vec<String>,
    pub allow_private: bool,
    pub max_body: Option<u64>,
    pub timeout_ms: Option<u32>,
    pub redirects: String,
    pub allow_credentials: bool,
}

/// The app-wide outbound TLS defaults (mirror of `types::tls-client`), with
/// material reduced to its source.
#[derive(Debug, Clone, Serialize)]
pub struct TlsClientJson {
    pub cert: Option<String>,
    pub key: Option<String>,
    pub ca: Option<String>,
    pub server_name: Option<String>,
    pub alpn: Vec<String>,
    pub min_version: String,
    pub verify: bool,
}

/// The app-wide inbound TLS defaults (mirror of `types::tls-server`), with
/// material reduced to its source.
#[derive(Debug, Clone, Serialize)]
pub struct TlsServerJson {
    pub cert: String,
    pub key: String,
    pub ca: Option<String>,
    pub client_auth: String,
    pub sni: Vec<String>,
    pub alpn: Vec<String>,
    pub min_version: String,
}

/// The dev endpoint's view of the descriptor: a host-built, stable projection
/// of the typed contract.
///
/// Only the extension slots that are `Some` are emitted, so a guest that uses
/// none of them keeps the pre-13 JSON surface (`doctypes`, `themes`,
/// `folders`, `workspaces`, `methods`).
#[derive(Debug, Clone, Serialize)]
pub struct DescriptorJson {
    /// What the app asked the operator for (`describe().capabilities`).
    pub capabilities: Vec<String>,
    /// The surface the host mounts.
    pub routes: Vec<RouteJson>,
    /// The default auth of routes that declare none.
    pub auth_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_policy: Option<HttpPolicyJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_client: Option<TlsClientJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_server: Option<TlsServerJson>,
    pub doctypes: Vec<ConfigDoctype>,
    pub themes: Vec<ThemeSpec>,
    pub folders: Vec<WorkspaceFolderSpec>,
    pub workspaces: Vec<WorkspaceSpec>,
    pub methods: Vec<MethodSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_extensions: Option<Vec<UiExtensionSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboards: Option<Vec<DashboardSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widgets: Option<Vec<WidgetDefSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website: Option<WebsiteSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assets: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscriptions: Option<Vec<SubscriptionJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Vec<ConfigJson>>,
    /// Locales the app declares (`descriptor.locales`); the catalogues travel
    /// in the package under `locales/<tag>.toml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locales: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<Vec<String>>,
}

impl From<&Descriptor> for DescriptorJson {
    fn from(described: &Descriptor) -> Self {
        Self {
            capabilities: described
                .capabilities
                .iter()
                .map(|capability| {
                    crate::host::Capability::from_wit(*capability)
                        .name()
                        .to_string()
                })
                .collect(),
            routes: described.routes.iter().map(route_json).collect(),
            auth_mode: route_auth_name(&described.auth_mode).to_string(),
            http_policy: described.http_policy.as_ref().map(http_policy_json),
            tls_client: described.tls_client.as_ref().map(tls_client_json),
            tls_server: described.tls_server.as_ref().map(tls_server_json),
            doctypes: described.doctypes.iter().map(ConfigDoctype::from).collect(),
            themes: described.themes.iter().map(ThemeSpec::from).collect(),
            folders: described
                .folders
                .iter()
                .map(WorkspaceFolderSpec::from)
                .collect(),
            workspaces: described.workspaces.iter().map(workspace_spec).collect(),
            methods: described.methods.iter().map(MethodSpec::from).collect(),
            ui_extensions: described
                .ui_extensions
                .as_ref()
                .map(|extensions| extensions.iter().map(ui_extension_spec).collect()),
            dashboards: described
                .dashboards
                .as_ref()
                .map(|dashboards| dashboards.iter().map(dashboard_spec).collect()),
            widgets: described
                .widgets
                .as_ref()
                .map(|widgets| widgets.iter().map(widget_def_spec).collect()),
            website: described.website.as_deref().map(website_spec),
            assets: described.assets.clone(),
            subscriptions: described.subscriptions.as_ref().map(|subscriptions| {
                subscriptions
                    .iter()
                    .map(|s| SubscriptionJson {
                        channel: s.channel.clone(),
                    })
                    .collect()
            }),
            config: described
                .config
                .as_ref()
                .map(|items| items.iter().map(ConfigJson::from).collect()),
            locales: described.locales.clone(),
            plugin_version: described.plugin_version.clone(),
            requires: described.requires.clone(),
        }
    }
}

// -- route projections ------------------------------------------------------

fn route_json(route: &Route) -> RouteJson {
    RouteJson {
        id: route.id.clone(),
        method: route.method.clone(),
        path: route.path.clone(),
        kind: match route.kind {
            RouteKind::Http => "http",
            RouteKind::Stream => "stream",
        }
        .to_string(),
        auth: route.auth.as_ref().map_or_else(
            || "inherit".to_string(),
            |auth| route_auth_name(auth).to_string(),
        ),
        body_limit: route.body_limit,
        tls_server: route.tls_server.is_some(),
        listener: route
            .listener
            .as_ref()
            .and_then(|listener| listener.address.clone()),
    }
}

const fn route_auth_name(auth: &RouteAuth) -> &'static str {
    match auth {
        RouteAuth::None => "none",
        RouteAuth::Session => "session",
        // The token's config key name is not a secret, but it is not part of
        // the public projection either: the Desk only needs the kind.
        RouteAuth::Capability(_) => "capability",
    }
}

fn http_policy_json(policy: &HttpPolicy) -> HttpPolicyJson {
    HttpPolicyJson {
        allowed_hosts: policy.allowed_hosts.clone(),
        allow_private: policy.allow_private,
        max_body: policy.max_body,
        timeout_ms: policy.timeout_ms,
        redirects: redirect_policy_name(policy.redirects).to_string(),
        allow_credentials: policy.allow_credentials,
    }
}

const fn redirect_policy_name(policy: RedirectPolicy) -> &'static str {
    match policy {
        RedirectPolicy::None => "none",
        RedirectPolicy::Limited(_) => "limited",
        RedirectPolicy::Follow => "follow",
    }
}

const fn tls_version_name(version: TlsVersion) -> &'static str {
    match version {
        TlsVersion::Tls12 => "tls12",
        TlsVersion::Tls13 => "tls13",
    }
}

const fn client_auth_name(auth: ClientAuth) -> &'static str {
    match auth {
        ClientAuth::None => "none",
        ClientAuth::Optional => "optional",
        ClientAuth::Required => "required",
    }
}

/// How one piece of TLS material is sourced — **never** its value.
///
/// An inline `pem` is reported as `<redacted>`: the plugin projects this JSON
/// at `/plugins/{name}`, and a private key must not travel with it.
fn material_source(material: &TlsMaterial) -> String {
    match material {
        TlsMaterial::Pem(_) => "pem:<redacted>".to_string(),
        TlsMaterial::Secret(key) => format!("secret:{key}"),
    }
}

fn tls_client_json(client: &TlsClient) -> TlsClientJson {
    TlsClientJson {
        cert: client.cert.as_ref().map(material_source),
        key: client.key.as_ref().map(material_source),
        ca: client.ca.as_ref().map(material_source),
        server_name: client.server_name.clone(),
        alpn: client.alpn.clone(),
        min_version: tls_version_name(client.min_version).to_string(),
        verify: client.verify,
    }
}

fn tls_server_json(server: &TlsServer) -> TlsServerJson {
    TlsServerJson {
        cert: material_source(&server.cert),
        key: material_source(&server.key),
        ca: server.ca.as_ref().map(material_source),
        client_auth: client_auth_name(server.client_auth).to_string(),
        sni: server.sni.clone(),
        alpn: server.alpn.clone(),
        min_version: tls_version_name(server.min_version).to_string(),
    }
}
