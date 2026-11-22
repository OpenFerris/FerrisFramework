//! Diagnostics: parse the framework WIT with `wit-parser` to surface layout
//! problems fast (the `bindgen!` macro reports them opaquely).
//!
//! This doubles as a contract guard for the plugin boundary: every host
//! service the guest can import must be declared in the world, every world
//! export the host forwards to must be advertised here, and the shapes the
//! guest compiles against (capabilities, routes, TLS policy, the stream and
//! datagram lifecycle, host features, the app world
//! and the method-override chain) are pinned field by field, so a host-side
//! rename cannot drift them silently.

use std::collections::BTreeMap;

use wit_parser::{Resolve, Type, TypeDefKind, TypeOwner, WorldItem, WorldKey};

/// Package version every copy of the contract must declare.
const PACKAGE: &str = "package ferris:framework@0.3.0;";

/// The interfaces the contract declares, in declaration order.
const SERVICES: [&str; 6] = ["types", "data", "schema", "io", "platform", "descriptor"];

/// Flatten a world's imports and exports into a `name -> item` map, resolving
/// the display name from either key form (`WorldKey::Name` for named/inline
/// items, or the interface's own name for `WorldKey::Interface`).
///
/// A `use descriptor.{descriptor}` re-exports the record as a world-level type
/// alias that shares its name with the interface import, so an interface entry
/// always wins the collision: the alias is the *record*, the import is the
/// service.
fn items<'a>(ctx: &'a Resolve, world: &'a wit_parser::World) -> BTreeMap<String, &'a WorldItem> {
    let mut map: BTreeMap<String, &'a WorldItem> = BTreeMap::new();
    for (key, item) in world.imports.iter().chain(world.exports.iter()) {
        let name = match key {
            WorldKey::Name(name) => name.clone(),
            WorldKey::Interface(id) => ctx.interfaces[*id]
                .name
                .clone()
                .unwrap_or_else(|| "(inline interface)".to_string()),
        };
        if matches!(map.get(&name), Some(WorldItem::Interface { .. })) {
            continue;
        }
        map.insert(name, item);
    }
    map
}

/// Display names of the functions exported by an imported service interface.
fn interface_fns<'a>(
    ctx: &'a Resolve,
    items: &BTreeMap<String, &'a WorldItem>,
    service: &str,
) -> Vec<&'a str> {
    let item = items
        .get(service)
        .unwrap_or_else(|| panic!("import `{service}` missing from the world"));
    match item {
        WorldItem::Interface { id, .. } => ctx.interfaces[*id]
            .functions
            .keys()
            .map(String::as_str)
            .collect(),
        other => panic!("`{service}` is not an interface import: {other:?}"),
    }
}

/// The interface behind an imported world item.
fn iface<'a>(
    ctx: &'a Resolve,
    items: &BTreeMap<String, &'a WorldItem>,
    service: &str,
) -> &'a wit_parser::Interface {
    match items.get(service) {
        Some(WorldItem::Interface { id, .. }) => &ctx.interfaces[*id],
        other => panic!("`{service}` is not an interface import: {other:?}"),
    }
}

/// One function of an interface, panicking with its name when absent.
fn func<'a>(iface: &'a wit_parser::Interface, name: &str) -> &'a wit_parser::Function {
    iface
        .functions
        .get(name)
        .unwrap_or_else(|| panic!("`{name}` missing from the interface"))
}

/// The interface named `name` anywhere in the resolve (for `descriptor`, which
/// the world imports for its types).
fn interface<'a>(ctx: &'a Resolve, name: &str) -> &'a wit_parser::Interface {
    ctx.interfaces
        .iter()
        .find(|(_, i)| i.name.as_deref() == Some(name))
        .map(|(_, i)| i)
        .unwrap_or_else(|| panic!("interface `{name}` missing from the resolve"))
}

/// `(name, rendered type)` pairs of a record's fields, in declaration order.
fn record_fields<'a>(
    ctx: &'a Resolve,
    iface: &'a wit_parser::Interface,
    name: &str,
) -> Vec<(&'a str, String)> {
    let def = iface
        .types
        .get(name)
        .unwrap_or_else(|| panic!("record `{name}` missing from the interface"));
    let TypeDefKind::Record(record) = &ctx.types[*def].kind else {
        panic!("`{name}` is not a record");
    };
    record
        .fields
        .iter()
        .map(|f| (f.name.as_str(), type_str(ctx, &f.ty)))
        .collect()
}

/// `(name, rendered payload)` pairs of a variant's cases, in declaration order
/// (`None` for a payload-less case).
fn variant_cases<'a>(
    ctx: &'a Resolve,
    iface: &'a wit_parser::Interface,
    name: &str,
) -> Vec<(&'a str, Option<String>)> {
    let def = iface
        .types
        .get(name)
        .unwrap_or_else(|| panic!("variant `{name}` missing from the interface"));
    let TypeDefKind::Variant(variant) = &ctx.types[*def].kind else {
        panic!("`{name}` is not a variant");
    };
    variant
        .cases
        .iter()
        .map(|case| {
            (
                case.name.as_str(),
                case.ty.as_ref().map(|t| type_str(ctx, t)),
            )
        })
        .collect()
}

/// Case names of an enum, in declaration order.
fn enum_cases<'a>(ctx: &'a Resolve, iface: &'a wit_parser::Interface, name: &str) -> Vec<&'a str> {
    let def = iface
        .types
        .get(name)
        .unwrap_or_else(|| panic!("enum `{name}` missing from the interface"));
    let TypeDefKind::Enum(enumeration) = &ctx.types[*def].kind else {
        panic!("`{name}` is not an enum");
    };
    enumeration.cases.iter().map(|c| c.name.as_str()).collect()
}

/// `(name, rendered type)` pairs of a function's parameters.
fn param_strs<'a>(ctx: &'a Resolve, f: &'a wit_parser::Function) -> Vec<(&'a str, String)> {
    f.params
        .iter()
        .map(|p| (p.name.as_str(), type_str(ctx, &p.ty)))
        .collect()
}

/// The rendered ok/err types of a `result<ok, err>` return (`None` for `_`).
fn result_types<'a>(
    ctx: &'a Resolve,
    f: &'a wit_parser::Function,
) -> (Option<String>, Option<String>) {
    let ret = f
        .result
        .unwrap_or_else(|| panic!("`{}` returns nothing", f.name));
    let Type::Id(id) = ret else {
        panic!("`{}` does not return a result: {ret:?}", f.name);
    };
    let TypeDefKind::Result(res) = &ctx.types[id].kind else {
        panic!("`{}` does not return a result", f.name);
    };
    (
        res.ok.as_ref().map(|t| type_str(ctx, t)),
        res.err.as_ref().map(|t| type_str(ctx, t)),
    )
}

/// The `ferris-app` world from a parsed resolve.
fn framework_world(ctx: &Resolve) -> &wit_parser::World {
    ctx.worlds
        .iter()
        .find_map(|(_, w)| (w.name == "ferris-app").then_some(w))
        .unwrap_or_else(|| panic!("world `ferris-app` not found"))
}

/// `(interface, type)` display name of a named WIT type.
///
/// A world-level `use types.{...}` re-exports the type as a named alias the
/// **world** owns; follow those aliases to the interface definition.
fn type_name<'a>(ctx: &'a Resolve, ty: &Type) -> (&'a str, &'a str) {
    let mut current = ty;
    loop {
        let Type::Id(id) = current else {
            panic!("expected a named type, got {current:?}");
        };
        let def = &ctx.types[*id];
        match (&def.owner, &def.kind) {
            (TypeOwner::Interface(iface), _) => {
                return (
                    ctx.interfaces[*iface].name.as_deref().unwrap_or("(inline)"),
                    def.name.as_deref().unwrap_or("(anonymous)"),
                );
            }
            (_, TypeDefKind::Type(alias)) => current = alias,
            (owner, _) => panic!(
                "`{}` is neither an interface type nor an alias (owner {owner:?})",
                def.name.as_deref().unwrap_or("(anonymous)")
            ),
        }
    }
}

/// Render a WIT type the way the source writes it — enough of a grammar for
/// the shapes the boundary uses (`wit_parser::Type` has no `Display`).
fn type_str(ctx: &Resolve, ty: &Type) -> String {
    match ty {
        Type::Bool => "bool".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U16 => "u16".to_string(),
        Type::U32 => "u32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::S8 => "s8".to_string(),
        Type::S16 => "s16".to_string(),
        Type::S32 => "s32".to_string(),
        Type::S64 => "s64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Char => "char".to_string(),
        Type::String => "string".to_string(),
        Type::ErrorContext => "error-context".to_string(),
        Type::Id(id) => {
            let def = &ctx.types[*id];
            match &def.kind {
                TypeDefKind::List(inner) => format!("list<{}>", type_str(ctx, inner)),
                TypeDefKind::Tuple(tuple) => format!(
                    "tuple<{}>",
                    tuple
                        .types
                        .iter()
                        .map(|t| type_str(ctx, t))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                TypeDefKind::Option(inner) => format!("option<{}>", type_str(ctx, inner)),
                _ => def
                    .name
                    .clone()
                    .unwrap_or_else(|| "(anonymous)".to_string()),
            }
        }
    }
}

/// The `package …;` declaration of a contract source, which follows the banner
/// comment (so the file does not literally *start* with it).
fn declared_package(source: &str) -> Option<&str> {
    source
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("package "))
}

/// The exported top-level functions of the world, in declaration order.
fn export_fns(world: &wit_parser::World) -> Vec<&str> {
    world
        .exports
        .iter()
        .filter_map(|(_, item)| match item {
            WorldItem::Function(f) => Some(f.name.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn framework_wit_parses() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));

    // Exactly one world, and exactly the six declared interfaces.
    assert_eq!(ctx.worlds.len(), 1, "the contract declares one world");
    let declared: Vec<&str> = ctx
        .interfaces
        .iter()
        .filter_map(|(_, i)| i.name.as_deref())
        .collect();
    assert_eq!(
        declared,
        SERVICES.to_vec(),
        "the contract declares six interfaces, in order"
    );

    let world = framework_world(&ctx);
    let imports = items(&ctx, world);

    // Every host service the plugin can import must be present.
    for service in SERVICES {
        assert!(
            imports.contains_key(service),
            "missing import `{service}` in world `ferris-app`"
        );
    }

    // Every world export the host forwards to must exist as a top-level
    // function, in declaration order. `handle-stream-event` carries
    // open/close/error, so those never ride on an empty `handle-stream` chunk,
    // and `handle-datagram` is the datagram sibling of `handle-stream`.
    assert_eq!(
        export_fns(world),
        vec![
            "describe",
            "handle-request",
            "doctype-event",
            "handle-stream",
            "handle-stream-event",
            "handle-datagram",
            "handle-job",
            "ui-asset",
            "ui-widget",
        ],
        "world `ferris-app` exports"
    );

    // Data: documents, naming series, metadata and the method chain.
    // `get-docs`/`write-doc`/`apply-ops` cover single and batch calls, so a
    // missing row is answered positionally, never by a second entry point.
    assert_eq!(
        interface_fns(&ctx, &imports, "data"),
        vec![
            "list-docs",
            "get-docs",
            "write-doc",
            "apply-ops",
            "count-docs",
            "aggregate-docs",
            "next-name",
            "rename-doc",
            "set-docstatus",
            "doctype-json",
            "field-types",
            "get-doctype",
            "list-doctypes",
            "call-method",
        ]
    );

    // schema: the full DocType declaration escape hatch. One function, so an
    // app cannot bypass the canonical fieldtype/ownership checks on its way in.
    assert_eq!(
        interface_fns(&ctx, &imports, "schema"),
        vec!["declare-doctype"]
    );

    // io: the host owns every socket; the guest controls protocol and policy.
    // Datagram sockets and the DNS listener sit here too: the guest asks for a
    // descriptor and receives payloads through `handle-datagram`, never an OS
    // socket.
    assert_eq!(
        interface_fns(&ctx, &imports, "io"),
        vec![
            "http-send",
            "connect-stream",
            "listen-stream",
            "send-stream",
            "close-stream",
            "stream-describe",
            "stream-read-state",
            "stream-write-state",
            "udp-connect",
            "udp-listen",
            "udp-send",
            "udp-close",
            "raw-open",
            "raw-send",
            "raw-close",
            "dns-serve",
            "dns-update",
            "dns-stop",
            "publish",
            "notify",
        ]
    );

    // platform: identity, grants, host features, config, schedules, the CA and
    // host-executed programs.
    assert_eq!(
        interface_fns(&ctx, &imports, "platform"),
        vec![
            "current-user",
            "can",
            "grants",
            "features",
            "config-get",
            "config-set",
            "config-list",
            "config-delete",
            "schedule",
            "enqueue-job",
            "cancel-schedule",
            "list-schedules",
            "sign-csr",
            "ca-cert",
            "revoke-cert",
            "is-revoked",
            "process-run",
            "process-spawn",
            "process-wait",
            "process-kill",
        ]
    );

    // `types`/`descriptor` are pure vocabulary: no functions, only records.
    assert!(interface_fns(&ctx, &imports, "types").is_empty());
    assert!(interface_fns(&ctx, &imports, "descriptor").is_empty());

    // The contract and the scaffold's mirror declare the same wave.
    let source = std::fs::read_to_string(path).expect("the wit file is readable");
    assert_eq!(
        declared_package(&source),
        Some(PACKAGE),
        "the contract must declare the framework package: {:?}",
        source.lines().next().unwrap_or("<empty>")
    );
}

/// The typed request boundary. `route-id` lets one export serve every declared
/// endpoint: the host matches the prefix, the guest parses
/// `http-request.path`. The reply is the typed `io` response, never a pair of
/// strings.
#[test]
fn handle_request_is_typed() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);

    let handle_request = match imports.get("handle-request") {
        Some(WorldItem::Function(f)) => f,
        other => panic!("`handle-request` is not a function export: {other:?}"),
    };
    assert_eq!(
        param_strs(&ctx, handle_request),
        vec![
            ("route-id", "string".to_string()),
            ("req", "http-request".to_string()),
        ],
        "handle-request takes the matched route id and the typed request"
    );
    assert_eq!(
        result_types(&ctx, handle_request),
        (Some("response".to_string()), Some("error".to_string())),
        "handle-request returns `result<io::response, types::error>`"
    );
    let Type::Id(req_id) = handle_request.params[1].ty else {
        panic!("handle-request's second parameter is not a named type");
    };
    assert_eq!(
        type_name(&ctx, &Type::Id(req_id)),
        ("types", "http-request"),
        "handle-request's parameter must be the typed http-request record"
    );
    let ret = handle_request
        .result
        .unwrap_or_else(|| panic!("handle-request returns nothing"));
    let Type::Id(ret_id) = ret else {
        panic!("handle-request must return `result<response, error>`: {ret:?}");
    };
    let TypeDefKind::Result(res) = &ctx.types[ret_id].kind else {
        panic!("handle-request does not return a result");
    };
    assert_eq!(
        type_name(&ctx, &res.ok.expect("ok type")),
        ("io", "response"),
        "handle-request ok type"
    );

    let types = iface(&ctx, &imports, "types");
    assert_eq!(
        record_fields(&ctx, types, "http-request"),
        vec![
            ("method", "string".to_string()),
            ("path", "string".to_string()),
            ("query", "string".to_string()),
            ("headers", "list<tuple<string, string>>".to_string()),
            ("body", "list<u8>".to_string()),
            ("client-identity", "option<client-identity>".to_string()),
        ],
        "http-request fields"
    );

    // The verified client-certificate facts the host forwards to
    // `handle-request` and `io::stream-describe`; the fingerprint doubles as
    // the revocation key.
    assert_eq!(
        record_fields(&ctx, types, "client-identity"),
        vec![
            ("subject", "string".to_string()),
            ("common-name", "option<string>".to_string()),
            ("fingerprint", "string".to_string()),
            ("issuer", "string".to_string()),
        ],
        "client-identity fields"
    );

    // A classified failure the host maps to a status without parsing text.
    assert_eq!(
        variant_cases(&ctx, types, "error"),
        vec![
            ("not-found", Some("string".to_string())),
            ("permission-denied", Some("string".to_string())),
            ("validation-failed", Some("string".to_string())),
            ("conflict", Some("string".to_string())),
            ("rate-limited", Some("u32".to_string())),
            ("timeout", Some("string".to_string())),
            ("internal", Some("string".to_string())),
        ],
        "error taxonomy"
    );
}

/// `describe` exports the typed `descriptor`. It leads with what the app
/// *asks for* (`capabilities`) and what the host mounts (`routes`); `website`
/// is a plain list of pages.
#[test]
fn describe_is_typed() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);

    let describe = match imports.get("describe") {
        Some(WorldItem::Function(f)) => f,
        other => panic!("`describe` is not a function export: {other:?}"),
    };
    assert!(
        describe.params.is_empty(),
        "describe must take no parameters, got {:?}",
        describe.params
    );
    assert_eq!(
        result_types(&ctx, describe),
        (Some("descriptor".to_string()), Some("error".to_string())),
        "describe returns `result<descriptor, types::error>`"
    );
    let ret = describe
        .result
        .unwrap_or_else(|| panic!("describe returns nothing"));
    let Type::Id(ret_id) = ret else {
        panic!("describe must return `result<descriptor, error>`: {ret:?}");
    };
    let TypeDefKind::Result(res) = &ctx.types[ret_id].kind else {
        panic!("describe does not return a result");
    };
    assert_eq!(
        type_name(&ctx, &res.ok.expect("ok type")),
        ("descriptor", "descriptor"),
        "describe ok type"
    );

    let descriptor = interface(&ctx, "descriptor");
    assert_eq!(
        record_fields(&ctx, descriptor, "descriptor"),
        vec![
            ("capabilities", "list<capability>".to_string()),
            ("routes", "list<route>".to_string()),
            ("auth-mode", "route-auth".to_string()),
            ("http-policy", "option<http-policy>".to_string()),
            ("tls-client", "option<tls-client>".to_string()),
            ("tls-server", "option<tls-server>".to_string()),
            ("doctypes", "list<doctype>".to_string()),
            ("themes", "list<theme>".to_string()),
            ("folders", "list<folder>".to_string()),
            ("workspaces", "list<workspace>".to_string()),
            ("methods", "list<method>".to_string()),
            ("ui-extensions", "option<list<ui-extension>>".to_string()),
            ("dashboards", "option<list<dashboard>>".to_string()),
            ("widgets", "option<list<widget-def>>".to_string()),
            ("website", "option<list<website-route>>".to_string()),
            ("assets", "option<list<string>>".to_string()),
            ("subscriptions", "option<list<subscription>>".to_string()),
            ("config", "option<list<config-item>>".to_string()),
            ("plugin-version", "option<string>".to_string()),
            ("requires", "option<list<string>>".to_string()),
        ],
        "descriptor record fields"
    );
}

/// The explicit sandbox. An app declares the capabilities it wants, and the
/// host refuses anything outside the effective grant with `permission-denied`.
/// The network surface is declarative (`route` + `tls-*` + `http-policy`), and
/// the guest can open its own listeners.
#[test]
fn capabilities_and_routes_are_declarative() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);
    let types = iface(&ctx, &imports, "types");
    let descriptor = interface(&ctx, "descriptor");

    // The closed capability set. Every variant is a distinct grant, so the
    // list is pinned exactly: adding one is a wire change. `udp`, `raw-sockets`,
    // `dns-server` and `process` widen a host boundary (datagrams, packets, a
    // DNS zone, child processes) and `threads` widens the guest's own runtime;
    // none of them is primordial.
    assert_eq!(
        enum_cases(&ctx, types, "capability"),
        vec![
            "routes",
            "http",
            "http-unrestricted",
            "tls-client",
            "tls-server",
            "streams",
            "streams-listen",
            "docstore-read",
            "docstore-write",
            "filesystem",
            "overrides",
            "pki",
            "scheduler",
            "config",
            "schema",
            "notifications",
            "pubsub",
            "udp",
            "dns-server",
            "raw-sockets",
            "process",
            "threads",
        ],
        "capability enum"
    );
    // The grant is the effective set the host computed; there is no separate
    // operator profile to intersect with.
    assert_eq!(
        record_fields(&ctx, types, "grant-set"),
        vec![("capabilities", "list<capability>".to_string())]
    );

    // What a route requires before the host dispatches it.
    assert_eq!(
        variant_cases(&ctx, types, "route-auth"),
        vec![
            ("none", None),
            ("session", None),
            ("capability", Some("string".to_string())),
        ]
    );

    // The declared endpoint: a prefix under the app, with its own TLS policy
    // and—exactly for a stream route—its listener.
    assert_eq!(
        record_fields(&ctx, descriptor, "route"),
        vec![
            ("id", "string".to_string()),
            ("method", "string".to_string()),
            ("path", "string".to_string()),
            ("kind", "route-kind".to_string()),
            ("auth", "option<route-auth>".to_string()),
            ("body-limit", "option<u32>".to_string()),
            ("tls-server", "option<tls-server>".to_string()),
            ("listener", "option<stream-listen>".to_string()),
        ]
    );
    assert_eq!(
        enum_cases(&ctx, descriptor, "route-kind"),
        vec!["http", "stream"]
    );

    // Outbound policy: the declaration can only tighten the host's caps.
    assert_eq!(
        record_fields(&ctx, types, "http-policy"),
        vec![
            ("allowed-hosts", "list<string>".to_string()),
            ("allow-private", "bool".to_string()),
            ("max-body", "option<u64>".to_string()),
            ("timeout-ms", "option<u32>".to_string()),
            ("redirects", "redirect-policy".to_string()),
            ("allow-credentials", "bool".to_string()),
        ]
    );
    assert_eq!(
        variant_cases(&ctx, types, "redirect-policy"),
        vec![
            ("none", None),
            ("limited", Some("u8".to_string())),
            ("follow", None),
        ]
    );

    // TLS material, client and server side. `secret` is the recommended form
    // for a private key; `pem` is redacted from every JSON projection.
    assert_eq!(
        variant_cases(&ctx, types, "tls-material"),
        vec![
            ("pem", Some("string".to_string())),
            ("secret", Some("string".to_string())),
        ]
    );
    assert_eq!(
        enum_cases(&ctx, types, "tls-version"),
        vec!["tls12", "tls13"]
    );
    assert_eq!(
        enum_cases(&ctx, types, "client-auth"),
        vec!["none", "optional", "required"]
    );
    assert_eq!(
        record_fields(&ctx, types, "tls-client"),
        vec![
            ("cert", "option<tls-material>".to_string()),
            ("key", "option<tls-material>".to_string()),
            ("ca", "option<tls-material>".to_string()),
            ("server-name", "option<string>".to_string()),
            ("alpn", "list<string>".to_string()),
            ("min-version", "tls-version".to_string()),
            ("verify", "bool".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, types, "tls-server"),
        vec![
            ("cert", "tls-material".to_string()),
            ("key", "tls-material".to_string()),
            ("ca", "option<tls-material>".to_string()),
            ("client-auth", "client-auth".to_string()),
            ("sni", "list<string>".to_string()),
            ("alpn", "list<string>".to_string()),
            ("min-version", "tls-version".to_string()),
        ]
    );

    // A listener the app owns: the host binds and accepts, the guest gets a
    // `stream-id` per connection. `address` is optional so the host can infer
    // the path prefix or its bind address, and `port` `none`/`0` is an inferred
    // port that `stream-describe` reports. `udp`, `raw` and `icmp` are datagram
    // transports and are refused by `listen-stream`: they have their own opener.
    assert_eq!(
        enum_cases(&ctx, types, "stream-transport"),
        vec!["websocket", "tcp", "tls", "udp", "raw", "icmp"]
    );
    assert_eq!(
        record_fields(&ctx, types, "stream-listen"),
        vec![
            ("transport", "stream-transport".to_string()),
            ("address", "option<string>".to_string()),
            ("port", "option<u16>".to_string()),
            ("tls", "option<tls-server>".to_string()),
            ("auth", "option<route-auth>".to_string()),
            ("max-frame", "option<u32>".to_string()),
            ("idle-timeout-seconds", "option<u32>".to_string()),
        ]
    );
}

/// The document store has one write primitive. `write-doc` takes a typed
/// `write` (kind + body + disposition) and `apply-ops` is the only batch;
/// create/upsert/update/delete ride on the write kind, and a no-events write is
/// the `write-disposition.silent` case.
#[test]
fn data_surface_has_one_write_and_one_batch() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);
    let data = iface(&ctx, &imports, "data");

    assert_eq!(
        param_strs(&ctx, func(data, "write-doc")),
        vec![("write", "write".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(data, "write-doc")),
        (Some("option<doc>".to_string()), Some("error".to_string()))
    );
    assert_eq!(
        param_strs(&ctx, func(data, "apply-ops")),
        vec![("ops", "list<write>".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(data, "apply-ops")),
        (
            Some("list<option<doc>>".to_string()),
            Some("error".to_string())
        )
    );
    assert_eq!(
        record_fields(&ctx, data, "write"),
        vec![
            ("kind", "op-kind".to_string()),
            ("doctype", "string".to_string()),
            ("name", "option<string>".to_string()),
            ("data", "option<string>".to_string()),
            ("disposition", "write-disposition".to_string()),
        ]
    );
    assert_eq!(
        enum_cases(&ctx, data, "op-kind"),
        vec!["create", "update", "upsert", "delete"]
    );
    assert_eq!(
        enum_cases(&ctx, data, "write-disposition"),
        vec!["emit-events", "silent"]
    );

    // Positional batch read: `none` for a row that does not exist, so the
    // caller owns the strictness instead of a second `get` entry point.
    assert_eq!(
        param_strs(&ctx, func(data, "get-docs")),
        vec![
            ("doctype", "string".to_string()),
            ("names", "list<string>".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, func(data, "get-docs")),
        (
            Some("list<option<doc>>".to_string()),
            Some("error".to_string())
        )
    );

    // Filters are typed: the operator is a closed enum, not a string the guest
    // and host must spell identically.
    assert_eq!(
        record_fields(&ctx, data, "filter"),
        vec![
            ("field", "string".to_string()),
            ("op", "filter-op".to_string()),
            ("value", "string".to_string()),
        ]
    );
    assert_eq!(
        enum_cases(&ctx, data, "filter-op"),
        vec![
            "eq", "ne", "like", "in", "gt", "gte", "lt", "lte", "between"
        ]
    );
    assert_eq!(
        record_fields(&ctx, data, "sort"),
        vec![
            ("field", "string".to_string()),
            ("desc", "bool".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, data, "query"),
        vec![
            ("filters", "list<filter>".to_string()),
            ("order-by", "list<sort>".to_string()),
            ("limit", "option<u32>".to_string()),
            ("offset", "option<u32>".to_string()),
            ("fields", "option<list<string>>".to_string()),
        ]
    );

    // Aggregates travel without surprises: a grand-total row has an empty
    // group.
    assert_eq!(
        record_fields(&ctx, data, "aggregate-spec"),
        vec![
            ("fn", "aggregate-fn".to_string()),
            ("field", "string".to_string()),
            ("alias", "option<string>".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, data, "aggregate-row"),
        vec![
            ("group", "list<tuple<string, string>>".to_string()),
            ("values", "list<tuple<string, string>>".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, data, "doctype-meta"),
        vec![
            ("name", "string".to_string()),
            ("fields", "list<field-meta>".to_string()),
            ("is-submittable", "bool".to_string()),
            ("is-child-table", "bool".to_string()),
            ("is-single", "bool".to_string()),
        ]
    );
}

/// `io` is the only network interface. Outbound calls carry their own policy
/// and TLS, streams are described by `stream-info`, and `publish`/`notify` live
/// here too.
#[test]
fn io_and_platform_are_the_network_surface() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);
    let io = iface(&ctx, &imports, "io");
    let platform = iface(&ctx, &imports, "platform");

    assert_eq!(
        record_fields(&ctx, io, "request"),
        vec![
            ("method", "string".to_string()),
            ("url", "string".to_string()),
            // `headers` is the shared `types.headers` alias, not a repeated
            // `list<tuple<…>>`, so the two sides cannot drift.
            ("headers", "headers".to_string()),
            ("body", "option<list<u8>>".to_string()),
            ("timeout-ms", "option<u32>".to_string()),
            ("redirects", "option<redirect-policy>".to_string()),
            ("tls", "option<tls-client>".to_string()),
            ("policy", "option<http-policy>".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, io, "response"),
        vec![
            ("status", "u16".to_string()),
            ("headers", "headers".to_string()),
            ("body", "list<u8>".to_string()),
        ]
    );

    // Transport facts of an active stream. `registration-id` is re-stamped
    // exactly when a different connection re-takes the id, which is what a
    // guest compares to invalidate its cached identity; `listener` names the
    // accepting listener when the app owns it. `protocol` names the datagram
    // protocol for a `udp`/`raw`/`icmp` socket (`none` for a byte stream), and
    // the optional addresses plus `port` carry an inferred bind back to the
    // guest.
    assert_eq!(
        record_fields(&ctx, io, "stream-info"),
        vec![
            ("transport", "string".to_string()),
            ("peer", "string".to_string()),
            ("auth-required", "bool".to_string()),
            ("created-at", "u64".to_string()),
            ("bytes-received", "u64".to_string()),
            ("client-identity", "option<client-identity>".to_string()),
            ("registration-id", "string".to_string()),
            ("listener", "option<string>".to_string()),
            ("protocol", "option<string>".to_string()),
            ("local-addr", "option<string>".to_string()),
            ("remote-addr", "option<string>".to_string()),
            ("port", "option<u16>".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, io, "connect-spec"),
        vec![
            ("transport", "stream-transport".to_string()),
            ("uri", "string".to_string()),
            ("address", "option<string>".to_string()),
            ("port", "option<u16>".to_string()),
            ("tls", "option<tls-client>".to_string()),
            ("policy", "option<http-policy>".to_string()),
            ("max-frame", "option<u32>".to_string()),
            ("idle-timeout-seconds", "option<u32>".to_string()),
        ]
    );

    // Publish and notify: the declaration is enforcement (`subscriptions`), and
    // delivery infrastructure stays the host's.
    assert_eq!(
        param_strs(&ctx, func(io, "publish")),
        vec![
            ("channel", "string".to_string()),
            ("event-type", "string".to_string()),
            ("payload", "string".to_string()),
        ]
    );
    assert_eq!(
        param_strs(&ctx, func(io, "notify")),
        vec![
            ("to", "string".to_string()),
            ("channel", "string".to_string()),
            ("title", "string".to_string()),
            ("message", "string".to_string()),
        ]
    );

    // Platform: one batch permission call in place of `can`/`can-many`, and
    // `grants` reporting the effective set the app branches on.
    assert_eq!(
        record_fields(&ctx, platform, "permission-check"),
        vec![
            ("doctype", "string".to_string()),
            ("action", "string".to_string()),
            ("name", "option<string>".to_string()),
        ]
    );
    assert_eq!(
        param_strs(&ctx, func(platform, "can")),
        vec![("checks", "list<permission-check>".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(platform, "can")),
        (Some("list<bool>".to_string()), Some("error".to_string()))
    );
    assert_eq!(
        result_types(&ctx, func(platform, "grants")),
        (Some("grant-set".to_string()), Some("error".to_string()))
    );
    assert!(func(platform, "current-user").params.is_empty());
    assert_eq!(
        param_strs(&ctx, func(platform, "enqueue-job")),
        vec![
            ("job-type", "string".to_string()),
            ("payload", "string".to_string()),
        ]
    );

    // Scheduler: `schedule` returns the new id, `cancel-schedule` and
    // `list-schedules` complete the app-scoped lifecycle.
    assert_eq!(
        result_types(&ctx, func(platform, "schedule")),
        (Some("string".to_string()), Some("error".to_string()))
    );
    assert_eq!(
        param_strs(&ctx, func(platform, "cancel-schedule")),
        vec![("schedule-id", "string".to_string())]
    );
    assert_eq!(
        record_fields(&ctx, platform, "schedule-info"),
        vec![
            ("id", "string".to_string()),
            ("job-name", "string".to_string()),
            ("cron", "string".to_string()),
            ("enabled", "bool".to_string()),
        ]
    );

    // The CA never crosses the boundary: the guest signs a CSR, reads the CA
    // certificate and revokes by fingerprint.
    assert_eq!(
        enum_cases(&ctx, platform, "cert-usage"),
        vec!["client", "server"]
    );
    assert_eq!(
        param_strs(&ctx, func(platform, "sign-csr")),
        vec![
            ("csr-der-b64", "string".to_string()),
            ("ttl-seconds", "u32".to_string()),
            ("usage", "cert-usage".to_string()),
        ]
    );

    // `handle-job`: `job-type` + JSON payload in, a result string out (a world
    // export, not an interface import).
    let Some(WorldItem::Function(handle_job)) = imports.get("handle-job") else {
        panic!("handle-job must be an exported function");
    };
    assert_eq!(
        param_strs(&ctx, handle_job),
        vec![
            ("job-type", "string".to_string()),
            ("payload", "string".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, handle_job),
        (Some("string".to_string()), Some("error".to_string()))
    );
}

/// Stream lifecycle is its own export. `handle-stream` carries data only (an
/// empty frame is legitimate) and `handle-stream-event` reports `opened` /
/// `closed` / `error` once per stream, in order — the guest creates
/// per-connection state on open and releases it on close, instead of inferring
/// either from a chunk.
#[test]
fn stream_lifecycle_has_its_own_export() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);

    let Some(WorldItem::Function(handle_stream)) = imports.get("handle-stream") else {
        panic!("handle-stream must be an exported function");
    };
    assert_eq!(
        param_strs(&ctx, handle_stream),
        vec![
            ("stream-id", "string".to_string()),
            ("chunk", "list<u8>".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, handle_stream),
        (Some("list<u8>".to_string()), Some("error".to_string()))
    );

    let Some(WorldItem::Function(handle_event)) = imports.get("handle-stream-event") else {
        panic!("handle-stream-event must be an exported function");
    };
    assert_eq!(
        param_strs(&ctx, handle_event),
        vec![
            ("stream-id", "string".to_string()),
            ("event", "stream-event".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, handle_event),
        (None, Some("error".to_string())),
        "handle-stream-event answers `result<_, error>`"
    );
    let Type::Id(event_ty) = handle_event.params[1].ty else {
        panic!("handle-stream-event's event is not a named type");
    };
    assert_eq!(
        type_name(&ctx, &Type::Id(event_ty)),
        ("types", "stream-event")
    );

    // The lifecycle events, and no others: a transported protocol rides on the
    // app's own frames, never on a new host variant.
    let types = iface(&ctx, &imports, "types");
    assert_eq!(
        variant_cases(&ctx, types, "stream-event"),
        vec![
            ("opened", None),
            ("closed", None),
            ("error", Some("string".to_string())),
        ]
    );
}

/// 0.3.0 adds the host-executed datagram and process surfaces. The guest never
/// holds a socket or a fork: it receives datagrams through `handle-datagram`,
/// asks for a DNS zone by records and gets a program's captured output. `threads`
/// is the one new capability with no WIT call — the guest uses its own runtime
/// and only reads `features`/`grants`, so a function for it would be dead surface.
#[test]
fn datagram_sockets_and_processes_are_host_executed() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);
    let io = iface(&ctx, &imports, "io");
    let platform = iface(&ctx, &imports, "platform");
    let types = iface(&ctx, &imports, "types");

    // The datagram export: a socket id, the payload and the sender, with the
    // reply bytes back to the same peer. It is separate from `handle-stream`
    // because a datagram has boundaries and an empty payload is a real packet.
    let Some(WorldItem::Function(handle_datagram)) = imports.get("handle-datagram") else {
        panic!("handle-datagram must be an exported function");
    };
    assert_eq!(
        param_strs(&ctx, handle_datagram),
        vec![
            ("stream-id", "string".to_string()),
            ("data", "list<u8>".to_string()),
            ("peer", "string".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, handle_datagram),
        (Some("list<u8>".to_string()), Some("error".to_string()))
    );

    // UDP: a connected socket and a listener, both owned by the host.
    assert_eq!(
        record_fields(&ctx, io, "udp-spec"),
        vec![
            ("address", "string".to_string()),
            ("port", "option<u16>".to_string()),
            ("local-address", "option<string>".to_string()),
            ("local-port", "option<u16>".to_string()),
            ("broadcast", "bool".to_string()),
            ("ttl", "option<u8>".to_string()),
        ]
    );
    // The record is `udp-listener`: an interface has one item namespace, and
    // `udp-listen` is the function that returns its id.
    assert_eq!(
        record_fields(&ctx, io, "udp-listener"),
        vec![
            ("address", "option<string>".to_string()),
            ("port", "option<u16>".to_string()),
            ("broadcast", "bool".to_string()),
            ("max-datagram", "option<u32>".to_string()),
        ]
    );
    assert_eq!(
        param_strs(&ctx, func(io, "udp-connect")),
        vec![("spec", "udp-spec".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(io, "udp-connect")),
        (Some("string".to_string()), Some("error".to_string()))
    );
    assert_eq!(
        param_strs(&ctx, func(io, "udp-listen")),
        vec![("spec", "udp-listener".to_string())]
    );
    assert_eq!(
        param_strs(&ctx, func(io, "udp-send")),
        vec![
            ("stream-id", "string".to_string()),
            ("datagram", "list<u8>".to_string()),
            ("peer", "option<string>".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, func(io, "udp-send")),
        (None, Some("error".to_string()))
    );

    // Raw/ICMP: the same shape, with the IP protocol number and no rewriting of
    // the packet the guest built.
    assert_eq!(
        record_fields(&ctx, io, "raw-spec"),
        vec![
            ("transport", "stream-transport".to_string()),
            ("address", "option<string>".to_string()),
            ("port", "option<u16>".to_string()),
            ("ip-protocol", "option<u8>".to_string()),
        ]
    );
    assert_eq!(
        param_strs(&ctx, func(io, "raw-open")),
        vec![("spec", "raw-spec".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(io, "raw-open")),
        (Some("string".to_string()), Some("error".to_string()))
    );
    assert_eq!(
        param_strs(&ctx, func(io, "raw-send")),
        vec![
            ("stream-id", "string".to_string()),
            ("packet", "list<u8>".to_string()),
            ("destination", "option<string>".to_string()),
        ]
    );

    // Closing is idempotent and carries no result by design: the authoritative
    // signal is the stream `closed` event, and a bind failure already surfaced
    // in the open call. A `result` here would invent a second failure path.
    for (name, param) in [
        ("udp-close", "stream-id"),
        ("raw-close", "stream-id"),
        ("dns-stop", "listener-id"),
    ] {
        let f = func(io, name);
        assert!(f.result.is_none(), "`{name}` must not return a result");
        assert_eq!(param_strs(&ctx, f), vec![(param, "string".to_string())]);
    }

    // DNS: a closed record-type set, textual RDATA, and one typed SOA.
    assert_eq!(
        enum_cases(&ctx, io, "dns-record-type"),
        vec!["a", "aaaa", "cname", "mx", "txt", "srv", "ptr", "ns", "soa"]
    );
    assert_eq!(
        record_fields(&ctx, io, "dns-record"),
        vec![
            ("name", "string".to_string()),
            // `%type` is the WIT escape for the `type` keyword; the wire name is
            // `type`, like `%stream` is `stream`.
            ("type", "dns-record-type".to_string()),
            ("ttl", "u32".to_string()),
            ("data", "string".to_string()),
            ("priority", "option<u16>".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, io, "dns-soa"),
        vec![
            ("primary-ns", "string".to_string()),
            ("admin", "string".to_string()),
            ("serial", "u32".to_string()),
            ("refresh", "u32".to_string()),
            ("retry", "u32".to_string()),
            ("expire", "u32".to_string()),
            ("minimum-ttl", "u32".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, io, "dns-zone"),
        vec![
            ("origin", "string".to_string()),
            ("address", "option<string>".to_string()),
            ("port", "option<u16>".to_string()),
            ("records", "list<dns-record>".to_string()),
            ("soa", "option<dns-soa>".to_string()),
        ]
    );
    assert_eq!(
        param_strs(&ctx, func(io, "dns-serve")),
        vec![("zone", "dns-zone".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(io, "dns-serve")),
        (Some("string".to_string()), Some("error".to_string()))
    );
    assert_eq!(
        param_strs(&ctx, func(io, "dns-update")),
        vec![
            ("listener-id", "string".to_string()),
            ("zone", "dns-zone".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, func(io, "dns-update")),
        (None, Some("error".to_string()))
    );

    // Programs: the spec carries the whole environment and a world-relative cwd;
    // the output carries both streams, the exit status and the timeout flag.
    assert_eq!(
        record_fields(&ctx, platform, "process-spec"),
        vec![
            ("program", "string".to_string()),
            ("args", "list<string>".to_string()),
            ("env", "list<tuple<string, string>>".to_string()),
            ("cwd", "option<string>".to_string()),
            ("stdin", "option<list<u8>>".to_string()),
            ("timeout-ms", "option<u32>".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, platform, "process-output"),
        vec![
            // The WIT spelling of a signed 32-bit status is `s32`.
            ("status", "s32".to_string()),
            ("stdout", "list<u8>".to_string()),
            ("stderr", "list<u8>".to_string()),
            ("timed-out", "bool".to_string()),
        ]
    );
    assert_eq!(
        param_strs(&ctx, func(platform, "process-run")),
        vec![("spec", "process-spec".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(platform, "process-run")),
        (
            Some("process-output".to_string()),
            Some("error".to_string())
        )
    );
    assert_eq!(
        param_strs(&ctx, func(platform, "process-spawn")),
        vec![("spec", "process-spec".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(platform, "process-spawn")),
        (Some("string".to_string()), Some("error".to_string()))
    );
    assert_eq!(
        param_strs(&ctx, func(platform, "process-wait")),
        vec![("process-id", "string".to_string())]
    );
    assert_eq!(
        result_types(&ctx, func(platform, "process-wait")),
        (
            Some("process-output".to_string()),
            Some("error".to_string())
        )
    );
    assert_eq!(
        param_strs(&ctx, func(platform, "process-kill")),
        vec![("process-id", "string".to_string())]
    );

    // Every new host boundary is specific: declaring it is the request, and the
    // grant set reports it. `threads` is a capability with no call of its own.
    let capabilities = enum_cases(&ctx, types, "capability");
    for capability in ["udp", "dns-server", "raw-sockets", "process", "threads"] {
        assert!(
            capabilities.contains(&capability),
            "the capability set must carry `{capability}`"
        );
    }
    for (service, iface) in [("io", io), ("platform", platform)] {
        for f in iface.functions.keys() {
            assert!(
                !f.contains("thread"),
                "`threads` is a grant, not a {service} function (found `{f}`)"
            );
        }
    }
}

/// The Desk-facing descriptor shapes stay pinned here because the guest
/// constructor, the host projection and the `/api/ui-extensions` feed all
/// compile against them; a field rename would break every installed app.
#[test]
fn descriptor_shapes_are_pinned() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let descriptor = interface(&ctx, "descriptor");

    // Grid geometry on the widget catalog.
    assert_eq!(
        record_fields(&ctx, descriptor, "grid-size"),
        vec![("span", "u8".to_string()), ("rows", "u8".to_string())]
    );
    for (record, field, ty) in [
        ("widget-param", "group", "option<string>"),
        ("widget-def", "icon", "option<string>"),
        ("widget-def", "category", "option<string>"),
        ("widget-def", "size", "option<grid-size>"),
        ("widget-def", "min", "option<grid-size>"),
        ("widget-instance", "col", "option<u8>"),
        ("widget-instance", "row", "option<u16>"),
        ("widget-instance", "rows", "option<u8>"),
    ] {
        let fields = record_fields(&ctx, descriptor, record);
        assert!(
            fields.iter().any(|(n, t)| *n == field && t == ty),
            "{record} must carry `{field}: {ty}`: found {fields:?}"
        );
    }

    // 6.0.0 + 7.0.0: a doctype says how its documents open, and the
    // document-backed case carries per-field bindings (never `route`: a
    // document's route is its Desk URL).
    let doctype = record_fields(&ctx, descriptor, "doctype");
    assert!(
        doctype.iter().any(|(n, t)| *n == "view" && t == "doc-view"),
        "doctype must carry `view: doc-view`: found {doctype:?}"
    );
    assert_eq!(
        variant_cases(&ctx, descriptor, "doc-view"),
        vec![
            ("form", None),
            ("dashboard-native", None),
            ("dashboard-fixed", Some("string".to_string())),
            ("dashboard-link", Some("string".to_string())),
            ("dashboard-document", Some("dashboard-format".to_string())),
            ("custom", None),
        ]
    );
    assert_eq!(
        record_fields(&ctx, descriptor, "dashboard-format"),
        vec![
            ("widgets", "option<string>".to_string()),
            ("layout", "option<string>".to_string()),
            ("label", "option<string>".to_string()),
            ("config", "option<string>".to_string()),
        ]
    );

    // 9.0.0–11.0.0: the widget catalog entry — preview, full module source and
    // the asset path that keeps relative imports resolving.
    let def = record_fields(&ctx, descriptor, "widget-def");
    for field in [
        "preview",
        "code-path",
        "script",
        "stylesheet",
        "ssr",
        "renderer",
        "version",
    ] {
        assert!(
            def.iter()
                .any(|(name, ty)| *name == field && ty == "option<string>"),
            "widget-def must carry `{field}: option<string>`: found {def:?}"
        );
    }

    // 10.0.0: declared appearance, every knob optional and colours as tokens.
    assert_eq!(
        record_fields(&ctx, descriptor, "style-spec"),
        vec![
            ("background", "option<string>".to_string()),
            ("background-custom", "option<string>".to_string()),
            ("border-color", "option<string>".to_string()),
            ("border-width", "option<u8>".to_string()),
            ("border-radius", "option<u8>".to_string()),
            ("padding", "option<u8>".to_string()),
            ("shadow", "option<string>".to_string()),
        ]
    );
    assert_eq!(
        record_fields(&ctx, descriptor, "dashboard-style"),
        vec![
            ("background", "option<string>".to_string()),
            ("background-custom", "option<string>".to_string()),
            ("gap-px", "option<u8>".to_string()),
            ("padding-px", "option<u8>".to_string()),
            ("border-radius", "option<u8>".to_string()),
            ("title-full-screen", "option<bool>".to_string()),
        ]
    );
    for (record, field, ty) in [
        ("widget-def", "style", "option<style-spec>"),
        ("widget-instance", "style", "option<style-spec>"),
        ("dashboard", "style", "option<dashboard-style>"),
    ] {
        let fields = record_fields(&ctx, descriptor, record);
        assert!(
            fields.iter().any(|(name, t)| *name == field && t == ty),
            "{record} must carry `{field}: {ty}`: found {fields:?}"
        );
    }

    // A widget parameter describes its own form control.
    let param = record_fields(&ctx, descriptor, "widget-param");
    for field in ["placeholder", "depends-on"] {
        assert!(
            param
                .iter()
                .any(|(name, ty)| *name == field && ty == "option<string>"),
            "widget-param must carry `{field}: option<string>`: found {param:?}"
        );
    }
    for field in ["read-only", "hidden"] {
        assert!(
            param
                .iter()
                .any(|(name, ty)| *name == field && ty == "bool"),
            "widget-param must carry `{field}: bool`: found {param:?}"
        );
    }
}

/// The host is modular, so the contract carries feature detection
/// (`platform::features`). The running binary reports what it compiled; a guest
/// checks the list before showing a surface the host may not have.
#[test]
fn features_report_the_compiled_set() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);
    let platform = iface(&ctx, &imports, "platform");

    // Feature detection: no parameters, a list of names (sorted by the host).
    assert!(
        func(platform, "features").params.is_empty(),
        "platform::features takes no parameters"
    );
    assert_eq!(
        result_types(&ctx, func(platform, "features")),
        (Some("list<string>".to_string()), Some("error".to_string())),
        "platform::features returns `result<list<string>, error>`"
    );
}

/// The guest owns a world through `filesystem`, has no blob/file host API, and
/// a method can override another with an explicit `super` through
/// `data::call-method`.
#[test]
fn world_and_method_overrides_are_part_of_the_contract() {
    let mut ctx = Resolve::default();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    ctx.push_path(path)
        .unwrap_or_else(|e| panic!("wit parse failed on {path}: {e}"));
    let world = framework_world(&ctx);
    let imports = items(&ctx, world);
    let data = iface(&ctx, &imports, "data");
    let descriptor = interface(&ctx, "descriptor");
    let types = iface(&ctx, &imports, "types");

    // The world is a mount, not a per-file API: the guest imports a filesystem,
    // and the blob round-trip the host used to provide is gone.
    let capabilities = enum_cases(&ctx, types, "capability");
    assert!(capabilities.contains(&"filesystem"), "{capabilities:?}");
    assert!(capabilities.contains(&"overrides"), "{capabilities:?}");
    for retired in ["blobs", "files", "secrets"] {
        assert!(
            !capabilities.contains(&retired),
            "`{retired}` is not part of the capability set"
        );
    }
    for retired in [
        "put-blob",
        "get-blob",
        "stat-blob",
        "list-blobs",
        "sign-blob-url",
        "delete-blob",
    ] {
        assert!(
            !interface_fns(&ctx, &imports, "data").contains(&retired),
            "`{retired}` must not survive the world"
        );
    }

    // The explicit `super`: JSON in, JSON out, and the result carries the same
    // typed error as every other import.
    assert_eq!(
        param_strs(&ctx, func(data, "call-method")),
        vec![
            ("name", "string".to_string()),
            ("params", "string".to_string()),
        ]
    );
    assert_eq!(
        result_types(&ctx, func(data, "call-method")),
        (Some("string".to_string()), Some("error".to_string()))
    );

    // A declaration says whether it creates a method or overrides one.
    assert_eq!(
        record_fields(&ctx, descriptor, "method"),
        vec![
            ("name", "string".to_string()),
            ("params", "list<tuple<string, string>>".to_string()),
            ("returns", "string".to_string()),
            ("description", "string".to_string()),
            ("overrides", "bool".to_string()),
        ]
    );
}

/// The mirror copy the `ferris new` scaffold (and the git-ignored `test/`
/// guest) compiles against must track the same wave and stay **byte-identical**:
/// two different contracts under one package version is how a scaffolded guest
/// loads on one host and fails on another.
#[test]
fn mirror_wit_is_byte_identical() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/wit/ferris.wit");
    let mirror = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/wit/ferris.wit");
    let source = std::fs::read_to_string(path).expect("the wit file is readable");
    let mirror_source = std::fs::read_to_string(mirror).expect("the mirror wit is readable");
    assert_eq!(
        declared_package(&mirror_source),
        Some(PACKAGE),
        "the scaffold's mirror WIT must declare the same wave: {:?}",
        mirror_source.lines().next().unwrap_or("<empty>")
    );
    assert_eq!(
        source, mirror_source,
        "the two WIT copies must stay byte-identical"
    );
}
