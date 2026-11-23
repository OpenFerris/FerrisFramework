//! Probe guest for the world mount: it reaches `std::fs`, so it imports
//! `wasi:filesystem/types`. Without the `filesystem` grant the selective linker
//! registers no filesystem at all and instantiation fails with a clear
//! missing-linker error; with the grant the host mounts the app's world at
//! `/world` and every path is confined to it.

wit_bindgen::generate!({
    world: "ferris-app",
    path: "../../../wit/ferris.wit",
});

struct Probe;

/// A 200 text reply (the host defaults the content-type).
fn text(body: String) -> Response {
    Response {
        status: 200,
        headers: vec![],
        body: body.into_bytes(),
    }
}

/// The typed descriptor: one catch-all route and no doctypes/UI/methods (every
/// list `[]`, every optional section `None`). The contract requires the capability
/// request and the mounted routes; nothing here reaches a host service.
impl Guest for Probe {
    fn describe() -> Result<ferris::framework::descriptor::Descriptor, Error> {
        Ok(ferris::framework::descriptor::Descriptor {
            locales: None,
            capabilities: vec![
                ferris::framework::types::Capability::Routes,
                ferris::framework::types::Capability::Filesystem,
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

    fn handle_request(_route_id: String, req: HttpRequest) -> Result<Response, Error> {
        match req.path.as_str() {
            // An absolute host path: the preopen is the only filesystem, so
            // this must never resolve.
            "/fs" => std::fs::read_to_string("/definitely/not/here")
                .map_err(|e| Error::NotFound(e.to_string()))
                .map(text),
            // Relative to the initial cwd (`/world`): writes, reads back and
            // reports the size. The file stays for the host-side assertion.
            "/write" => {
                let path = std::path::Path::new("/world/data/probe.txt");
                std::fs::write(path, b"inside the world")
                    .map_err(|e| Error::Internal(e.to_string()))?;
                let bytes = std::fs::read(path).map_err(|e| Error::Internal(e.to_string()))?;
                Ok(text(format!("{}", bytes.len())))
            }
            // `..` from the world root must be refused by the runtime.
            "/escape" => match std::fs::read_to_string("../outside.txt") {
                Ok(content) => Ok(text(format!("ESCAPED:{content}"))),
                Err(e) => Ok(text(format!("refused:{e}"))),
            },
            // A sibling app's world: also unreachable.
            "/sibling" => match std::fs::read_to_string("/world/../other/secrets/key") {
                Ok(content) => Ok(text(format!("ESCAPED:{content}"))),
                Err(e) => Ok(text(format!("refused:{e}"))),
            },
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
export!(Probe);
