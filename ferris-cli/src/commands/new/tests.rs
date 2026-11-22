use super::cargo_files::render_cargo_toml;
use super::command::validate_project_name;
use super::config_files::render_ferris_toml;
use super::lib_rs::render_lib_rs;
use super::project_docs::render_readme;
use super::toolchain::render_toolchain_toml;

#[test]
fn cargo_toml_is_a_standalone_component() {
    let manifest = render_cargo_toml("my-app");
    assert!(manifest.contains("name = \"my-app\""));
    assert!(manifest.contains("crate-type = [\"cdylib\"]"));
    assert!(manifest.contains("wit-bindgen = \"0.62\""));
    assert!(manifest.contains("[workspace]"));
    assert!(
        !manifest
            .lines()
            .any(|line| line.trim_start().starts_with("ferris-")),
        "no framework dependency"
    );
}

/// The world declares every export as `result<_, types::Error>`; a
/// scaffold returning plain `String` does not compile (16 errors
/// reproduced on 2026-10-07 — keep these signatures exact).
#[test]
fn lib_rs_matches_the_world_export_signatures() {
    let src = render_lib_rs("my-app");
    assert!(src.contains("wit_bindgen::generate!"));
    assert!(src.contains("fn describe() -> Result<descriptor::Descriptor, Error>"));
    assert!(src.contains("descriptor::Field::Name(\"subject\".to_string())"));
    assert!(src.contains("indexes: Vec::new()"));
    // WIT 6.0.0: a declared doctype states how its documents open; a
    // scaffold that omits `view` does not compile against the record.
    assert!(
        src.contains("view: descriptor::DocView::Form"),
        "the scaffold must declare the doctype's view kind"
    );
    // WIT 2.0.0: the request crosses as a typed record (method/path/
    // query/headers/body bytes) and the reply is a typed response
    // (status/headers/body), not two `String`s.
    // The export also takes the id of the route the host matched.
    assert!(src.contains(
        "fn handle_request(route_id: String, req: HttpRequest) -> Result<Response, Error>"
    ));
    assert!(src.contains(") -> Result<(), Error>"));
    assert!(
        src.contains(
            "fn handle_stream(stream_id: String, chunk: Vec<u8>) -> Result<Vec<u8>, Error>"
        )
    );
    // Stream lifecycle is its own export — open/close/error no
    // longer ride on an empty chunk.
    assert!(
        src.contains(
            "fn handle_stream_event(stream_id: String, event: StreamEvent) -> Result<(), Error>"
        ),
        "the scaffold must export the stream lifecycle callback"
    );
    // The job callback (the cron re-entry): `job-type` + JSON `payload` in,
    // a result `String` out.
    assert!(
        src.contains("fn handle_job(job_type: String, payload: String) -> Result<String, Error>")
    );
    // WIT 0.3.0: datagrams have their own export, sibling of `handle-stream`.
    assert!(
        src.contains("fn handle_datagram("),
        "the scaffold must export the datagram callback"
    );
    // The WIT error is the typed `types::Error`, never `String`.
    assert!(
        !src.contains("Result<String, String>"),
        "exports must not use String as the error type"
    );
    // Only events the host actually dispatches (`EVENT_POINTS` in
    // ferris-wasm/src/store.rs): no `before-*`, no `after-submit`.
    assert!(src.contains("\"submitted\""));
    assert!(!src.contains("after-submit"));
    assert!(!src.contains("before-save"));
    // The schedule payload matches what `scheduler_schedule` parses
    // (cron/job_name/payload) — not the dead `function`/`args` keys.
    assert!(src.contains("task_nightly"));
    assert!(!src.contains("app.on_submit"));
}

/// The app declares what it needs and what it serves. A scaffold that omits
/// `capabilities` compiles but is granted nothing, so every non-primordial
/// service call would answer `permission-denied`; one that omits `routes` never
/// sees `handle-request` at all. Pin both, plus the consolidated service names.
#[test]
fn lib_rs_declares_capabilities_and_routes() {
    let src = render_lib_rs("my-app");
    assert!(
        src.contains("capabilities: vec!["),
        "the descriptor must request capabilities"
    );
    for capability in [
        "types::Capability::Routes",
        "types::Capability::DocstoreRead",
        "types::Capability::DocstoreWrite",
        "types::Capability::Filesystem",
        "types::Capability::Streams",
        "types::Capability::Scheduler",
        "types::Capability::Notifications",
        "types::Capability::Pubsub",
    ] {
        assert!(
            src.contains(capability),
            "the scaffold uses the service, so it must ask for `{capability}`"
        );
    }
    assert!(
        !src.contains("Capability::Mqtt") && !src.contains("mqtt"),
        "MQTT is not part of the contract"
    );
    // One HTTP route (the whole prefix) and one stream route with its listener.
    assert!(src.contains("routes: vec!["));
    assert!(src.contains("kind: descriptor::RouteKind::Http"));
    assert!(src.contains("kind: descriptor::RouteKind::Stream"));
    assert!(src.contains("listener: Some(types::StreamListen {"));
    assert!(src.contains("auth_mode: types::RouteAuth::None"));
    assert!(src.contains("website: None"));
    // A published channel must be declared, or `io::publish` refuses it.
    assert!(src.contains("subscriptions: Some(vec![descriptor::Subscription {"));
    assert!(src.contains("channel: \"app:announcements\".to_string()"));
    // The scaffold exercises the consolidated API it declares.
    assert!(src.contains("data::get_docs(&doctype, &[name.clone()])"));
    assert!(src.contains("data::next_name(series)"));
    assert!(src.contains("platform::current_user()"));
    assert!(src.contains("platform::can(&[platform::PermissionCheck {"));
}

#[test]
fn dev_config_is_sqlite_with_dev_roles() {
    let cfg = render_ferris_toml("my-app");
    assert!(cfg.contains("type = \"sqlite\""));
    assert!(cfg.contains("roles = [\"api\", \"web\", \"realtime\"]"));
    // The server publishes `ferris build`'s output from here; an omitted
    // section would fall back to the same default but hide the contract.
    assert!(cfg.contains("wasm_dir = \"plugins\""));
    assert!(cfg.contains("enabled = []"));
}

/// Without `targets` in the pin, a fresh machine fails at first build
/// with `can't find crate for core` (H-J).
#[test]
fn toolchain_pins_the_component_target() {
    let toml = render_toolchain_toml();
    assert!(toml.contains("targets = [\"wasm32-wasip2\"]"));
    assert!(toml.contains("channel = \"1.98.1\""));
}

#[test]
fn accepts_valid_project_names() {
    for name in ["app", "my-app", "my_app", "App42"] {
        assert!(validate_project_name(name).is_ok(), "rejected {name:?}");
    }
}

#[test]
fn rejects_unsafe_project_names() {
    for bad in ["", "1app", "app/name", "app name", "app\"x", "..", "-app"] {
        assert!(validate_project_name(bad).is_err(), "accepted {bad:?}");
    }
}

/// The README ships to every new project, so it must describe the same contract
/// as the scaffold: the current WIT version, its six import interfaces, and one
/// explanation of grants versus compiled features (it used to repeat the
/// feature-detection paragraph and name only five interfaces).
#[test]
fn the_readme_describes_the_current_contract_once() {
    let readme = render_readme("my-app");
    assert!(readme.contains("ferris:framework@0.3.0"), "{readme}");
    assert!(readme.contains("six interfaces"), "{readme}");
    for interface in ["types", "data", "schema", "io", "platform", "descriptor"] {
        assert!(readme.contains(&format!("`{interface}`")), "{interface}");
    }
    assert_eq!(
        readme.matches("platform::features()").count(),
        1,
        "the feature-detection paragraph must not be repeated"
    );
    assert!(
        !readme.contains("`streams` and `streams`"),
        "the scaffold needs exactly one specific grant"
    );
}
