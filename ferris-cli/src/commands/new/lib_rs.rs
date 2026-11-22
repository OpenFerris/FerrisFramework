//! The guest `src/lib.rs` the scaffold writes.

use anyhow::Result;
use std::fs;
use std::path::Path;

/// Guest implementation of the `ferris-app` world.
///
/// The `generate!` macro emits the component glue plus the guest-side bindings
/// for the framework imports (`ferris::framework::{data,io,platform,types}`).
/// The nine exported functions below are what the framework calls, and between
/// them they use every service the scaffold asks a capability for: a scaffold
/// whose first `cargo check` prints warnings is a scaffold nobody reads past the
/// warnings. This file is deliberately small; real apps grow it and register
/// their own DocTypes via `describe`.
// One large scaffold template by design: readability of the rendered crate
// beats the arbitrary line budget of a code generator.
#[allow(clippy::too_many_lines)]
pub(super) fn render_lib_rs(name: &str) -> String {
    format!(
        r##"//! {name} — Ferris app component.
//!
//! Pure compute: everything the app needs (documents, naming, user, scheduler,
//! pubsub, its own world on disk, notifications and stream metadata) comes from the
//! framework through the `wit/ferris.wit` imports. The app never touches the
//! database directly.
//!
//! The contract is `ferris:framework@0.3.0`: six interfaces (`types`, `data`,
//! `schema`, `io`, `platform`, `descriptor`) and an explicit capability
//! declaration in `describe()`. The primordial capabilities (the app's world,
//! its routes, outbound HTTP, the document store, config, schema, the
//! scheduler, notifications and pubsub) are always on; declaring a specific one
//! grants it, and a call outside the resulting set fails with
//! `permission-denied`.
//! `platform::features()` is the other half: it reports which verticals the
//! **host binary** was compiled with, so the app can hide an affordance whose
//! subsystem is absent.

wit_bindgen::generate!({{
    world: "ferris-app",
    path: "wit/ferris.wit",
}});

// Framework imports (generated at macro expansion site). `Error`,
// `HttpRequest`, `Response` and `StreamEvent` are re-exported at crate root by
// the world's `use` clauses; the four service modules cover documents, the
// network, platform services and the typed descriptor.
use ferris::framework::{{data, descriptor, io, platform, types}};

/// A plain 200 reply. The host defaults `content-type` to
/// `text/plain; charset=utf-8` when the `Response` sets none; set
/// `status`/`headers` explicitly for redirects, JSON or downloads.
fn text(body: String) -> Response {{
    Response {{ status: 200, headers: vec![], body: body.into_bytes() }}
}}

struct MyComponent;

impl Guest for MyComponent {{
    /// What the app declares, as the typed WIT `descriptor`: the capabilities
    /// it asks for, the routes it owns, the Doctype(s) it publishes (bare field
    /// names have their fieldtype inferred). The descriptor schema is documented on the
    /// `describe` export in `wit/ferris.wit`.
    fn describe() -> Result<descriptor::Descriptor, Error> {{
        Ok(descriptor::Descriptor {{
            // What the app declares. The primordial capabilities (its world,
            // routes, outbound HTTP, the document store, config, schema, the
            // scheduler, notifications and pubsub) are always on; declaring a
            // specific one grants it, and the host answers `permission-denied`
            // for any call outside the resulting set, which `platform::grants`
            // reports. This scaffold uses documents, naming series, its own
            // world, streams, schedules, notifications and pubsub.
            capabilities: vec![
                types::Capability::Routes,
                types::Capability::DocstoreRead,
                types::Capability::DocstoreWrite,
                // `filesystem` mounts `plugins/<app>/` at `/world`; every file
                // the app owns (settings, uploads, secrets) lives there.
                types::Capability::Filesystem,
                types::Capability::Streams,
                types::Capability::Scheduler,
                types::Capability::Notifications,
                types::Capability::Pubsub,
            ],
            // The mounted surface. `path` is a **prefix** matched segment-wise
            // under `/plugins/{{name}}`; the longest match wins and `"/"` claims
            // the whole namespace. The host interprets no placeholders: it
            // dispatches with the matched `route-id` and the guest parses
            // `http-request.path`. An app that declares no route never sees
            // `handle-request`.
            routes: vec![
                descriptor::Route {{
                    id: "app".to_string(),
                    method: "*".to_string(),
                    path: "/".to_string(),
                    kind: descriptor::RouteKind::Http,
                    auth: None,
                    body_limit: None,
                    tls_server: None,
                    listener: None,
                }},
                // A `%stream` route carries its listener: the host binds and
                // accepts, and each connection reaches the guest as a
                // `stream-id` (frames via `handle-stream`, lifecycle via
                // `handle-stream-event`). A websocket under the app prefix
                // rides the host's own socket, so `streams` covers it; a
                // `tcp`/`tls` listener on an address the app owns would need
                // `streams-listen` (trusted-only) as well.
                descriptor::Route {{
                    id: "socket".to_string(),
                    method: "*".to_string(),
                    path: "/ws".to_string(),
                    kind: descriptor::RouteKind::Stream,
                    auth: None,
                    body_limit: None,
                    tls_server: None,
                    listener: Some(types::StreamListen {{
                        transport: types::StreamTransport::Websocket,
                        // `none` lets the host infer the bind address (the app
                        // prefix for a path); `port` `none`/`0` asks for an
                        // inferred port, reported by `stream-describe`.
                        address: Some("/ws".to_string()),
                        port: None,
                        tls: None,
                        auth: None,
                        max_frame: None,
                        idle_timeout_seconds: None,
                    }}),
                }},
            ],
            // The default for a route that declares none of its own. `none`
            // makes this app's routes public and leaves authentication to the
            // guest; `session` has the host answer 401 before dispatch and bind
            // the caller to `platform::current-user`.
            auth_mode: types::RouteAuth::None,
            http_policy: None,
            tls_client: None,
            tls_server: None,
            doctypes: vec![descriptor::Doctype {{
                name: "Task".to_string(),
                is_submittable: false,
                is_single: false,
                allow_submit: false,
                allow_cancel: false,
                allow_amend: false,
                allow_delete: false,
                allow_copy: false,
                allow_print: false,
                allow_rename: false,
                in_calendar: false,
                in_kanban: false,
                in_gantt: false,
                in_tree_view: false,
                in_report_view: false,
                // Which view a document opens in. Beside `DocView::Form`
                // (this scaffold), the cases are `DashboardNative`,
                // `DashboardFixed(id)`, `DashboardLink(field)` and `Custom`
                // — see `doc-view` in wit/ferris.wit.
                view: descriptor::DocView::Form,
                fields: vec![
                    descriptor::Field::Name("subject".to_string()),
                    descriptor::Field::Name("status".to_string()),
                    descriptor::Field::Name("due_date".to_string()),
                ],
                indexes: Vec::new(),
                list_default_order: None,
            }}],
            themes: Vec::new(),
            folders: Vec::new(),
            workspaces: Vec::new(),
            methods: Vec::new(),
            ui_extensions: None,
            dashboards: None,
            widgets: None,
            // Website pages are a plain list — each entry is
            // `(path, component, kind)` with `kind` `"page"` (inside the desk
            // chrome) or `"bare"`, and no wrapper record. The `component` is a
            // custom element the app ships through `ui-asset`.
            website: None,
            assets: None,
            // A pubsub channel the app publishes to. Declaring it is what makes
            // `io::publish` accept the channel; the host refuses the rest.
            subscriptions: Some(vec![descriptor::Subscription {{
                channel: "app:announcements".to_string(),
            }}]),
            config: None,
            plugin_version: None,
            requires: None,
        }})
    }}

    /// Route an HTTP request. The host dispatches with the id of the route it
    /// matched (`route-id`), so one export serves every endpoint without the
    /// guest re-deriving which declaration matched. `req` carries the method,
    /// the path after the app prefix, the raw query string, the headers and the
    /// body **bytes** (binary-safe). The `Response` picks the status, headers
    /// and body the host sends back.
    fn handle_request(route_id: String, req: HttpRequest) -> Result<Response, Error> {{
        // A declared API method arrives with the reserved route id
        // `method:<name>` and the JSON params as the body (see
        // `describe().methods`); a declared route arrives with its own id.
        if route_id.starts_with("method:") {{
            return Err(Error::NotFound(format!(
                "no API method implemented: {{route_id}}"
            )));
        }}
        let body = String::from_utf8_lossy(&req.body).into_owned();
        match (req.path.as_str(), body.as_str()) {{
            ("/ping" | "/", _) => Ok(text("pong".to_string())),
            // Naming series, resolved by the host: `SAL-.-YYYY-MM-####`.
            // Requires `docstore-write`: a series allocates, it does not read.
            ("/next-name", series) => {{
                let name = data::next_name(series)?;
                Ok(text(name))
            }}
            // Who is calling, and what they are allowed to do.
            ("/whoami", _) => {{
                let user = platform::current_user()?;
                Ok(text(format!(
                    "{{}} <{{}}> [{{}}]",
                    user.name,
                    user.email,
                    user.roles.join(", ")
                )))
            }}
            // Ask one batch of permission questions: one boolean per check, in
            // input order. Without a `name` the check is DocType-wide; with one
            // it is row-level, so `if_owner` grants are honoured.
            ("/can", _) => {{
                let allowed = platform::can(&[platform::PermissionCheck {{
                    doctype: "Task".to_string(),
                    action: "read".to_string(),
                    name: None,
                }}])?;
                let read_task = allowed.first().copied().unwrap_or(false);
                Ok(text(format!("can_read_task={{read_task}}")))
            }}
            // Write a file inside the app's own world (`/world`) and read it
            // back. The mount is confined: `..` and absolute host paths do not
            // resolve, and no other app's world is reachable.
            ("/store-file", _) => {{
                let path = "/world/data/note.txt";
                std::fs::write(path, &req.body)
                    .map_err(|e| types::Error::Internal(e.to_string()))?;
                let bytes = std::fs::read(path)
                    .map_err(|e| types::Error::Internal(e.to_string()))?;
                Ok(text(format!("stored note.txt ({{}} bytes)", bytes.len())))
            }}
            // Ask the host to deliver an in-app notification.
            ("/notify", to) => {{
                io::notify(to, "in-app", "Hello", "You were mentioned")?;
                Ok(text("sent".to_string()))
            }}
            // Fan an event out to live subscribers on `channel:<channel>`.
            ("/broadcast", payload) => {{
                io::publish("app:announcements", "message", payload)?;
                Ok(text("broadcast".to_string()))
            }}
            // Feature detection: what the **host binary** was compiled with,
            // sorted (`api`, `web`, `realtime`, `jobs`, `scheduler`, `email`,
            // `files`, `print`, `report`, `integrations`, `wasm`, `full`). This is not a grant — `platform::grants` is — it is how
            // the app decides which affordances exist at all.
            ("/features", _) => {{
                let features = platform::features()?;
                Ok(text(features.join(",")))
            }}
            // Status and headers are the app's to choose: a redirect, a 201
            // or a download are plain `Response` values (the host drops
            // hop-by-hop and framework-owned headers).
            ("/teapot", _) => Ok(Response {{
                status: 418,
                headers: vec![("x-template".to_string(), "1".to_string())],
                body: b"short and stout".to_vec(),
            }}),
            (unknown, _) => Err(Error::NotFound(format!("unknown route: {{unknown}}"))),
        }}
    }}

    /// DocType lifecycle event: created / after-save / submitted / cancelled /
    /// trashed — exactly the points the host dispatches today (`before-*` is
    /// not wired yet). `data` is the document as JSON.
    fn doctype_event(
        event: String,
        doctype: String,
        name: String,
        data: String,
    ) -> Result<(), Error> {{
        match event.as_str() {{
            // Read back what the host actually stored and compare it with what
            // I was handed, rather than trusting either side. `get-docs` is
            // positional: entry 0 is the document named `name`, or `none`.
            "after-save" => {{
                let stored = data::get_docs(&doctype, &[name.clone()])?
                    .into_iter()
                    .next()
                    .flatten();
                match stored {{
                    Some(doc) if doc.data != data => {{
                        return Err(Error::Internal(format!(
                            "{{doctype}} {{name}}: stored data differs from the data I was given"
                        )));
                    }}
                    Some(_) => {{}}
                    // A row that vanished between the write and the read-back
                    // is a real defect, not something to swallow.
                    None => {{
                        return Err(Error::NotFound(format!(
                            "{{doctype}} {{name}} is gone after the event"
                        )));
                    }}
                }}
            }}
            // Ask the host to register a cron schedule: `cron` is required
            // (5-field; the host adds the seconds field), `job_name` names the
            // job the host enqueues when it fires; `payload`, `queue`,
            // `priority` and `tags` are optional. The call returns the new
            // schedule's id (which `platform::cancel-schedule` /
            // `list-schedules` use); this scaffold lets the host own it.
            "submitted" => {{
                platform::schedule(
                    "{{\"cron\":\"0 3 * * *\",\"job_name\":\"task_nightly\",\"payload\":{{\"doctype\":\"Task\"}}}}",
                )?;
            }}
            _ => {{}}
        }}
        Ok(())
    }}

    /// One frame of a live binary stream. **Chunks are data only**:
    /// open and close arrive through `handle-stream-event`, so an empty frame
    /// is a legitimate frame and framing is the app's business.
    /// `io::stream-describe` says which transport this `stream-id` arrived on.
    fn handle_stream(stream_id: String, chunk: Vec<u8>) -> Result<Vec<u8>, Error> {{
        // What transport is this stream on? Best effort, for diagnostics.
        let info = io::stream_describe(&stream_id)?;
        // Echo the frame back so raw sockets are easy to probe. WebSocket
        // frames also fan out to live pubsub subscribers, so a stream can be
        // watched from the desk.
        if info.transport.as_str() == "websocket" {{
            io::publish(
                "app:announcements",
                "frame",
                &String::from_utf8_lossy(&chunk),
            )?;
        }}
        Ok(chunk)
    }}

    /// Lifecycle of one stream, once per stream and in order: `opened`, then
    /// any number of chunks, then `closed` or `error`. Per-connection state
    /// belongs here (create on open, release on close); no cleanup is inferred
    /// from a chunk.
    fn handle_stream_event(stream_id: String, event: StreamEvent) -> Result<(), Error> {{
        let payload = match event {{
            StreamEvent::Opened => {{
                let info = io::stream_describe(&stream_id)?;
                format!("opened {{}} ({{}})", stream_id, info.transport)
            }}
            StreamEvent::Closed => format!("closed {{stream_id}}"),
            StreamEvent::Error(reason) => format!("error {{stream_id}}: {{reason}}"),
        }};
        io::publish("app:announcements", "stream_event", &payload)?;
        Ok(())
    }}

    /// One inbound datagram on a UDP or raw/ICMP socket the host opened for the
    /// app. `stream-id` is the socket, `peer` the sender and `data` the payload
    /// as the host received it. Returning bytes sends one datagram back to
    /// `peer`; an empty list drops it. UDP and raw sockets need the `udp` /
    /// `raw-sockets` capability and are opened through `io::udp-connect`,
    /// `io::udp-listen` or `io::raw-open` — the guest never sees a socket.
    fn handle_datagram(
        stream_id: String,
        data: Vec<u8>,
        _peer: String,
    ) -> Result<Vec<u8>, Error> {{
        // Echo the payload so a request/response protocol needs no second
        // send call; `stream-describe` names the datagram protocol when the
        // app must branch between UDP, raw and ICMP.
        let info = io::stream_describe(&stream_id)?;
        if info.protocol.as_deref() == Some("udp") {{
            io::publish(
                "app:announcements",
                "datagram",
                &String::from_utf8_lossy(&data),
            )?;
        }}
        Ok(data)
    }}

    /// Run one background job the host routes here. Jobs come from a cron
    /// schedule registered with `platform::schedule` above (its `job_name`
    /// arrives as `job_type`) or from `platform::enqueue-job`; `payload` is the
    /// job's JSON body. Return the job's result, or an `Err` to fail it (the
    /// worker retries, then dead-letters) — there is no silent fallback.
    fn handle_job(job_type: String, payload: String) -> Result<String, Error> {{
        match job_type.as_str() {{
            // The nightly cron the `submitted` handler registers above.
            "task_nightly" => Ok(format!("ran task_nightly with {{payload}}")),
            other => Err(Error::NotFound(format!("unknown job: {{other}}"))),
        }}
    }}

    /// Serve one UI asset to the Desk (script, stylesheet, image). The host
    /// relays the bytes at `/plugins/{{name}}/asset/{{path}}` with a content
    /// type guessed from the extension, so a plugin's Web Components can ship
    /// their own JS/CSS. These bytes are served **same-origin** and run as
    /// first-party code in the browser: trusted app code, not sandboxed guest
    /// code.
    fn ui_asset(path: String) -> Result<Vec<u8>, Error> {{
        Err(Error::NotFound(format!("ui asset not found: {{path}}")))
    }}

    /// SSR markup for one dashboard/website widget. The desk prints this HTML
    /// before the widget's custom element hydrates, or while its script is
    /// still loading through `/plugins/{{name}}/asset/...`. `params` carries
    /// the instance's (name, value) pairs as declared in the dashboard.
    fn ui_widget(widget: String, _params: Vec<(String, String)>) -> Result<String, Error> {{
        Err(Error::NotFound(format!("widget not found: {{widget}}")))
    }}
}}

// Generate the component exports (trampolines + ABI).
export!(MyComponent);
"##
    )
}

pub(super) fn write_lib_rs(project_dir: &Path, name: &str) -> Result<()> {
    fs::write(project_dir.join("src").join("lib.rs"), render_lib_rs(name))?;
    Ok(())
}
