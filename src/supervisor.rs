//! Starts, watches and stops services (contract §5, §9, §10.3).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::{Request, StatusCode};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, watch};

use crate::home::{Assigned, Config, Home, Persisted, RunFile};
use crate::manifest::{self, HttpDef, Restart, Service, ServiceDef};
use crate::{platform, util};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const LOG_LIMIT: u64 = 5 * 1024 * 1024;
const MAX_FAILURES: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Waiting,
    Starting,
    Running,
    Stopping,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Ok,
    Degraded,
    Unhealthy,
}

#[derive(Clone, Debug, Serialize)]
pub struct Runtime {
    pub status: Status,
    pub health: Option<Health>,
    pub summary: Option<String>,
    pub detail: Option<String>,
    pub version: Option<String>,
    pub pid: Option<u32>,
    pub since: Option<String>,
    pub restarts: u32,
    pub last_exit: Option<String>,
    pub error: Option<String>,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime {
            status: Status::Stopped,
            health: None,
            summary: None,
            detail: None,
            version: None,
            pid: None,
            since: None,
            restarts: 0,
            last_exit: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Change(String),
    Log { id: String, line: String },
}

struct Control {
    stop: Arc<watch::Sender<bool>>,
    done: watch::Receiver<bool>,
}

struct Entry {
    service: Service,
    rt: Runtime,
    control: Option<Control>,
    log: Arc<Mutex<LogFile>>,
}

struct Inner {
    entries: BTreeMap<String, Entry>,
    errors: Vec<String>,
    persisted: Persisted,
}

pub type HttpClient = Client<HttpConnector, Empty<Bytes>>;

pub struct Launcher {
    pub home: Home,
    pub config: Config,
    pub started_at: String,
    pub events: broadcast::Sender<Event>,
    inner: Mutex<Inner>,
    http: HttpClient,
    env: Vec<(String, String)>,
}

/// Everything needed to spawn one service.
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub port: Option<u16>,
    pub token: String,
}

enum Wait {
    Ready,
    Stopped,
    Failed(String),
}

enum Outcome {
    Stopped(String),
    SpawnFailed(String),
    Exited { success: bool, description: String, ran: Duration },
}

pub enum Probe {
    Healthy { health: Health, summary: Option<String>, version: Option<String>, detail: Option<String> },
    Unhealthy(String),
}

pub fn http_client() -> HttpClient {
    Client::builder(TokioExecutor::new()).build_http()
}

impl Launcher {
    pub fn new(home: Home, config: Config) -> anyhow::Result<Arc<Launcher>> {
        let persisted = home.load_state()?;
        home.save_state(&persisted)?;
        let (services, errors) = manifest::load_registry(&home);
        let mut entries = BTreeMap::new();
        for service in services {
            let log = Arc::new(Mutex::new(LogFile::new(home.log_path(&service.id))));
            entries.insert(service.id.clone(), Entry { service, rt: Runtime::default(), control: None, log });
        }
        let (events, _) = broadcast::channel(1024);
        Ok(Arc::new(Launcher {
            home,
            config,
            started_at: util::now_rfc3339(),
            events,
            inner: Mutex::new(Inner { entries, errors, persisted }),
            http: http_client(),
            env: platform::environment(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn token(&self) -> String {
        self.lock().persisted.launcher_token.clone()
    }

    pub fn secret(&self) -> String {
        self.lock().persisted.secret.clone()
    }

    pub fn has_service(&self, id: &str) -> bool {
        self.lock().entries.contains_key(id)
    }

    pub fn name(&self, id: &str) -> Option<String> {
        self.lock().entries.get(id).map(|e| e.service.def.name.clone())
    }

    /// Port and token of a service that is up, for the gateway.
    pub fn upstream(&self, id: &str) -> Option<(u16, String)> {
        let inner = self.lock();
        let entry = inner.entries.get(id)?;
        if entry.rt.status != Status::Running {
            return None;
        }
        let assigned = inner.persisted.services.get(id)?;
        Some((assigned.port, assigned.token.clone()))
    }

    pub fn runtime(&self, id: &str) -> Option<Runtime> {
        self.lock().entries.get(id).map(|e| e.rt.clone())
    }

    fn defs(inner: &Inner) -> BTreeMap<String, ServiceDef> {
        inner.entries.iter().map(|(id, e)| (id.clone(), e.service.def.clone())).collect()
    }

    fn changed(&self, id: &str) {
        let _ = self.events.send(Event::Change(id.to_string()));
    }

    fn update(&self, id: &str, change: impl FnOnce(&mut Runtime)) {
        {
            let mut inner = self.lock();
            if let Some(entry) = inner.entries.get_mut(id) {
                change(&mut entry.rt);
            }
        }
        self.changed(id);
    }

    fn log(&self, id: &str, line: &str) {
        let log = match self.lock().entries.get(id) {
            Some(entry) => entry.log.clone(),
            None => return,
        };
        let line = log.lock().unwrap_or_else(|p| p.into_inner()).write(line);
        let _ = self.events.send(Event::Log { id: id.to_string(), line });
    }

    // --- Public operations ----------------------------------------------

    /// Start `id` and anything it requires, and remember that the user wants it
    /// running. Returns the services this started.
    pub fn start(self: &Arc<Self>, id: &str) -> Result<Vec<String>, String> {
        let started = self.launch_ordered(id)?;
        self.remember(&[id.to_string()], true);
        Ok(started)
    }

    fn remember(&self, ids: &[String], running: bool) {
        let mut inner = self.lock();
        let before = inner.persisted.running.clone();
        for id in ids {
            if running {
                inner.persisted.running.insert(id.clone());
            } else {
                inner.persisted.running.remove(id);
            }
        }
        if inner.persisted.running != before {
            let _ = self.home.save_state(&inner.persisted);
        }
    }

    fn launch_ordered(self: &Arc<Self>, id: &str) -> Result<Vec<String>, String> {
        let order = {
            let inner = self.lock();
            let entry = inner.entries.get(id).ok_or_else(|| format!("Unknown service '{id}'"))?;
            if let Some(error) = &entry.service.error {
                return Err(format!("{id} cannot start: {error}"));
            }
            manifest::with_dependencies(id, &Self::defs(&inner))
        };
        let mut started = Vec::new();
        for service in order {
            if self.launch_task(&service)? {
                started.push(service);
            }
        }
        Ok(started)
    }

    fn launch_task(self: &Arc<Self>, id: &str) -> Result<bool, String> {
        let (stop_rx, done_tx) = {
            let mut inner = self.lock();
            let entry = inner.entries.get_mut(id).ok_or_else(|| format!("Unknown service '{id}'"))?;
            if let Some(error) = &entry.service.error {
                return Err(format!("{id} cannot start: {error}"));
            }
            if entry.control.is_some() {
                return Ok(false);
            }
            let (stop_tx, stop_rx) = watch::channel(false);
            let (done_tx, done_rx) = watch::channel(false);
            entry.control = Some(Control { stop: Arc::new(stop_tx), done: done_rx });
            entry.rt = Runtime { status: Status::Starting, last_exit: entry.rt.last_exit.take(), ..Runtime::default() };
            (stop_rx, done_tx)
        };
        self.changed(id);
        tokio::spawn(self.clone().lifecycle(id.to_string(), stop_rx, done_tx));
        Ok(true)
    }

    /// Stop `id`, after stopping the running services that require it.
    pub async fn stop(self: &Arc<Self>, id: &str) -> Result<Vec<String>, String> {
        let order = {
            let inner = self.lock();
            if !inner.entries.contains_key(id) {
                return Err(format!("Unknown service '{id}'"));
            }
            let mut order: Vec<String> = manifest::dependents(id, &Self::defs(&inner))
                .into_iter()
                .filter(|d| inner.entries.get(d).is_some_and(|e| e.control.is_some()))
                .collect();
            order.push(id.to_string());
            order
        };
        self.remember(&order, false);
        let mut stopped = Vec::new();
        for service in order {
            if self.stop_one(&service).await {
                stopped.push(service);
            }
        }
        Ok(stopped)
    }

    async fn stop_one(&self, id: &str) -> bool {
        let (stop, mut done, timeout) = {
            let inner = self.lock();
            match inner.entries.get(id) {
                Some(Entry { control: Some(control), service, .. }) => {
                    (control.stop.clone(), control.done.clone(), service.def.stop_timeout)
                }
                _ => return false,
            }
        };
        let _ = stop.send(true);
        let finished = async {
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(timeout + 10), finished).await;
        true
    }

    /// Restart one service; services that require it keep running (§9).
    pub async fn restart(self: &Arc<Self>, id: &str) -> Result<Vec<String>, String> {
        if !self.has_service(id) {
            return Err(format!("Unknown service '{id}'"));
        }
        self.stop_one(id).await;
        self.launch_ordered(id)
    }

    /// Start services marked `autostart`, and those the user left running last time.
    pub fn autostart(self: &Arc<Self>) {
        let ids: Vec<String> = {
            let inner = self.lock();
            inner
                .entries
                .values()
                .filter(|e| e.service.def.autostart || inner.persisted.running.contains(&e.service.id))
                .map(|e| e.service.id.clone())
                .collect()
        };
        for id in ids {
            if let Err(error) = self.launch_ordered(&id) {
                eprintln!("autostart {id}: {error}");
            }
        }
    }

    /// Stop everything, most dependent first.
    pub async fn shutdown(self: &Arc<Self>) {
        loop {
            let next = {
                let inner = self.lock();
                let active: Vec<&String> = inner.entries.iter().filter(|(_, e)| e.control.is_some()).map(|(id, _)| id).collect();
                active
                    .iter()
                    .find(|id| !active.iter().any(|other| inner.entries[*other].service.def.requires.contains(id)))
                    .or(active.first())
                    .map(|id| id.to_string())
            };
            match next {
                Some(id) => {
                    self.stop_one(&id).await;
                }
                None => break,
            }
        }
    }

    /// Re-read the registered manifests. Removed services are stopped;
    /// changed definitions apply from their next start.
    pub async fn reload(self: &Arc<Self>) -> Value {
        let (services, errors) = manifest::load_registry(&self.home);
        let ids: BTreeSet<String> = services.iter().map(|s| s.id.clone()).collect();
        let removed: Vec<String> = self.lock().entries.keys().filter(|id| !ids.contains(*id)).cloned().collect();
        for id in &removed {
            let _ = self.stop(id).await;
        }
        let mut added = Vec::new();
        {
            let mut inner = self.lock();
            for id in &removed {
                inner.entries.remove(id);
                inner.persisted.services.remove(id);
                inner.persisted.running.remove(id);
            }
            for service in services {
                match inner.entries.get_mut(&service.id) {
                    Some(entry) => entry.service = service,
                    None => {
                        added.push(service.id.clone());
                        let log = Arc::new(Mutex::new(LogFile::new(self.home.log_path(&service.id))));
                        inner.entries.insert(service.id.clone(), Entry { service, rt: Runtime::default(), control: None, log });
                    }
                }
            }
            inner.errors = errors.clone();
            let _ = self.home.save_state(&inner.persisted);
        }
        self.changed("*");
        json!({ "added": added, "removed": removed, "errors": errors })
    }

    /// Stop copies of services that a previous launcher left running.
    pub async fn cleanup_orphans(&self) {
        let Ok(entries) = fs::read_dir(self.home.run_dir()) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            if id == "launchx" {
                continue;
            }
            let run: Option<RunFile> = fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok());
            if let Some(run) = run {
                // Our children lead their own process group; anything else is not ours.
                if util::pid_alive(run.pid) && util::process_group(run.pid) == Some(run.pid) {
                    self.log(&id, &format!("[launchx] stopping pid {} left running by a previous launcher", run.pid));
                    util::signal_group(run.pid, libc::SIGTERM);
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while util::pid_alive(run.pid) && Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    util::signal_group(run.pid, libc::SIGKILL);
                }
            }
            let _ = fs::remove_file(&path);
        }
    }

    pub fn log_tail(&self, id: &str, lines: usize) -> Result<Vec<String>, String> {
        if !self.has_service(id) {
            return Err(format!("Unknown service '{id}'"));
        }
        Ok(tail(&self.home.log_path(id), lines))
    }

    // --- State documents ------------------------------------------------

    pub fn health(&self) -> Value {
        let inner = self.lock();
        let total = inner.entries.len();
        let running = inner.entries.values().filter(|e| e.rt.status == Status::Running).count();
        let failed = inner.entries.values().filter(|e| e.rt.status == Status::Failed).count();
        let mut summary = format!("{running} of {total} services running");
        if failed > 0 {
            summary.push_str(&format!(", {failed} failed"));
        }
        json!({ "id": "launchx", "version": VERSION, "status": "ok", "summary": summary })
    }

    pub fn snapshot(&self) -> Value {
        let inner = self.lock();
        let defs = Self::defs(&inner);
        let services: Vec<Value> = inner
            .entries
            .values()
            .map(|entry| {
                let def = &entry.service.def;
                let id = &entry.service.id;
                let http = def.http.as_ref();
                let ui = http.and_then(|h| h.ui.as_ref()).map(|path| format!("{}{}", self.config.service_url(id), path.trim_start_matches('/')));
                let required_by: Vec<&String> = defs.iter().filter(|(_, d)| d.requires.contains(id)).map(|(other, _)| other).collect();
                json!({
                    "id": id,
                    "name": def.name,
                    "description": def.description,
                    "level": def.level,
                    "status": entry.rt.status,
                    "health": entry.rt.health,
                    "summary": entry.rt.summary,
                    "detail": entry.rt.detail,
                    "version": entry.rt.version,
                    "pid": entry.rt.pid,
                    "port": http.and(inner.persisted.services.get(id).map(|a| a.port)),
                    "ui": ui,
                    "requires": def.requires,
                    "required_by": required_by,
                    "since": entry.rt.since,
                    "restarts": entry.rt.restarts,
                    "last_exit": entry.rt.last_exit,
                    "error": entry.rt.error.clone().or_else(|| entry.service.error.clone()),
                    "autostart": def.autostart,
                    "cli": def.cli,
                    "homepage": def.homepage,
                    "manifest": entry.service.manifest,
                })
            })
            .collect();
        json!({
            "launcher": {
                "version": VERSION,
                "pid": std::process::id(),
                "gateway": self.config.launcher_url(),
                "started_at": self.started_at,
                "home": self.home.root,
            },
            "errors": inner.errors,
            "services": services,
        })
    }

    // --- One service's life ---------------------------------------------

    async fn lifecycle(self: Arc<Self>, id: String, mut stop: watch::Receiver<bool>, done: watch::Sender<bool>) {
        let unit = Duration::from_millis(self.config.backoff_unit_ms.max(1));
        let mut failures: u32 = 0;
        loop {
            match self.wait_for_dependencies(&id, &mut stop).await {
                Wait::Ready => {}
                Wait::Stopped => {
                    self.update(&id, |rt| rt.status = Status::Stopped);
                    break;
                }
                Wait::Failed(message) => {
                    self.fail(&id, message);
                    break;
                }
            }
            let launch = match self.prepare(&id) {
                Ok(launch) => launch,
                Err(message) => {
                    self.fail(&id, message);
                    break;
                }
            };
            match self.run_once(&id, launch, &mut stop).await {
                Outcome::Stopped(description) => {
                    self.update(&id, |rt| {
                        *rt = Runtime { restarts: rt.restarts, last_exit: Some(description), ..Runtime::default() };
                    });
                    break;
                }
                Outcome::SpawnFailed(message) => {
                    self.fail(&id, message);
                    break;
                }
                Outcome::Exited { success, description, ran } => {
                    let policy = self.lock().entries.get(&id).map(|e| e.service.def.restart).unwrap_or_default();
                    if ran >= Duration::from_secs(60) {
                        failures = 0;
                    }
                    if !success {
                        failures += 1;
                    }
                    let again = match policy {
                        Restart::No => false,
                        Restart::OnFailure => !success,
                        Restart::Always => true,
                    };
                    if !again {
                        self.update(&id, |rt| {
                            *rt = Runtime {
                                status: if success { Status::Stopped } else { Status::Failed },
                                restarts: rt.restarts,
                                error: (!success).then(|| description.clone()),
                                last_exit: Some(description),
                                ..Runtime::default()
                            };
                        });
                        break;
                    }
                    if failures >= MAX_FAILURES {
                        self.fail(&id, format!("Gave up after {MAX_FAILURES} failed starts in a row; last: {description}"));
                        break;
                    }
                    let delay = if success { unit } else { (unit * 2u32.pow((failures - 1).min(6))).min(unit * 60) };
                    self.log(&id, &format!("[launchx] restarting in {:.1}s", delay.as_secs_f64()));
                    self.update(&id, |rt| {
                        *rt = Runtime {
                            status: Status::Starting,
                            restarts: rt.restarts + 1,
                            detail: Some(format!("Restarting in {:.0}s", delay.as_secs_f64().ceil())),
                            last_exit: Some(description),
                            ..Runtime::default()
                        };
                    });
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = stop.changed() => {
                            self.update(&id, |rt| { rt.status = Status::Stopped; rt.detail = None; });
                            break;
                        }
                    }
                }
            }
        }
        if let Some(entry) = self.lock().entries.get_mut(&id) {
            entry.control = None;
        }
        let _ = done.send(true);
        self.changed(&id);
    }

    fn fail(&self, id: &str, message: String) {
        self.log(id, &format!("[launchx] {message}"));
        self.update(id, |rt| {
            *rt = Runtime { status: Status::Failed, restarts: rt.restarts, last_exit: rt.last_exit.take(), error: Some(message), ..Runtime::default() };
        });
    }

    async fn wait_for_dependencies(&self, id: &str, stop: &mut watch::Receiver<bool>) -> Wait {
        let mut announced = false;
        loop {
            if *stop.borrow() {
                return Wait::Stopped;
            }
            let pending = {
                let inner = self.lock();
                let Some(entry) = inner.entries.get(id) else { return Wait::Failed("removed".into()) };
                let mut pending = Vec::new();
                for dep in &entry.service.def.requires {
                    let Some(other) = inner.entries.get(dep) else {
                        return Wait::Failed(format!("requires unknown service '{dep}'"));
                    };
                    match other.rt.status {
                        Status::Running if other.rt.health != Some(Health::Unhealthy) => {}
                        Status::Stopped | Status::Failed if other.control.is_none() => {
                            let state = if other.rt.status == Status::Failed { "failed" } else { "stopped" };
                            return Wait::Failed(format!("requires '{dep}', which {state}"));
                        }
                        _ => pending.push(dep.clone()),
                    }
                }
                pending
            };
            if pending.is_empty() {
                return Wait::Ready;
            }
            if !announced {
                announced = true;
                self.update(id, |rt| {
                    rt.status = Status::Waiting;
                    rt.detail = Some(format!("Waiting for {}", pending.join(", ")));
                });
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                _ = stop.changed() => return Wait::Stopped,
            }
        }
    }

    /// Choose port and token, and build the command and environment.
    fn prepare(&self, id: &str) -> Result<Launch, String> {
        let mut inner = self.lock();
        let service = inner.entries.get(id).ok_or("removed")?.service.clone();
        let mut assigned = inner.persisted.services.get(id).cloned();
        let mut dirty = false;
        if assigned.is_none() {
            assigned = Some(Assigned { port: 0, token: util::token() });
            dirty = true;
        }
        let mut assigned = assigned.unwrap();
        let mut port = None;
        if let Some(http) = &service.def.http {
            let wanted = if assigned.port != 0 { assigned.port } else { http.port.unwrap_or(0) };
            let chosen = if wanted != 0 && port_free(wanted) { wanted } else { free_port().map_err(|e| format!("no free port: {e}"))? };
            if chosen != assigned.port {
                assigned.port = chosen;
                dirty = true;
            }
            port = Some(chosen);
        }
        if dirty {
            inner.persisted.services.insert(id.to_string(), assigned.clone());
            if let Err(error) = self.home.save_state(&inner.persisted) {
                return Err(format!("could not save {}: {error:#}", self.home.state_path().display()));
            }
        }
        let deps: Vec<(String, String, String)> = service
            .def
            .requires
            .iter()
            .filter_map(|dep| {
                let other = inner.entries.get(dep)?;
                other.service.def.http.as_ref()?;
                let a = inner.persisted.services.get(dep)?;
                Some((dep.clone(), format!("http://127.0.0.1:{}", a.port), a.token.clone()))
            })
            .collect();
        drop(inner);
        build_launch(&service, port, &assigned.token, &self.env, &deps, &self.config)
    }

    async fn run_once(self: &Arc<Self>, id: &str, launch: Launch, stop: &mut watch::Receiver<bool>) -> Outcome {
        let (def, level) = match self.lock().entries.get(id) {
            Some(entry) => (entry.service.def.clone(), entry.service.def.level),
            None => return Outcome::SpawnFailed("removed".into()),
        };
        let mut child = match spawn(&launch) {
            Ok(child) => child,
            Err(message) => return Outcome::SpawnFailed(message),
        };
        let pid = child.id().unwrap_or(0);
        if let Some(out) = child.stdout.take() {
            tokio::spawn(pump(self.clone(), id.to_string(), out));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(pump(self.clone(), id.to_string(), err));
        }
        let where_ = launch.port.map(|p| format!(" on port {p}")).unwrap_or_default();
        self.log(id, &format!("[launchx] started {} (pid {pid}){where_}", launch.program.display()));
        self.update(id, |rt| {
            rt.status = Status::Starting;
            rt.pid = Some(pid);
            rt.detail = None;
            rt.error = None;
        });
        let begun = Instant::now();
        let deadline = begun + Duration::from_secs(def.start_timeout);
        let stop_timeout = Duration::from_secs(def.stop_timeout);
        let mut ready = false;
        let mut misses = 0u32;
        loop {
            let period = if ready { Duration::from_secs(5) } else { Duration::from_millis(250) };
            tokio::select! {
                status = child.wait() => {
                    util::signal_group(pid, libc::SIGKILL);
                    self.home.remove_run(id);
                    let mut description = status.map(describe).unwrap_or_else(|e| format!("could not be waited for: {e}"));
                    let success = ready && description == "exited with code 0";
                    if !ready {
                        description.push_str(" before it was ready");
                    }
                    self.log(id, &format!("[launchx] {description}"));
                    return Outcome::Exited { success, description, ran: begun.elapsed() };
                }
                _ = stop.changed() => {
                    self.update(id, |rt| { rt.status = Status::Stopping; rt.detail = None; });
                    self.log(id, "[launchx] stopping (SIGTERM)");
                    let description = terminate(&mut child, pid, stop_timeout).await;
                    self.home.remove_run(id);
                    self.log(id, &format!("[launchx] {description}"));
                    return Outcome::Stopped(description);
                }
                _ = tokio::time::sleep(period) => {
                    let probe = match (&def.http, launch.port) {
                        (Some(http), Some(port)) => self.probe(id, port, &launch.token, http, level).await,
                        _ if begun.elapsed() >= Duration::from_secs(1) => Probe::Healthy { health: Health::Ok, summary: None, version: None, detail: None },
                        _ => Probe::Unhealthy("starting".into()),
                    };
                    match probe {
                        Probe::Healthy { health, summary, version, detail } => {
                            misses = 0;
                            if !ready {
                                ready = true;
                                self.log(id, &format!("[launchx] ready after {:.1}s", begun.elapsed().as_secs_f64()));
                                if let Some(port) = launch.port {
                                    let ui = def.http.as_ref().and_then(|h| h.ui.as_ref())
                                        .map(|path| format!("{}{}", self.config.service_url(id), path.trim_start_matches('/')));
                                    let run = RunFile {
                                        id: id.to_string(), pid, url: format!("http://127.0.0.1:{port}"), token: launch.token.clone(),
                                        ui, version: version.clone(), started_at: util::now_rfc3339(),
                                    };
                                    if let Err(error) = self.home.write_run(&run) {
                                        self.log(id, &format!("[launchx] could not write run file: {error:#}"));
                                    }
                                }
                            }
                            self.update(id, |rt| {
                                if rt.status != Status::Running {
                                    rt.since = Some(util::now_rfc3339());
                                }
                                rt.status = Status::Running;
                                rt.health = Some(health);
                                rt.summary = summary;
                                rt.version = version.or(rt.version.take());
                                rt.detail = detail;
                            });
                        }
                        Probe::Unhealthy(reason) => {
                            if !ready {
                                if Instant::now() >= deadline {
                                    let message = format!("not ready within {}s ({reason})", def.start_timeout);
                                    self.log(id, &format!("[launchx] {message}; stopping it"));
                                    terminate(&mut child, pid, stop_timeout).await;
                                    self.home.remove_run(id);
                                    return Outcome::Exited { success: false, description: message, ran: begun.elapsed() };
                                }
                            } else {
                                misses += 1;
                                if misses >= 3 {
                                    self.update(id, |rt| { rt.health = Some(Health::Unhealthy); rt.detail = Some(reason); });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn probe(&self, id: &str, port: u16, token: &str, http: &HttpDef, level: u8) -> Probe {
        probe(&self.http, id, port, token, http, level).await
    }
}

/// One health check (contract §6.3).
pub async fn probe(client: &HttpClient, id: &str, port: u16, token: &str, http: &HttpDef, level: u8) -> Probe {
    let url = format!("http://127.0.0.1:{port}{}", http.health);
    let mut request = Request::get(&url);
    if level >= 1 {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let Ok(request) = request.body(Empty::new()) else { return Probe::Unhealthy("bad health path".into()) };
    let response = match tokio::time::timeout(Duration::from_secs(2), client.request(request)).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return Probe::Unhealthy(format!("no answer on port {port}: {}", root_cause(&error))),
        Err(_) => return Probe::Unhealthy("health check timed out".into()),
    };
    let status = response.status();
    let body = match tokio::time::timeout(Duration::from_secs(2), Limited::new(response.into_body(), 1 << 20).collect()).await {
        Ok(Ok(body)) => body.to_bytes(),
        _ => Bytes::new(),
    };
    if level == 0 {
        return if status.is_success() {
            Probe::Healthy { health: Health::Ok, summary: None, version: None, detail: None }
        } else {
            Probe::Unhealthy(format!("health returned {status}"))
        };
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return Probe::Unhealthy("not ready yet (503)".into());
    }
    if status != StatusCode::OK {
        return Probe::Unhealthy(format!("health returned {status}"));
    }
    let Ok(Value::Object(health)) = serde_json::from_slice::<Value>(&body) else {
        return Probe::Unhealthy("health did not return a JSON object".into());
    };
    let text = |key: &str| health.get(key).and_then(Value::as_str).map(str::to_string);
    if text("id").as_deref() != Some(id) {
        return Probe::Unhealthy(format!(
            "health reports id {:?}, not {id:?}; another program may be using port {port}",
            text("id").unwrap_or_default()
        ));
    }
    let state = match text("status").as_deref() {
        Some("ok") => Health::Ok,
        Some("degraded") => Health::Degraded,
        other => return Probe::Unhealthy(format!("health status {other:?} is not ok or degraded")),
    };
    Probe::Healthy { health: state, summary: text("summary"), version: text("version"), detail: text("detail") }
}

fn root_cause(error: &dyn std::error::Error) -> String {
    let mut current = error;
    while let Some(source) = current.source() {
        current = source;
    }
    current.to_string()
}

/// Build the command line and environment for one start (contract §4.4, §5.2).
pub fn build_launch(
    service: &Service,
    port: Option<u16>,
    token: &str,
    base_env: &[(String, String)],
    deps: &[(String, String, String)],
    config: &Config,
) -> Result<Launch, String> {
    let host = "127.0.0.1";
    let fill = |value: &str| {
        let mut value = value.replace("{host}", host);
        if let Some(port) = port {
            value = value.replace("{port}", &port.to_string());
        }
        value
    };
    let mut env: Vec<(String, String)> = base_env.to_vec();
    for (key, value) in &service.def.env {
        env.push((key.clone(), fill(value)));
    }
    env.push(("LAUNCHX".into(), manifest::CONTRACT_VERSION.into()));
    env.push(("LAUNCHX_SERVICE".into(), service.id.clone()));
    env.push(("LAUNCHX_HOST".into(), host.into()));
    if let Some(port) = port {
        env.push(("LAUNCHX_PORT".into(), port.to_string()));
        env.push(("LAUNCHX_TOKEN".into(), token.to_string()));
        env.push(("LAUNCHX_URL".into(), config.service_url(&service.id).trim_end_matches('/').to_string()));
    }
    for (dep, url, dep_token) in deps {
        let key = dep.to_uppercase().replace('-', "_");
        env.push((format!("LAUNCHX_DEP_{key}_URL"), url.clone()));
        env.push((format!("LAUNCHX_DEP_{key}_TOKEN"), dep_token.clone()));
    }
    // Later entries win, as in a shell.
    let mut seen = BTreeSet::new();
    let mut deduped: Vec<(String, String)> = env.into_iter().rev().filter(|(k, _)| seen.insert(k.clone())).collect();
    deduped.reverse();
    let env = deduped;

    let path = platform::lookup(&env, "PATH").unwrap_or("/usr/bin:/bin").to_string();
    for need in &service.def.needs {
        if platform::which(need, &path).is_none() {
            return Err(format!("needs `{need}`, which is not on PATH"));
        }
    }
    let cwd = service.cwd();
    if !cwd.is_dir() {
        return Err(format!("working directory {} does not exist", cwd.display()));
    }
    let args: Vec<String> = service.def.run.iter().map(|arg| fill(arg)).collect();
    let program = if args[0].contains('/') {
        let candidate = cwd.join(&args[0]);
        if !platform::executable(&candidate) {
            return Err(format!("{} is not an executable file", candidate.display()));
        }
        candidate
    } else {
        platform::which(&args[0], &path).ok_or_else(|| format!("`{}` is not on PATH", args[0]))?
    };
    Ok(Launch { program, args: args[1..].to_vec(), cwd, env, port, token: token.to_string() })
}

pub fn spawn(launch: &Launch) -> Result<Child, String> {
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .current_dir(&launch.cwd)
        .env_clear()
        .envs(launch.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(false);
    command.spawn().map_err(|error| format!("could not start {}: {error}", launch.program.display()))
}

/// SIGTERM, wait, then SIGKILL the whole process group (contract §5.4).
pub async fn terminate(child: &mut Child, pid: u32, timeout: Duration) -> String {
    util::signal(pid, libc::SIGTERM);
    let description = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => describe(status),
        _ => {
            util::signal_group(pid, libc::SIGKILL);
            let _ = child.wait().await;
            format!("did not exit within {}s of SIGTERM and was killed", timeout.as_secs())
        }
    };
    util::signal_group(pid, libc::SIGKILL);
    description
}

pub fn describe(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        _ => "exited".into(),
    }
}

async fn pump<R: AsyncRead + Unpin>(launcher: Arc<Launcher>, id: String, reader: R) {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let mut line = String::from_utf8_lossy(&buffer).trim_end_matches(['\n', '\r']).to_string();
                if line.len() > 8192 {
                    let mut cut = 8192;
                    while !line.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    line.truncate(cut);
                    line.push('…');
                }
                launcher.log(&id, &line);
            }
        }
    }
}

pub fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

/// Captured output of one service, rotated at 5 MB.
pub struct LogFile {
    path: PathBuf,
    file: Option<fs::File>,
    size: u64,
}

impl LogFile {
    fn new(path: PathBuf) -> LogFile {
        LogFile { path, file: None, size: 0 }
    }

    /// Append a line with a timestamp; returns the line as written.
    fn write(&mut self, line: &str) -> String {
        let stamped = format!("{} {line}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
        if self.file.is_none() || self.size > LOG_LIMIT {
            if self.size > LOG_LIMIT {
                let _ = fs::rename(&self.path, self.path.with_extension("log.1"));
            }
            self.file = fs::OpenOptions::new().create(true).append(true).open(&self.path).ok();
            self.size = self.file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
        }
        if let Some(file) = self.file.as_mut() {
            if writeln!(file, "{stamped}").is_ok() {
                self.size += stamped.len() as u64 + 1;
            }
        }
        stamped
    }
}

/// The last `lines` lines of a log file (reads at most the last 1 MB).
pub fn tail(path: &std::path::Path, lines: usize) -> Vec<String> {
    let Ok(mut file) = fs::File::open(path) else { return Vec::new() };
    let length = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = length.saturating_sub(1 << 20);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let mut all: Vec<&str> = text.lines().collect();
    if start > 0 && !all.is_empty() {
        all.remove(0); // probably a partial line
    }
    all[all.len().saturating_sub(lines)..].iter().map(|s| s.to_string()).collect()
}
