//! Probe guest for the selective-WASI linker test: exercises the interfaces
//! a `wasm32-wasip2` std uses **on demand** — `std::time::SystemTime` and
//! `chrono` (wasi:clocks/wall-clock), `uuid::Uuid::new_v4` (wasi:random via
//! getrandom), `std::env::var` and a stdin read (wasi:cli/{environment,stdin}
//! stay linked but the host context hands them empty/closed) — so the host
//! test can prove both halves: what must keep working, and what must leak
//! nothing.
//!
//! It also carries the typed HTTP contract: `/echo` reflects
//! method/query/headers/body bytes back verbatim and `/teapot` answers 418 with
//! its own headers, so the host tests can prove the full request crosses and the
//! full response passes back. The descriptor also declares capabilities, routes
//! and the `handle-stream-event` lifecycle export.

wit_bindgen::generate!({
    world: "ferris-app",
    path: "../../../wit/ferris.wit",
});

/// A 200 text reply; the host supplies `content-type: text/plain` when the
/// response sets none.
fn text(body: String) -> Response {
    Response {
        status: 200,
        headers: vec![],
        body: body.into_bytes(),
    }
}

struct Probe;

/// The typed descriptor: capabilities, one catch-all route, and no doctypes,
/// UI or API methods. Every list is `[]` and every optional section is `None`.
///
/// `routes` is what makes `handle-request` reachable at all ("empty:
/// `handle-request` is never called"), so the probe claims the whole namespace
/// with a single prefix route. `docstore-write` covers the `/write-eq` route
/// below: declaring a capability is a request the operator may refuse, and the
/// host then answers `permission-denied` instead of silently narrowing.
impl Guest for Probe {
    fn describe() -> Result<ferris::framework::descriptor::Descriptor, Error> {
        Ok(ferris::framework::descriptor::Descriptor {
            capabilities: vec![
                ferris::framework::types::Capability::Routes,
                ferris::framework::types::Capability::DocstoreWrite,
                // The network probe: the server's integration tests drive the
                // host-owned sockets, so the fixture declares the surfaces it
                // exercises (declaration is the grant in this model).
                ferris::framework::types::Capability::Streams,
                ferris::framework::types::Capability::StreamsListen,
                ferris::framework::types::Capability::TlsClient,
                ferris::framework::types::Capability::TlsServer,
                ferris::framework::types::Capability::Udp,
                ferris::framework::types::Capability::RawSockets,
                ferris::framework::types::Capability::DnsServer,
                ferris::framework::types::Capability::Process,
                // The stream tests connect to loopback peers with
                // `allow_private`; the policy only honours that bit with this
                // grant (WASM-006).
                ferris::framework::types::Capability::HttpUnrestricted,
            ],
            routes: vec![ferris::framework::descriptor::Route {
                id: "probe".to_string(),
                method: "*".to_string(),
                path: "/".to_string(),
                kind: ferris::framework::descriptor::RouteKind::Http,
                auth: None,
                body_limit: None,
                tls_server: None,
                listener: None,
            }],
            auth_mode: ferris::framework::types::RouteAuth::None,
            http_policy: None,
            tls_client: None,
            tls_server: None,
            doctypes: Vec::new(),
            themes: Vec::new(),
            folders: Vec::new(),
            workspaces: Vec::new(),
            methods: Vec::new(),
            ui_extensions: None,
            dashboards: None,
            widgets: None,
            website: None,
            assets: None,
            subscriptions: None,
            config: None,
            plugin_version: None,
            requires: None,
        })
    }

    /// The host dispatches with the id of the route it matched. The probe
    /// declares one catch-all route and parses the path itself: the host
    /// matches the prefix, the guest owns the rest. The id is deliberately not validated: the host also dispatches an
    /// app-wide request under its own reserved id, and a fixture that rejected
    /// anything but its declared id would fail that path for no reason.
    fn handle_request(_route_id: String, req: HttpRequest) -> Result<Response, Error> {
        match req.path.as_str() {
            "/time" => {
                let unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| Error::Internal(e.to_string()))?
                    .as_millis();
                let chrono_ms = chrono::Utc::now().timestamp_millis();
                Ok(text(format!("{unix_ms}|{chrono_ms}")))
            }
            "/rand" => Ok(text(uuid::Uuid::new_v4().to_string())),
            // PATH exists in every sane host env: a non-empty answer means the
            // host called `inherit_env`.
            "/env" => Ok(text(std::env::var("PATH").unwrap_or_default())),
            // Closed stdin must yield immediate EOF (""), not host input.
            "/stdin" => {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .map_err(|e| Error::Internal(e.to_string()))?;
                Ok(text(buf))
            }
            // Typed request in, typed response out: `METHOD|QUERY|header|`
            // followed by the raw body bytes (binary-safe both ways).
            "/echo" => {
                let header = req
                    .headers
                    .iter()
                    .find(|(name, _)| name == "x-probe-key")
                    .map(|(_, value)| value.clone())
                    .unwrap_or_else(|| "-".to_string());
                let mut body = format!("{}|{}|{}|", req.method, req.query, header).into_bytes();
                body.extend(req.body);
                Ok(Response {
                    status: 200,
                    headers: vec![("x-probe".to_string(), "echo".to_string())],
                    body,
                })
            }
            // H7 proof: the guest performs the exact host-side write the
            // SolmecBOX hot path does (a `write-doc` on its own doctype) and
            // reports the outcome, so the host test can prove a
            // certificate-authenticated, session-less request is authorised
            // under certificate service mode and denied without it.
            //
            // A no-events write is a *disposition*: `silent` persists and
            // stamps the row but re-enters none of the app's own handlers,
            // which is what a guest call the host cannot re-enter needs.
            "/write-eq" => {
                let body = r#"{"name":"H7-1","probe":"ok"}"#;
                let write = ferris::framework::data::Write {
                    kind: ferris::framework::data::OpKind::Upsert,
                    doctype: "Equipo".to_string(),
                    name: None,
                    data: Some(body.to_string()),
                    disposition: ferris::framework::data::WriteDisposition::Silent,
                };
                match ferris::framework::data::write_doc(&write) {
                    Ok(Some(doc)) => Ok(text(format!("ok|{}", doc.name))),
                    Ok(None) => Err(Error::Internal("upsert returned no document".to_string())),
                    Err(e) => Ok(Response {
                        status: 403,
                        headers: vec![("content-type".to_string(), "text/plain".to_string())],
                        body: format!("denied|{e}").into_bytes(),
                    }),
                }
            }
            // A status and headers of the guest's own choosing.
            "/teapot" => Ok(Response {
                status: 418,
                headers: vec![(
                    "content-type".to_string(),
                    "text/teapot; charset=utf-8".to_string(),
                )],
                body: b"short and stout".to_vec(),
            }),
            _ => Err(Error::NotFound(format!("unknown route: {}", req.path))),
        }
    }

    fn doctype_event(
        _event: String,
        _doctype: String,
        _name: String,
        _data: String,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn ui_asset(_path: String) -> Result<Vec<u8>, Error> {
        Err(Error::NotFound("no assets".to_string()))
    }

    fn ui_widget(_widget: String, _params: Vec<(String, String)>) -> Result<String, Error> {
        Err(Error::NotFound("no widget".to_string()))
    }

    /// Echo the chunk back so a host test can prove the whole loop: peer frame →
    /// `handle-stream` → the bytes the host relays to the peer. The guest keeps
    /// no per-connection state, which is enough to tell "the frame crossed" from
    /// "the host dropped it".
    fn handle_stream(_stream_id: String, chunk: Vec<u8>) -> Result<Vec<u8>, Error> {
        Ok(chunk)
    }

    /// Chunks are data only; open/close/error arrive here. The probe
    /// has no per-connection state, so it only proves the export is dispatched.
    fn handle_stream_event(_stream_id: String, _event: StreamEvent) -> Result<(), Error> {
        Ok(())
    }

    /// Echo the datagram back, like `handle_stream`, so a host test that opens
    /// a UDP or raw socket can prove the datagram crossed the boundary. The
    /// probe declares no datagram capability, so this is only reachable when
    /// the host itself calls the export.
    fn handle_datagram(_stream_id: String, data: Vec<u8>, _peer: String) -> Result<Vec<u8>, Error> {
        Ok(data)
    }

    /// Echo the job back so the host test can prove both the `job-type` and the
    /// JSON `payload` crossed the boundary intact.
    fn handle_job(job_type: String, payload: String) -> Result<String, Error> {
        Ok(format!("{job_type}|{payload}"))
    }
}

// Generate the component exports (trampolines + ABI).
export!(Probe);
