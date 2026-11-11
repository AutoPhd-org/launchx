//! How `lx` talks to a running daemon: the run file `run/launchx.json` gives
//! its URL and bearer token, like for any other service (§10.2).

use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;

use crate::home::Home;

pub struct Daemon {
    url: String,
    token: String,
    http: Client<HttpConnector, Full<Bytes>>,
}

impl Daemon {
    pub fn find(home: &Home) -> Option<Daemon> {
        let run = home.read_run("launchx")?;
        Some(Daemon { url: run.url, token: run.token, http: Client::builder(TokioExecutor::new()).build_http() })
    }

    pub fn require(home: &Home) -> anyhow::Result<Daemon> {
        Daemon::find(home).ok_or_else(|| anyhow!("LaunchX is not running. Start it with `lx up` (or `lx daemon` in a terminal)."))
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>, timeout: Duration) -> anyhow::Result<Value> {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let request = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.url))
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(payload)))?;
        let response = tokio::time::timeout(timeout, self.http.request(request))
            .await
            .map_err(|_| anyhow!("LaunchX did not answer within {}s", timeout.as_secs()))?
            .with_context(|| format!("cannot reach LaunchX at {}", self.url))?;
        let status = response.status();
        let bytes = response.into_body().collect().await?.to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !status.is_success() {
            let message = value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| status.to_string());
            bail!("{message}");
        }
        Ok(value)
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.call(Method::GET, path, None, Duration::from_secs(10)).await
    }

    pub async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        // Stopping waits for services to exit, which can take their stop_timeout.
        self.call(Method::POST, path, Some(body), Duration::from_secs(180)).await
    }
}
