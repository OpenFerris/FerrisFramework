//! Probe guest for the trap-recovery regression test: `/ping` answers `pong`
//! (a healthy instance), `/trap` executes the wasm `unreachable` instruction,
//! which poisons the host's `wasmtime::Store` — the exact failure class the
//! framework must recover from by publishing a fresh instance.
//!
//! Everything else is a copy of the minimal `probe-ok` guest: one catch-all
//! route, the typed descriptor (capabilities + routes the contract requires), no-op
//! lifecycle exports, `/teapot` + `/echo` kept for symmetry with the echo
//! battery.

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

struct ProbeTrap;

impl Guest for ProbeTrap {
    fn describe() -> Result<ferris::framework::descriptor::Descriptor, Error> {
        Ok(ferris::framework::descriptor::Descriptor {
            capabilities: vec![ferris::framework::types::Capability::Routes],
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

    fn handle_request(_route_id: String, req: HttpRequest) -> Result<Response, Error> {
        match req.path.as_str() {
            "/ping" => Ok(text("pong".to_string())),
            // Unlike a returned `Error::Internal`, this raises a real wasm
            // trap (`unreachable` instruction): the host observes a
            // `wasmtime::Error` and the instance can never be entered again.
            "/trap" => core::arch::wasm32::unreachable(),
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

    fn handle_stream(_stream_id: String, _chunk: Vec<u8>) -> Result<Vec<u8>, Error> {
        Ok(Vec::new())
    }

    fn handle_stream_event(_stream_id: String, _event: StreamEvent) -> Result<(), Error> {
        Ok(())
    }

    /// The world exports a datagram entry point; this probe opens no datagram
    /// socket, so it drops whatever arrives.
    fn handle_datagram(
        _stream_id: String,
        _data: Vec<u8>,
        _peer: String,
    ) -> Result<Vec<u8>, Error> {
        Ok(Vec::new())
    }

    fn handle_job(_job_type: String, _payload: String) -> Result<String, Error> {
        Ok(String::new())
    }
}

// Generate the component exports (trampolines + ABI).
export!(ProbeTrap);
