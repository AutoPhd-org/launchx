//! The gateway (contract §10.4) and the launcher's own API (§10.5).
//!
//! Hosts on the gateway port:
//! - `launchx.localhost` — the launcher's UI and API (browser session or bearer token)
//! - `<id>.localhost`    — service `<id>`, proxied with its bearer token
//! - `127.0.0.1`, `localhost` — the launcher API for programs (bearer token only)

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::header::{self, HeaderMap, HeaderName, HeaderValue};
use axum::http::{Method, StatusCode, Uri};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};

use crate::manifest;
use crate::supervisor::{Event, Launcher};
use crate::util;

/// Session cookie. `SameSite=Lax`, because WebKit counts each `<id>.localhost` as its own
/// site and the sign-in hop between them is a cross-site top-level navigation; unsafe
/// methods are protected by the Origin check instead, and GETs must not change state.
const COOKIE: &str = "lx_session";
const SESSION_DAYS: i64 = 30;
const CODE_LIFETIME: Duration = Duration::from_secs(120);

const INDEX: &str = include_str!("../ui/index.html");
const APP_JS: &str = include_str!("../ui/app.js");
const STYLE: &str = include_str!("../ui/style.css");
const FAVICON: &str = include_str!("../ui/favicon.svg");

pub struct App {
    launcher: Arc<Launcher>,
    codes: Mutex<HashMap<String, Code>>,
    proxy: Client<HttpConnector, Body>,
    shutdown: watch::Sender<bool>,
}

struct Code {
    host: String,
    next: String,
    expires: Instant,
}

enum Target {
    Launcher,
    Direct,
    Service(String),
    Unknown,
}

impl App {
    pub fn new(launcher: Arc<Launcher>, shutdown: watch::Sender<bool>) -> Arc<App> {
        Arc::new(App {
            launcher,
            codes: Mutex::new(HashMap::new()),
            proxy: Client::builder(TokioExecutor::new()).build_http(),
            shutdown,
        })
    }

    fn launcher_host(&self) -> String {
        self.launcher.config.gateway_host("launchx.localhost")
    }

    fn service_host(&self, id: &str) -> String {
        self.launcher.config.gateway_host(&format!("{id}.localhost"))
    }

    fn route(&self, host: &str) -> Target {
        let port = self.launcher.config.gateway_port;
        let (name, given) = match host.rsplit_once(':') {
            Some((name, digits)) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => (name, digits.parse::<u16>().ok()),
            _ => (host, None),
        };
        let port_ok = given == Some(port) || (given.is_none() && port == 80);
        if !port_ok {
            return Target::Unknown;
        }
        match name {
            "launchx.localhost" => Target::Launcher,
            "127.0.0.1" | "localhost" => Target::Direct,
            _ => match name.strip_suffix(".localhost") {
                Some(id) if manifest::valid_id(id) && self.launcher.has_service(id) => Target::Service(id.to_string()),
                _ => Target::Unknown,
            },
        }
    }

    // --- Sessions ---------------------------------------------------------

    fn session_value(&self, host: &str) -> String {
        let issued = util::unix_now();
        let nonce = util::token();
        let mac = util::sign(&self.launcher.secret(), &format!("session|{host}|{issued}|{nonce}"));
        format!("{issued}.{nonce}.{mac}")
    }

    fn session_ok(&self, headers: &HeaderMap, host: &str) -> bool {
        let Some(value) = cookie(headers, COOKIE) else { return false };
        let mut parts = value.splitn(3, '.');
        let (Some(issued), Some(nonce), Some(mac)) = (parts.next(), parts.next(), parts.next()) else { return false };
        let Ok(issued_at) = issued.parse::<i64>() else { return false };
        if util::unix_now() - issued_at > SESSION_DAYS * 86400 || issued_at > util::unix_now() + 60 {
            return false;
        }
        let expected = util::sign(&self.launcher.secret(), &format!("session|{host}|{issued}|{nonce}"));
        util::same(mac, &expected)
    }

    fn bearer_ok(&self, headers: &HeaderMap) -> bool {
        bearer(headers).is_some_and(|token| util::same(token, &self.launcher.token()))
    }

    /// A one-time sign-in link for `host` that lands on `next`.
    fn login_link(&self, host: &str, next: &str) -> String {
        let code = util::token();
        let mut codes = self.codes.lock().unwrap_or_else(|p| p.into_inner());
        codes.retain(|_, c| c.expires > Instant::now());
        codes.insert(code.clone(), Code { host: host.to_string(), next: next.to_string(), expires: Instant::now() + CODE_LIFETIME });
        format!("http://{host}/.launchx/login?code={code}")
    }

    fn login(&self, request: &Request, host: &str) -> Response {
        let query = query(request.uri());
        let code = query.get("code").cloned().unwrap_or_default();
        let found = self.codes.lock().unwrap_or_else(|p| p.into_inner()).remove(&code);
        match found {
            Some(entry) if entry.expires > Instant::now() && entry.host == host => {
                let cookie = format!("{COOKIE}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}", self.session_value(host), SESSION_DAYS * 86400);
                Response::builder()
                    .status(StatusCode::SEE_OTHER)
                    .header(header::LOCATION, safe_next(&entry.next))
                    .header(header::SET_COOKIE, cookie)
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::empty())
                    .unwrap()
            }
            _ => page(
                StatusCode::UNAUTHORIZED,
                "Sign-in link expired",
                "<p>This sign-in link has expired or was already used.</p><p>Run <code>lx open</code> in a terminal for a new one.</p>",
            ),
        }
    }

    /// On the launcher host: hand the launcher session to a service host.
    fn authorize(&self, request: &Request) -> Response {
        let query = query(request.uri());
        let id = query.get("service").cloned().unwrap_or_default();
        if !self.launcher.has_service(&id) {
            return page(StatusCode::NOT_FOUND, "Unknown service", &format!("<p>No service is registered as <code>{}</code>.</p>", util::html(&id)));
        }
        let next = safe_next(query.get("next").map(String::as_str).unwrap_or("/"));
        redirect(&self.login_link(&self.service_host(&id), &next))
    }

    // --- Launcher host and direct API ------------------------------------

    async fn on_launcher(self: Arc<Self>, request: Request, host: String) -> Response {
        let path = request.uri().path().to_string();
        if path == "/.launchx/login" {
            return self.login(&request, &host);
        }
        let bearer = self.bearer_ok(request.headers());
        let session = self.session_ok(request.headers(), &host);
        if !bearer && !session {
            return if wants_html(request.headers()) {
                page(
                    StatusCode::UNAUTHORIZED,
                    "Sign in to LaunchX",
                    "<p>This browser is not signed in to LaunchX.</p><p>Run <code>lx open</code> in a terminal; it opens a signed-in page.</p>",
                )
            } else {
                error(StatusCode::UNAUTHORIZED, "Unauthorized")
            };
        }
        if !bearer && unsafe_method(request.method()) && !origin_ok(request.headers(), &host) {
            return error(StatusCode::FORBIDDEN, "Cross-origin request refused");
        }
        if path == "/.launchx/authorize" {
            return self.authorize(&request);
        }
        self.api(request, bearer).await
    }

    async fn on_direct(self: Arc<Self>, request: Request) -> Response {
        if !self.bearer_ok(request.headers()) {
            if request.method() == Method::GET && request.uri().path() == "/" && wants_html(request.headers()) {
                return redirect(&self.launcher.config.launcher_url());
            }
            return error(StatusCode::UNAUTHORIZED, "Unauthorized");
        }
        self.api(request, true).await
    }

    async fn api(self: Arc<Self>, request: Request, bearer: bool) -> Response {
        let method = request.method().clone();
        let path = request.uri().path().to_string();
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let launcher = &self.launcher;
        match (&method, segments.as_slice()) {
            (&Method::GET, [""]) => asset(INDEX, "text/html; charset=utf-8"),
            (&Method::GET, ["app.js"]) => asset(APP_JS, "text/javascript; charset=utf-8"),
            (&Method::GET, ["style.css"]) => asset(STYLE, "text/css; charset=utf-8"),
            (&Method::GET, ["favicon.svg"]) => asset(FAVICON, "image/svg+xml"),
            (&Method::GET, ["api", "health"]) => json_response(StatusCode::OK, launcher.health()),
            (&Method::GET, ["api", "state"]) => json_response(StatusCode::OK, launcher.snapshot()),
            (&Method::GET, ["api", "events"]) => self.events(),
            (&Method::GET, ["api", "services", id, "logs"]) => {
                let lines = query(request.uri()).get("tail").and_then(|n| n.parse().ok()).unwrap_or(200usize).min(5000);
                match launcher.log_tail(id, lines) {
                    Ok(lines) => json_response(StatusCode::OK, json!({ "lines": lines })),
                    Err(message) => error(StatusCode::NOT_FOUND, &message),
                }
            }
            (&Method::GET, ["api", "agents"]) => json_response(StatusCode::OK, launcher.agents_index()),
            (&Method::POST, ["api", "services", id, "agents"]) => {
                let id = id.to_string();
                if !launcher.has_service(&id) {
                    return error(StatusCode::NOT_FOUND, &format!("Unknown service '{id}'"));
                }
                match read_json(request).await.get("discoverable").and_then(Value::as_bool) {
                    Some(on) => match launcher.set_discoverable(&id, on) {
                        Ok(reply) => json_response(StatusCode::OK, reply),
                        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, &message),
                    },
                    None => error(StatusCode::BAD_REQUEST, "Send {\"discoverable\": true} or {\"discoverable\": false}"),
                }
            }
            (&Method::POST, ["api", "services", id, action]) => {
                let id = id.to_string();
                match *action {
                    "start" => match launcher.start(&id) {
                        Ok(started) => json_response(StatusCode::OK, json!({ "started": started })),
                        Err(message) => error(StatusCode::CONFLICT, &message),
                    },
                    "stop" => match launcher.stop(&id).await {
                        Ok(stopped) => json_response(StatusCode::OK, json!({ "stopped": stopped })),
                        Err(message) => error(StatusCode::NOT_FOUND, &message),
                    },
                    "restart" => match launcher.restart(&id).await {
                        Ok(started) => json_response(StatusCode::OK, json!({ "started": started })),
                        Err(message) => error(StatusCode::CONFLICT, &message),
                    },
                    _ => error(StatusCode::NOT_FOUND, "Not found"),
                }
            }
            (&Method::POST, ["api", "link"]) => {
                let body = read_json(request).await;
                let next = body.get("next").and_then(Value::as_str).unwrap_or("/").to_string();
                let landing = match body.get("service").and_then(Value::as_str) {
                    Some(id) if launcher.has_service(id) => format!("/.launchx/authorize?service={id}&next={}", encode(&safe_next(&next))),
                    Some(id) => return error(StatusCode::NOT_FOUND, &format!("Unknown service '{id}'")),
                    None => safe_next(&next),
                };
                json_response(StatusCode::OK, json!({ "url": self.login_link(&self.launcher_host(), &landing) }))
            }
            (&Method::POST, ["api", "reload"]) => json_response(StatusCode::OK, launcher.reload().await),
            (&Method::POST, ["api", "shutdown"]) if bearer => {
                let _ = self.shutdown.send(true);
                json_response(StatusCode::ACCEPTED, json!({ "stopping": true }))
            }
            _ => error(StatusCode::NOT_FOUND, "Not found"),
        }
    }

    fn events(&self) -> Response {
        let receiver = self.launcher.events.subscribe();
        let mut shutdown = self.shutdown.subscribe();
        let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
            let event = match receiver.recv().await {
                Ok(Event::Change(id)) => SseEvent::default().event("change").data(json!({ "keys": ["services"], "id": id }).to_string()),
                Ok(Event::Log { id, line }) => SseEvent::default().event("log").data(json!({ "id": id, "line": line }).to_string()),
                Err(broadcast::error::RecvError::Lagged(_)) => SseEvent::default().event("change").data(r#"{"keys":["services"]}"#),
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            Some((Ok::<_, Infallible>(event), receiver))
        })
        .take_until(async move {
            while !*shutdown.borrow() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
        });
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response()
    }

    // --- Service hosts ----------------------------------------------------

    async fn on_service(self: Arc<Self>, request: Request, host: String, id: String) -> Response {
        let path = request.uri().path().to_string();
        if path == "/.launchx/login" {
            return self.login(&request, &host);
        }
        if !self.session_ok(request.headers(), &host) {
            if request.method() == Method::GET && wants_html(request.headers()) {
                let next = request.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
                return redirect(&format!(
                    "{}.launchx/authorize?service={id}&next={}",
                    self.launcher.config.launcher_url(),
                    encode(next)
                ));
            }
            return error(StatusCode::UNAUTHORIZED, "Unauthorized");
        }
        if unsafe_method(request.method()) && !origin_ok(request.headers(), &host) {
            return error(StatusCode::FORBIDDEN, "Cross-origin request refused");
        }
        match (request.method().clone(), path.as_str()) {
            (Method::POST, "/.launchx/start") => {
                return match self.launcher.start(&id) {
                    Ok(started) => json_response(StatusCode::OK, json!({ "started": started })),
                    Err(message) => error(StatusCode::CONFLICT, &message),
                };
            }
            (Method::GET, "/.launchx/status") => {
                return json_response(StatusCode::OK, json!(self.launcher.runtime(&id)));
            }
            _ => {}
        }
        match self.launcher.upstream(&id) {
            Some((port, token)) => self.proxy(request, port, &token, &host, &id).await,
            None => self.not_running(&request, &id),
        }
    }

    fn not_running(&self, request: &Request, id: &str) -> Response {
        let runtime = self.launcher.runtime(id).unwrap_or_default();
        if !wants_html(request.headers()) {
            return json_response(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": format!("{id} is not running"), "status": runtime.status }));
        }
        let name = util::html(&self.launcher.name(id).unwrap_or_else(|| id.to_string()));
        let status = serde_json::to_value(runtime.status).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let problem = runtime.error.as_deref().map(|e| format!("<p class=\"problem\">{}</p>", util::html(e))).unwrap_or_default();
        let body = format!(
            r#"<p><strong>{name}</strong> is <span id="status">{status}</span>.</p>{problem}
<p><button id="start">Start {name}</button> <a href="{launcher}">Open LaunchX</a></p>
<script>
const button = document.getElementById("start");
async function wait() {{
  for (let i = 0; i < 240; i++) {{
    const state = await (await fetch("/.launchx/status")).json();
    document.getElementById("status").textContent = state.status;
    if (state.status === "running") {{ location.reload(); return; }}
    if (state.status === "failed" || state.status === "stopped") {{ button.disabled = false; return; }}
    await new Promise(r => setTimeout(r, 500));
  }}
}}
button.addEventListener("click", async () => {{
  button.disabled = true;
  const reply = await fetch("/.launchx/start", {{method: "POST"}});
  if (!reply.ok) {{ alert((await reply.json()).error); button.disabled = false; return; }}
  wait();
}});
if (["starting", "waiting"].includes("{status}")) {{ button.disabled = true; wait(); }}
</script>"#,
            launcher = self.launcher.config.launcher_url()
        );
        page(StatusCode::SERVICE_UNAVAILABLE, &format!("{name} is {status}"), &body)
    }

    async fn proxy(&self, mut request: Request, port: u16, token: &str, host: &str, id: &str) -> Response {
        let upgrade = is_upgrade(request.headers());
        let target = request.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_else(|| "/".into());
        prepare_headers(request.headers_mut(), port, token, host, upgrade);
        if upgrade {
            return proxy_upgrade(request, port, &target).await;
        }
        *request.uri_mut() = match format!("http://127.0.0.1:{port}{target}").parse::<Uri>() {
            Ok(uri) => uri,
            Err(_) => return error(StatusCode::BAD_REQUEST, "Bad request target"),
        };
        match self.proxy.request(request).await {
            Ok(response) => {
                let mut response = response.map(Body::new);
                strip_hop_headers(response.headers_mut(), false);
                response
            }
            Err(err) => page(
                StatusCode::BAD_GATEWAY,
                "Service not answering",
                &format!("<p><strong>{}</strong> did not answer: {}</p>", util::html(id), util::html(&err.to_string())),
            ),
        }
    }
}

/// Forward an Upgrade request (WebSocket and the like) and splice the two connections.
async fn proxy_upgrade(mut request: Request, port: u16, target: &str) -> Response {
    let client_side = hyper::upgrade::on(&mut request);
    *request.uri_mut() = target.parse().unwrap_or_else(|_| Uri::from_static("/"));
    let stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(stream) => stream,
        Err(err) => return error(StatusCode::BAD_GATEWAY, &format!("Service not answering: {err}")),
    };
    let (mut sender, connection) = match hyper::client::conn::http1::handshake::<_, Body>(TokioIo::new(stream)).await {
        Ok(pair) => pair,
        Err(err) => return error(StatusCode::BAD_GATEWAY, &format!("Service not answering: {err}")),
    };
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let mut response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(err) => return error(StatusCode::BAD_GATEWAY, &format!("Service not answering: {err}")),
    };
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        let mut response = response.map(Body::new);
        strip_hop_headers(response.headers_mut(), false);
        return response;
    }
    let service_side = hyper::upgrade::on(&mut response);
    tokio::spawn(async move {
        if let (Ok(client), Ok(service)) = tokio::join!(client_side, service_side) {
            let mut client = TokioIo::new(client);
            let mut service = TokioIo::new(service);
            let _ = tokio::io::copy_bidirectional(&mut client, &mut service).await;
        }
    });
    let mut reply = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for (name, value) in response.headers() {
        reply = reply.header(name, value);
    }
    reply.body(Body::empty()).unwrap()
}

const HOP_HEADERS: [&str; 8] = ["connection", "keep-alive", "proxy-connection", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding"];

fn strip_hop_headers(headers: &mut HeaderMap, keep_upgrade: bool) {
    let listed: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(',').map(|s| s.trim().to_ascii_lowercase()))
        .filter(|name| !name.is_empty() && name != "upgrade")
        .collect();
    for name in HOP_HEADERS.iter().map(|s| s.to_string()).chain(listed) {
        headers.remove(name.as_str());
    }
    if keep_upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    } else {
        headers.remove(header::UPGRADE);
    }
}

/// Swap the browser's credentials for the service's token (§10.4).
fn prepare_headers(headers: &mut HeaderMap, port: u16, token: &str, host: &str, upgrade: bool) {
    strip_hop_headers(headers, upgrade);
    headers.remove(header::AUTHORIZATION);
    let remaining: Vec<String> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';').map(|s| s.trim().to_string()))
        .filter(|pair| !pair.is_empty() && pair.split('=').next() != Some(COOKIE))
        .collect();
    headers.remove(header::COOKIE);
    if !remaining.is_empty() {
        if let Ok(value) = HeaderValue::from_str(&remaining.join("; ")) {
            headers.insert(header::COOKIE, value);
        }
    }
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        headers.insert(header::AUTHORIZATION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&format!("127.0.0.1:{port}")) {
        headers.insert(header::HOST, value);
    }
    if let Ok(value) = HeaderValue::from_str(host) {
        headers.insert(HeaderName::from_static("x-forwarded-host"), value);
    }
    headers.insert(HeaderName::from_static("x-forwarded-proto"), HeaderValue::from_static("http"));
    headers.insert(HeaderName::from_static("x-forwarded-for"), HeaderValue::from_static("127.0.0.1"));
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
        && headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.split(',').any(|s| s.trim().eq_ignore_ascii_case("upgrade")))
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ")
}

fn unsafe_method(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn origin_ok(headers: &HeaderMap, host: &str) -> bool {
    headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(&format!("http://{host}"))
}

fn wants_html(headers: &HeaderMap) -> bool {
    headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("text/html"))
}

fn query(uri: &Uri) -> HashMap<String, String> {
    Query::<HashMap<String, String>>::try_from_uri(uri).map(|q| q.0).unwrap_or_default()
}

/// Only same-origin paths may follow a sign-in.
fn safe_next(next: &str) -> String {
    if next.starts_with('/') && !next.starts_with("//") && !next.contains('\\') { next.to_string() } else { "/".to_string() }
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn read_json(request: Request) -> Value {
    let Ok(body) = http_body_util::Limited::new(request.into_body(), 64 * 1024).collect().await else { return json!({}) };
    serde_json::from_slice(&body.to_bytes()).unwrap_or_else(|_| json!({}))
}

fn json_response(status: StatusCode, value: Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(value.to_string()))
        .unwrap()
}

fn error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({ "error": message }))
}

fn redirect(location: &str) -> Response {
    Response::builder().status(StatusCode::FOUND).header(header::LOCATION, location).header(header::CACHE_CONTROL, "no-store").body(Body::empty()).unwrap()
}

fn asset(content: &'static str, kind: &'static str) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, kind)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'",
        )
        .body(Body::from(content))
        .unwrap()
}

/// A small HTML page from the gateway itself.
fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark"><title>{title} · LaunchX</title>
<style>
:root{{--bg:#f6f7f9;--card:#fff;--text:#16181d;--muted:#5b6270;--line:#e3e6eb;--accent:#3b5bdb}}
@media (prefers-color-scheme:dark){{:root{{--bg:#101114;--card:#181a1f;--text:#e8eaee;--muted:#9aa1ad;--line:#2a2e36;--accent:#7c95f5}}}}
body{{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--text);font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;padding:16px;box-sizing:border-box}}
main{{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:28px 32px;max-width:480px;width:100%;box-sizing:border-box}}
h1{{font-size:18px;margin:0 0 12px}} .brand{{font-size:12px;letter-spacing:.08em;color:var(--muted);margin-bottom:6px}}
code{{background:var(--bg);border:1px solid var(--line);border-radius:5px;padding:1px 6px;font-size:13px}}
button{{font:inherit;background:var(--accent);color:#fff;border:0;border-radius:8px;padding:7px 14px;cursor:pointer}} button:disabled{{opacity:.5}}
a{{color:var(--accent);margin-left:10px}} .problem{{color:#c92a2a;font-size:14px}}
</style></head><body><main><div class="brand">LAUNCHX</div><h1>{title}</h1>{body}</main></body></html>"#
    );
    Response::builder().status(status).header(header::CONTENT_TYPE, "text/html; charset=utf-8").header(header::CACHE_CONTROL, "no-store").body(Body::from(html)).unwrap()
}

async fn dispatch(State(app): State<Arc<App>>, request: Request) -> Response {
    let host = request.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    match app.route(&host) {
        Target::Launcher => app.clone().on_launcher(request, host).await,
        Target::Direct => app.clone().on_direct(request).await,
        Target::Service(id) => app.clone().on_service(request, host, id).await,
        Target::Unknown => error(StatusCode::MISDIRECTED_REQUEST, "Unknown host. Use launchx.localhost or <service>.localhost on this port."),
    }
}

/// Serve until `shutdown` turns true; then give open requests a moment.
pub async fn serve(app: Arc<App>, listener: TcpListener) -> anyhow::Result<()> {
    let mut shutdown = app.shutdown.subscribe();
    let router = Router::new().fallback(dispatch).with_state(app);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            while !*shutdown.borrow() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_paths_stay_on_the_same_origin() {
        assert_eq!(safe_next("/a?b=1"), "/a?b=1");
        for bad in ["//evil.example", "https://evil.example", "\\\\evil", "/\\evil", ""] {
            assert_eq!(safe_next(bad), "/", "{bad}");
        }
        assert_eq!(encode("/a b?c=d&e"), "/a%20b%3Fc%3Dd%26e");
    }

    #[test]
    fn credentials_are_swapped_and_hop_headers_dropped() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer browser"));
        headers.insert(header::COOKIE, HeaderValue::from_static("a=1; lx_session=secret; b=2"));
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive, x-private"));
        headers.insert(HeaderName::from_static("x-private"), HeaderValue::from_static("1"));
        headers.insert(header::TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        prepare_headers(&mut headers, 5000, "svc-token", "vmc.localhost:7600", false);
        assert_eq!(headers[header::AUTHORIZATION], "Bearer svc-token");
        assert_eq!(headers[header::COOKIE], "a=1; b=2");
        assert_eq!(headers[header::HOST], "127.0.0.1:5000");
        assert_eq!(headers["x-forwarded-host"], "vmc.localhost:7600");
        assert!(!headers.contains_key("x-private") && !headers.contains_key(header::CONNECTION) && !headers.contains_key(header::TRANSFER_ENCODING));
    }
}
