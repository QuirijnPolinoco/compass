//! The localhost HTTP + SSE server (ADR-0005). Bound to `127.0.0.1` only, read-only,
//! synchronous (thread-per-SSE-stream) via `tiny_http` — no async runtime.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tiny_http::{Header, Method, Request, Response, Server};

use crate::{MapState, DEFAULT_PORT};

/// How long an idle SSE connection waits before emitting a keep-alive comment.
const SSE_KEEPALIVE: Duration = Duration::from_secs(15);

/// A bound, not-yet-running viz server. Holds the listener so the caller can read the
/// actual address (to print/open) before handing control to [`VizServer::run`].
pub struct VizServer {
    server: Server,
    addr: SocketAddr,
}

/// Bind the viz server on `127.0.0.1`.
///
/// With no `preferred` port we try the uncommon default ([`DEFAULT_PORT`]) and, if it's
/// taken, fall back to an OS-assigned free port — so the server can never collide with the
/// user's other services or containers (ADR-0005). A `preferred` port is honored exactly
/// (an error if it's busy), so `--port` gives a stable, bookmarkable URL.
pub fn bind(preferred: Option<u16>) -> std::io::Result<VizServer> {
    let loopback = Ipv4Addr::LOCALHOST;
    let server = match preferred {
        Some(port) => Server::http((loopback, port)).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("could not bind 127.0.0.1:{port}: {e}"),
            )
        })?,
        None => match Server::http((loopback, DEFAULT_PORT)) {
            Ok(server) => server,
            // Default busy → let the OS pick any free port (port 0).
            Err(_) => Server::http((loopback, 0)).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!("could not bind a localhost port: {e}"),
                )
            })?,
        },
    };

    let addr = server
        .server_addr()
        .to_ip()
        .unwrap_or_else(|| SocketAddr::from((loopback, DEFAULT_PORT)));

    Ok(VizServer { server, addr })
}

impl VizServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Serve requests until the process ends. Blocking — the CLI runs this on its own thread
    /// while the watch loop keeps the map fresh.
    pub fn run(self, state: Arc<MapState>) {
        for request in self.server.incoming_requests() {
            handle(request, &state);
        }
    }
}

fn handle(request: Request, state: &Arc<MapState>) {
    let raw = request.url().to_owned();
    let path = raw.split('?').next().unwrap_or("/");
    let is_get = *request.method() == Method::Get;

    if !is_get {
        let _ = request.respond(Response::from_string("method not allowed").with_status_code(405));
        return;
    }

    match path {
        "/" => serve_str(request, render_index(), "text/html; charset=utf-8"),
        "/app.js" => serve_str(
            request,
            crate::render::APP_JS,
            "application/javascript; charset=utf-8",
        ),
        "/app.css" => serve_str(request, crate::render::APP_CSS, "text/css; charset=utf-8"),
        "/vendor/cytoscape.min.js" => serve_str(
            request,
            crate::render::CYTOSCAPE_JS,
            "application/javascript; charset=utf-8",
        ),
        "/graph" => serve_graph(request, state, &raw),
        "/subgraph" => serve_subgraph(request, state, &raw),
        // Read-only local token-savings dashboard (loopback only): the page and the JSON it
        // fetches, aggregated from `<repo>/.compass/sessions/*.tokens.json`.
        "/tokens" => serve_str(
            request,
            crate::render::TOKENS_HTML,
            "text/html; charset=utf-8",
        ),
        "/api/session-tokens" => serve_json(
            request,
            crate::session_tokens::summary_json(state.repo_root()),
        ),
        "/events" => {
            // SSE holds the connection open; run it off the accept loop.
            let state = Arc::clone(state);
            thread::spawn(move || stream_events(request, state));
        }
        _ => {
            let _ = request.respond(Response::from_string("not found").with_status_code(404));
        }
    }
}

/// The served index page (live mode — the app fetches `/graph`).
fn render_index() -> &'static str {
    crate::render::INDEX_HTML
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn serve_str(request: Request, body: &str, content_type: &str) {
    let response = Response::from_string(body).with_header(header("Content-Type", content_type));
    let _ = request.respond(response);
}

fn serve_json(request: Request, json: String) {
    let response = Response::from_string(json)
        .with_header(header("Content-Type", "application/json; charset=utf-8"));
    let _ = request.respond(response);
}

fn serve_graph(request: Request, state: &Arc<MapState>, raw_url: &str) {
    let include_symbols = query_param(raw_url, "symbols").as_deref() == Some("1");
    let view = state.graph_view(include_symbols);
    serve_json(
        request,
        crate::render::graph_payload_json(&view, state.version()),
    );
}

fn serve_subgraph(request: Request, state: &Arc<MapState>, raw_url: &str) {
    let Some(file) = query_param(raw_url, "file") else {
        let _ = request.respond(Response::from_string("missing ?file=").with_status_code(400));
        return;
    };
    let depth = query_param(raw_url, "depth")
        .and_then(|d| d.parse::<usize>().ok())
        .unwrap_or(1);
    match state.subgraph(&file, depth) {
        Some(sub) => serve_json(request, serde_json::to_string(&sub).unwrap_or_default()),
        None => {
            let _ = request.respond(Response::from_string("file not in map").with_status_code(404));
        }
    }
}

/// Stream `update` events to one browser tab over SSE until it disconnects.
fn stream_events(request: Request, state: Arc<MapState>) {
    let mut writer = request.into_writer();
    // We own the raw stream, so we write the HTTP response head ourselves.
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream\r\n\
                Cache-Control: no-cache\r\n\
                Connection: keep-alive\r\n\r\n";
    if writer.write_all(head.as_bytes()).is_err() || writer.flush().is_err() {
        return;
    }

    // Emit the current version immediately so a freshly-opened tab is in sync.
    let mut last = state.version();
    if writeln!(writer, "data: {last}\n").is_err() || writer.flush().is_err() {
        return;
    }

    loop {
        let version = state.wait_for_change(last, SSE_KEEPALIVE);
        let frame = if version != last {
            last = version;
            format!("data: {version}\n\n")
        } else {
            // Idle: a comment keeps the connection alive and surfaces a dead client.
            ": keep-alive\n\n".to_string()
        };
        if writer.write_all(frame.as_bytes()).is_err() || writer.flush().is_err() {
            break; // client went away
        }
    }
}

/// Extract a query-string parameter value from a raw request URL (e.g. `symbols` from
/// `/graph?symbols=1`). Minimal: handles the few flat params the viz uses.
fn query_param(raw_url: &str, key: &str) -> Option<String> {
    let query = raw_url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| percent_decode(v))
    })
}

/// Decode the handful of percent-escapes that appear in repo-relative paths (`%2F`, `%20`).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_query_params() {
        assert_eq!(
            query_param("/graph?symbols=1", "symbols").as_deref(),
            Some("1")
        );
        assert_eq!(query_param("/graph", "symbols"), None);
        assert_eq!(
            query_param("/subgraph?file=src%2Fmain.rs&depth=2", "file").as_deref(),
            Some("src/main.rs")
        );
        assert_eq!(
            query_param("/subgraph?file=a&depth=2", "depth").as_deref(),
            Some("2")
        );
    }

    #[test]
    fn binds_loopback_only() {
        let server = bind(None).expect("bind default or fallback");
        assert!(server.local_addr().ip().is_loopback());
    }
}
