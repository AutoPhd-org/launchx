//! `lx`: the LaunchX launcher — daemon, gateway and command line in one executable.

mod agents;
mod check;
mod client;
mod gateway;
mod home;
mod install;
mod jobs;
mod manifest;
mod memory;
mod platform;
mod supervisor;
mod util;

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::client::Daemon;
use crate::home::{Home, RunFile};

#[derive(Parser)]
#[command(name = "lx", version, about = "LaunchX: run, watch and open your local services from one place")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the launcher in the foreground (what `lx up` registers)
    Daemon,
    /// Start the launcher now and at every login
    Up,
    /// Stop the launcher and its services, and stop starting it at login
    Down,
    /// List services with status and health
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Start a service and the services it requires
    Start { id: String },
    /// Stop a service, after the running services that require it
    Stop { id: String },
    /// Restart one service
    Restart { id: String },
    /// Open the launcher, or a service's page, in the browser, signed in
    Open {
        id: Option<String>,
        /// Print the sign-in link instead of opening a browser
        #[arg(long)]
        print: bool,
    },
    /// Show a service's captured output
    Logs {
        id: String,
        /// Keep printing new output
        #[arg(short, long)]
        follow: bool,
        /// Lines to show
        #[arg(short = 'n', long, default_value_t = 100)]
        lines: usize,
    },
    /// Register a manifest: a launchx.toml, or a directory that has one
    Add { path: PathBuf },
    /// Unregister the manifest that defines a service
    Rm { id: String },
    /// Install an app from a git URL, owner/repo on GitHub, or a folder; with no source, list installed apps
    Install {
        source: Option<String>,
        /// The name to install it under (default: the repository's)
        #[arg(long)]
        name: Option<String>,
        /// Do not ask before running the app's installer
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Update installed apps: pull, run the installer again, restart what was running
    Update {
        /// Apps (or service ids) to update; all of them when left out
        names: Vec<String>,
        /// Only fetch and report what has updates
        #[arg(long)]
        check: bool,
        /// Run the installer even when there is nothing new
        #[arg(long)]
        force: bool,
    },
    /// Remove an installed app: run its uninstaller, unregister it, delete the checkout LaunchX cloned. Its data stays
    Uninstall {
        /// The app's name, or one of its service ids
        name: String,
        #[arg(short, long)]
        yes: bool,
    },
    /// Start a service on a spare port and test it against its declared level
    Check {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the launcher's state document
    State,
    /// What LLM agents can discover: list those apps, or switch apps on or off
    Agents {
        #[command(subcommand)]
        action: Option<AgentsAction>,
        #[arg(long)]
        json: bool,
    },
    /// The user's shared memory: a wiki of pages their apps and agents keep, and who may use it
    Memory {
        #[command(subcommand)]
        action: Option<MemoryAction>,
    },
}

#[derive(Subcommand)]
enum MemoryAction {
    /// List the pages (the default)
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// Print a page; --json gives its fields and its rev
    Show {
        page: String,
        /// An earlier version, from `lx memory history`
        #[arg(long)]
        rev: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Write a page from stdin or --file; to change a page that exists, pass the rev you read as --base
    Write {
        page: String,
        /// Read the page from this file instead of stdin (front matter with title and summary is allowed)
        #[arg(long)]
        file: Option<PathBuf>,
        /// The rev of the page you read; leave it out to create a new page
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        title: Option<String>,
        /// One line on what the page holds; the index is made of these
        #[arg(long)]
        summary: Option<String>,
        /// One line on why, kept in the log
        #[arg(long)]
        note: Option<String>,
        /// Who is writing, for the log (an agent gives its own name)
        #[arg(long)]
        by: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Add text to the end of a page, creating the page if needed
    Append {
        page: String,
        /// The text; read from stdin when left out. It may start with '-', as a list item does
        #[arg(allow_hyphen_values = true)]
        text: Option<String>,
        /// Title for a page this creates
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        by: Option<String>,
    },
    /// Delete a page; its earlier versions stay in history
    Rm {
        page: String,
        /// The rev you read; without it, whatever the page holds now is deleted
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        by: Option<String>,
    },
    /// Earlier versions of a page, newest first
    History {
        page: String,
        #[arg(long)]
        json: bool,
    },
    /// Recent writes, newest first: who changed which page, and why
    Log {
        #[arg(short = 'n', long, default_value_t = 20)]
        lines: usize,
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Pages that contain all the words
    Search {
        #[arg(required = true)]
        words: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// What is worth tidying: broken links, missing summaries, large, duplicate or stale pages
    Lint {
        #[arg(long)]
        json: bool,
    },
    /// The short index of every page, as agents get it at the start of a session
    Index,
    /// Who may use memory, or set it: lx memory access <app|agents> <none|read|write>
    Access {
        /// A service id, or `agents` for the user's AI agents
        who: Option<String>,
        /// none, read or write
        level: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Open the memory page in the browser, signed in
    Open {
        #[arg(long)]
        print: bool,
    },
}

#[derive(Subcommand)]
enum AgentsAction {
    /// Make services discoverable by agents: list them and publish their skills
    On {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Hide services from agents and withdraw their skills
    Off {
        #[arg(required = true)]
        ids: Vec<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    match runtime.block_on(run(cli.command)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("lx: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> anyhow::Result<ExitCode> {
    let home = Home::from_env();
    match command {
        Command::Daemon => daemon(home).await,
        Command::Up => up(home).await,
        Command::Down => down(home).await,
        Command::Ls { json } => ls(home, json).await,
        Command::Start { id } => start(home, &id).await,
        Command::Stop { id } => stop(home, &id).await,
        Command::Restart { id } => {
            let daemon = Daemon::require(&home)?;
            daemon.post(&format!("/api/services/{id}/restart"), json!({})).await?;
            wait_running(&daemon, &[id]).await
        }
        Command::Open { id, print } => open(home, id, print).await,
        Command::Logs { id, follow, lines } => logs(home, &id, follow, lines).await,
        Command::Add { path } => add(home, &path).await,
        Command::Rm { id } => rm(home, &id).await,
        Command::Install { source: None, json, .. } => list_installs(home, json),
        Command::Install { source: Some(source), name, yes, .. } => install_app(home, &source, name.as_deref(), yes).await,
        Command::Update { names, check, force } => update_apps(home, names, check, force).await,
        Command::Uninstall { name, yes } => uninstall_app(home, &name, yes).await,
        Command::Check { id, json } => {
            let config = home.config()?;
            let ok = check::check(&home, &config, &id, json).await?;
            Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
        Command::State => {
            let state = Daemon::require(&home)?.get("/api/state").await?;
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(ExitCode::SUCCESS)
        }
        Command::Agents { action: None, json } => list_agents(home, json).await,
        Command::Agents { action: Some(AgentsAction::On { ids }), .. } => switch_agents(home, ids, true).await,
        Command::Agents { action: Some(AgentsAction::Off { ids }), .. } => switch_agents(home, ids, false).await,
        Command::Memory { action } => memory_command(home, action.unwrap_or(MemoryAction::Ls { json: false })).await,
    }
}

// --- Daemon ----------------------------------------------------------------

async fn daemon(home: Home) -> anyhow::Result<ExitCode> {
    home.ensure()?;
    let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(home.lock_path())?;
    // SAFETY: flock(2) on a descriptor we own; held for the daemon's lifetime.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let pid = home.read_run("launchx").map(|r| format!(" (pid {})", r.pid)).unwrap_or_default();
        bail!("LaunchX is already running{pid}");
    }
    let config = home.config()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", config.gateway_port))
        .await
        .with_context(|| format!("cannot listen on 127.0.0.1:{}; set gateway_port in {}", config.gateway_port, home.config_path().display()))?;
    let launcher = supervisor::Launcher::new(home.clone(), config.clone())?;
    launcher.cleanup_orphans().await;
    let run = RunFile {
        id: "launchx".into(),
        pid: std::process::id(),
        url: format!("http://127.0.0.1:{}", config.gateway_port),
        token: launcher.token(),
        ui: Some(config.launcher_url()),
        version: Some(supervisor::VERSION.into()),
        started_at: util::now_rfc3339(),
    };
    home.write_run(&run)?;
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let app = gateway::App::new(launcher.clone(), shutdown_tx.clone());
    let server = tokio::spawn(gateway::serve(app, listener));
    launcher.autostart();
    jobs::watch_updates(&launcher);
    println!("LaunchX {} listening at {}", supervisor::VERSION, config.launcher_url());

    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
        _ = async { while !*shutdown_rx.borrow() { if shutdown_rx.changed().await.is_err() { break; } } } => {}
    }
    println!("LaunchX stopping services…");
    // Stop services first; their streams through the gateway close with them.
    launcher.shutdown().await;
    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
    home.remove_run("launchx");
    drop(lock);
    println!("LaunchX stopped.");
    Ok(ExitCode::SUCCESS)
}

async fn up(home: Home) -> anyhow::Result<ExitCode> {
    home.ensure()?;
    if let Some(run) = home.read_run("launchx") {
        println!("LaunchX is already running (pid {}) at {}", run.pid, run.ui.unwrap_or_default());
        return Ok(ExitCode::SUCCESS);
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        eprintln!("Note: registering {} — a build directory. Install lx somewhere stable first (./install.sh).", exe.display());
    }
    println!("{}", platform::install(&exe, &home)?);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(run) = home.read_run("launchx") {
            println!("LaunchX is running at {}", run.ui.unwrap_or_default());
            return Ok(ExitCode::SUCCESS);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    bail!("LaunchX did not start; see {}", home.logs_dir().join("launchx.log").display())
}

async fn down(home: Home) -> anyhow::Result<ExitCode> {
    let registered = platform::uninstall()?;
    if !registered {
        match Daemon::find(&home) {
            Some(daemon) => {
                daemon.post("/api/shutdown", json!({})).await?;
            }
            None => {
                println!("LaunchX is not running.");
                return Ok(ExitCode::SUCCESS);
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    while home.read_run("launchx").is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!("LaunchX stopped{}.", if registered { " and will not start at login" } else { "" });
    Ok(ExitCode::SUCCESS)
}

// --- Services ----------------------------------------------------------------

fn text(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => "—".into(),
    }
}

fn table(rows: Vec<Vec<String>>) {
    let widths: Vec<usize> = (0..rows[0].len()).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    for row in rows {
        let line: Vec<String> = row.iter().zip(&widths).map(|(cell, width)| format!("{cell:<width$}")).collect();
        println!("{}", line.join("  ").trim_end());
    }
}

async fn ls(home: Home, as_json: bool) -> anyhow::Result<ExitCode> {
    let Some(daemon) = Daemon::find(&home) else {
        let (services, errors) = manifest::load_registry(&home);
        let discoverable = home.load_state()?.agents;
        if as_json {
            let list: Vec<Value> = services
                .iter()
                .map(|s| json!({ "id": s.id, "name": s.def.name, "status": "unknown", "error": s.error, "agents": { "discoverable": discoverable.contains(&s.id) } }))
                .collect();
            println!("{}", serde_json::to_string_pretty(&json!({ "running": false, "services": list, "errors": errors }))?);
            return Ok(ExitCode::SUCCESS);
        }
        println!("LaunchX is not running. Start it with `lx up`.\n");
        let mut rows = vec![vec!["ID".into(), "NAME".into(), "AGENTS".into(), "MANIFEST".into()]];
        rows.extend(services.iter().map(|s| vec![s.id.clone(), s.def.name.clone(), on_off(discoverable.contains(&s.id)), s.manifest.display().to_string()]));
        table(rows);
        for error in errors {
            eprintln!("! {error}");
        }
        return Ok(ExitCode::SUCCESS);
    };
    let state = daemon.get("/api/state").await?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&state["services"])?);
        return Ok(ExitCode::SUCCESS);
    }
    let services = state["services"].as_array().cloned().unwrap_or_default();
    if services.is_empty() {
        println!("No services registered. Add one with `lx add <path>`.");
    } else {
        let mut rows = vec![vec![
            "ID".into(),
            "STATUS".into(),
            "HEALTH".into(),
            "VERSION".into(),
            "PORT".into(),
            "AGENTS".into(),
            "MEMORY".into(),
            "SUMMARY".into(),
        ]];
        for s in &services {
            let summary = match s.get("error").and_then(Value::as_str) {
                Some(error) => error.to_string(),
                None => s.get("summary").or(s.get("detail")).and_then(Value::as_str).unwrap_or("").to_string(),
            };
            let agents = on_off(s["agents"]["discoverable"].as_bool().unwrap_or(false));
            let memory = s["memory"]["access"].as_str().unwrap_or("none").to_string();
            rows.push(vec![text(s, "id"), text(s, "status"), text(s, "health"), text(s, "version"), text(s, "port"), agents, memory, summary]);
        }
        table(rows);
    }
    for error in state["errors"].as_array().into_iter().flatten() {
        eprintln!("! {}", error.as_str().unwrap_or(""));
    }
    Ok(ExitCode::SUCCESS)
}

fn on_off(on: bool) -> String {
    (if on { "on" } else { "off" }).into()
}

// --- Agents (§10.7) -----------------------------------------------------------

fn tilde(path: &std::path::Path) -> String {
    match path.strip_prefix(home::home_dir()) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

async fn list_agents(home: Home, as_json: bool) -> anyhow::Result<ExitCode> {
    let config = home.config()?;
    let index = match Daemon::find(&home) {
        Some(daemon) => daemon.get("/api/agents").await?,
        None => {
            let (services, _) = manifest::load_registry(&home);
            let mut index = agents::index(&home, &config, &services, &home.load_state()?.agents, |_| None);
            index["launcher"] = json!({ "running": false });
            index
        }
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&index)?);
        return Ok(ExitCode::SUCCESS);
    }
    let apps = index["apps"].as_array().cloned().unwrap_or_default();
    if apps.is_empty() {
        println!("No app is discoverable by agents. Switch one on with: lx agents on <id>");
        return Ok(ExitCode::SUCCESS);
    }
    println!("Apps that agents can discover:\n");
    let mut rows = vec![vec!["ID".into(), "NAME".into(), "STATUS".into(), "CLI".into(), "SKILLS".into()]];
    for app in &apps {
        let skills: Vec<&str> = app["skills"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str()).collect();
        let status = app["status"].as_str().unwrap_or(if index["launcher"]["running"] == true { "—" } else { "LaunchX not running" });
        rows.push(vec![text(app, "id"), text(app, "name"), status.into(), text(app, "cli"), if skills.is_empty() { "—".into() } else { skills.join(", ") }]);
    }
    table(rows);
    let folders: Vec<String> = config.agent_skill_dirs().iter().filter(|d| d.is_dir()).map(|d| tilde(d)).collect();
    if folders.is_empty() {
        println!("\nNo agent skill folder exists yet ({}); set agent_skill_dirs in {}.", config.agent_skill_dirs.join(", "), tilde(&home.config_path()));
    } else {
        println!("\nAgents find them through the `launchx` skill in {}.", folders.join(" and "));
    }
    Ok(ExitCode::SUCCESS)
}

async fn switch_agents(home: Home, ids: Vec<String>, on: bool) -> anyhow::Result<ExitCode> {
    let problems: Vec<String> = match Daemon::find(&home) {
        Some(daemon) => {
            let state = daemon.get("/api/state").await?;
            let known: Vec<&str> = state["services"].as_array().into_iter().flatten().filter_map(|s| s["id"].as_str()).collect();
            if let Some(unknown) = ids.iter().find(|id| !known.contains(&id.as_str())) {
                bail!("Unknown service '{unknown}'");
            }
            let mut problems = Vec::new();
            for id in &ids {
                let reply = daemon.post(&format!("/api/services/{id}/agents"), json!({ "discoverable": on })).await?;
                problems = serde_json::from_value(reply["problems"].clone()).unwrap_or_default();
            }
            problems
        }
        None => {
            home.ensure()?;
            let config = home.config()?;
            let (services, _) = manifest::load_registry(&home);
            if let Some(unknown) = ids.iter().find(|id| !services.iter().any(|s| &s.id == *id)) {
                bail!("Unknown service '{unknown}'");
            }
            let mut state = home.load_state()?;
            for id in &ids {
                if on {
                    state.agents.insert(id.clone());
                } else {
                    state.agents.remove(id);
                }
            }
            home.save_state(&state)?;
            agents::publish(&home, &config, &services, &state.agents, memory_skill(&home, &state).as_deref()).problems
        }
    };
    for id in &ids {
        println!("{id} is {}.", if on { "now discoverable by agents" } else { "now hidden from agents" });
    }
    for problem in problems {
        eprintln!("! {problem}");
    }
    Ok(ExitCode::SUCCESS)
}

async fn start(home: Home, id: &str) -> anyhow::Result<ExitCode> {
    let daemon = Daemon::require(&home)?;
    let reply = daemon.post(&format!("/api/services/{id}/start"), json!({})).await?;
    let started: Vec<String> = serde_json::from_value(reply["started"].clone()).unwrap_or_default();
    if started.is_empty() {
        println!("{id} is already running or starting.");
    }
    wait_running(&daemon, &if started.is_empty() { vec![id.to_string()] } else { started }).await
}

/// Wait until each service is running, and say how each one ended up.
async fn wait_running(daemon: &Daemon, ids: &[String]) -> anyhow::Result<ExitCode> {
    let mut pending: Vec<String> = ids.to_vec();
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut failed = false;
    while !pending.is_empty() && Instant::now() < deadline {
        let state = daemon.get("/api/state").await?;
        let services = state["services"].as_array().cloned().unwrap_or_default();
        let mut still = Vec::new();
        for id in pending {
            let Some(service) = services.iter().find(|s| s["id"] == id.as_str()) else { continue };
            match service["status"].as_str().unwrap_or("") {
                "running" => {
                    let summary = service["summary"].as_str().map(|s| format!(" — {s}")).unwrap_or_default();
                    let ui = service["ui"].as_str().map(|u| format!("  {u}")).unwrap_or_default();
                    println!("✓ {id} running{summary}{ui}");
                }
                "failed" | "stopped" => {
                    failed = true;
                    let error = service["error"].as_str().or(service["last_exit"].as_str()).unwrap_or("stopped");
                    println!("✗ {id}: {error}");
                    if let Ok(logs) = daemon.get(&format!("/api/services/{id}/logs?tail=8")).await {
                        for line in logs["lines"].as_array().into_iter().flatten() {
                            println!("    {}", line.as_str().unwrap_or(""));
                        }
                    }
                    println!("  More: lx logs {id}");
                }
                _ => still.push(id),
            }
        }
        pending = still;
        if !pending.is_empty() {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    for id in &pending {
        println!("… {id} is still starting. Watch it with: lx logs -f {id}");
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

async fn stop(home: Home, id: &str) -> anyhow::Result<ExitCode> {
    let daemon = Daemon::require(&home)?;
    let state = daemon.get("/api/state").await?;
    let dependents: Vec<String> = state["services"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["requires"].as_array().is_some_and(|r| r.iter().any(|x| x == id)))
        .filter(|s| !matches!(s["status"].as_str(), Some("stopped" | "failed")))
        .filter_map(|s| s["id"].as_str().map(str::to_string))
        .collect();
    if !dependents.is_empty() {
        println!("Stopping {} first, since it requires {id}.", dependents.join(", "));
    }
    let reply = daemon.post(&format!("/api/services/{id}/stop"), json!({})).await?;
    let stopped: Vec<String> = serde_json::from_value(reply["stopped"].clone()).unwrap_or_default();
    if stopped.is_empty() {
        println!("{id} is not running.");
    } else {
        println!("Stopped {}.", stopped.join(", "));
    }
    Ok(ExitCode::SUCCESS)
}

// --- Shared memory (§12) -------------------------------------------------------

/// The text of the `launchx-memory` skill, while the user lets agents use memory.
fn memory_skill(home: &Home, state: &home::Persisted) -> Option<String> {
    state.memory_agents.can_read().then(|| memory::skill(&memory::Store::new(home.memory_dir()), state.memory_agents))
}

/// Without a launcher: bring the agents' skill folders in line with the switches.
fn republish_offline(home: &Home) -> anyhow::Result<Vec<String>> {
    let config = home.config()?;
    let (services, _) = manifest::load_registry(home);
    let state = home.load_state()?;
    Ok(agents::publish(home, &config, &services, &state.agents, memory_skill(home, &state).as_deref()).problems)
}

/// One call to the memory API: through the launcher when it runs, else on the files.
async fn memory_call(home: &Home, method: &str, rest: &str, query: &[(&str, String)], body: Value) -> anyhow::Result<Value> {
    if let Some(daemon) = Daemon::find(home) {
        let mut path = format!("/api/memory{rest}");
        if !query.is_empty() {
            let pairs: Vec<String> = query.iter().map(|(key, value)| format!("{key}={}", util::url_encode(value))).collect();
            path = format!("{path}?{}", pairs.join("&"));
        }
        return daemon.send(method, &path, (method != "GET").then_some(body)).await;
    }
    home.ensure()?;
    let store = memory::Store::new(home.memory_dir());
    let segments: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
    let query: std::collections::HashMap<String, String> = query.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    let (status, value) = memory::api(&store, &memory::Who::User, method, &segments, &query, &body);
    if status != 200 {
        bail!("{}", value["error"].as_str().unwrap_or("memory request failed"));
    }
    if method != "GET" && home.load_state()?.memory_agents.can_read() {
        for problem in republish_offline(home)? {
            eprintln!("! {problem}");
        }
    }
    Ok(value)
}

fn read_input(file: Option<&std::path::Path>) -> anyhow::Result<String> {
    use std::io::IsTerminal;
    match file {
        Some(path) => fs::read_to_string(path).with_context(|| format!("reading {}", path.display())),
        None if std::io::stdin().is_terminal() => bail!("Pipe the text in (lx memory write <page> < page.md) or pass --file"),
        None => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            Ok(text)
        }
    }
}

/// An RFC 3339 time from memory, in the user's own time zone.
fn local(value: &Value, format: &str) -> String {
    match value.as_str() {
        Some(time) => chrono::DateTime::parse_from_rfc3339(time)
            .map(|t| t.with_timezone(&chrono::Local).format(format).to_string())
            .unwrap_or_else(|_| time.to_string()),
        None => "—".into(),
    }
}

fn day(value: &Value) -> String {
    local(value, "%Y-%m-%d")
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    format!("{}…", text.chars().take(limit.saturating_sub(1)).collect::<String>().trim_end())
}

fn print_json(value: &Value) -> anyhow::Result<ExitCode> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(ExitCode::SUCCESS)
}

async fn memory_command(home: Home, action: MemoryAction) -> anyhow::Result<ExitCode> {
    match action {
        MemoryAction::Ls { json } => {
            let listing = memory_call(&home, "GET", "", &[], json!({})).await?;
            if json {
                return print_json(&listing);
            }
            let pages = listing["pages"].as_array().cloned().unwrap_or_default();
            if pages.is_empty() {
                println!("Memory is empty. Write a page with: lx memory write profile --summary \"Who I am\" < profile.md");
            } else {
                let mut rows = vec![vec!["PAGE".into(), "TITLE".into(), "UPDATED".into(), "BY".into(), "SUMMARY".into()]];
                for page in &pages {
                    rows.push(vec![text(page, "page"), clip(&text(page, "title"), 30), day(&page["updated"]), text(page, "updated_by"), clip(page["summary"].as_str().unwrap_or(""), 60)]);
                }
                table(rows);
            }
            let missing: Vec<&str> = listing["suggested"].as_array().into_iter().flatten().filter_map(|s| s["page"].as_str()).collect();
            if !missing.is_empty() {
                println!("\nNot written yet: {}", missing.join(", "));
            }
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Show { page, rev, json } => {
            let query: Vec<(&str, String)> = rev.map(|r| vec![("rev", r)]).unwrap_or_default();
            let found = memory_call(&home, "GET", &format!("/pages/{page}"), &query, json!({})).await?;
            if json {
                return print_json(&found);
            }
            print!("{}", found["text"].as_str().unwrap_or(""));
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Write { page, file, base, title, summary, note, by, json } => {
            let mut body = json!({ "text": read_input(file.as_deref())?, "base": base });
            for (key, value) in [("title", title), ("summary", summary), ("note", note), ("by", by)] {
                if let Some(value) = value {
                    body[key] = json!(value);
                }
            }
            let written = memory_call(&home, "PUT", &format!("/pages/{page}"), &[], body).await?;
            if json {
                return print_json(&written);
            }
            println!("Saved {page} (rev {}).", text(&written, "rev"));
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Append { page, text: addition, title, note, by } => {
            let addition = match addition {
                Some(addition) => addition,
                None => read_input(None)?,
            };
            let mut body = json!({ "text": addition });
            for (key, value) in [("title", title), ("note", note), ("by", by)] {
                if let Some(value) = value {
                    body[key] = json!(value);
                }
            }
            let written = memory_call(&home, "POST", &format!("/append/{page}"), &[], body).await?;
            println!("Added to {page} (rev {}).", text(&written, "rev"));
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Rm { page, base, note, by } => {
            let base = match base {
                Some(base) => base,
                None => text(&memory_call(&home, "GET", &format!("/pages/{page}"), &[], json!({})).await?, "rev"),
            };
            memory_call(&home, "DELETE", &format!("/pages/{page}"), &[], json!({ "base": base, "note": note, "by": by })).await?;
            println!("Deleted {page}. Its versions stay in history: lx memory history {page}");
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::History { page, json } => {
            let history = memory_call(&home, "GET", &format!("/history/{page}"), &[], json!({})).await?;
            if json {
                return print_json(&history);
            }
            let versions = history["versions"].as_array().cloned().unwrap_or_default();
            if versions.is_empty() {
                println!("{page} has no earlier versions.");
                return Ok(ExitCode::SUCCESS);
            }
            let mut rows = vec![vec!["REV".into(), "WRITTEN".into(), "BY".into(), "REPLACED".into(), "TITLE".into()]];
            for version in &versions {
                let replaced = local(&version["replaced"], "%Y-%m-%d %H:%M");
                rows.push(vec![text(version, "rev"), day(&version["updated"]), text(version, "updated_by"), replaced, text(version, "title")]);
            }
            table(rows);
            println!("\nRead one with: lx memory show {page} --rev <rev>");
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Log { lines, page, json } => {
            let mut query = vec![("tail", lines.to_string())];
            if let Some(page) = page {
                query.push(("page", page));
            }
            let log = memory_call(&home, "GET", "/log", &query, json!({})).await?;
            if json {
                return print_json(&log);
            }
            let entries = log["entries"].as_array().cloned().unwrap_or_default();
            if entries.is_empty() {
                println!("No writes yet.");
                return Ok(ExitCode::SUCCESS);
            }
            let mut rows = vec![vec!["TIME".into(), "BY".into(), "ACTION".into(), "PAGE".into(), "NOTE".into()]];
            for entry in &entries {
                let time = local(&entry["time"], "%Y-%m-%d %H:%M");
                rows.push(vec![time, text(entry, "by"), text(entry, "action"), text(entry, "page"), entry["note"].as_str().unwrap_or("").to_string()]);
            }
            table(rows);
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Search { words, json } => {
            let found = memory_call(&home, "GET", "/search", &[("q", words.join(" "))], json!({})).await?;
            if json {
                return print_json(&found);
            }
            let pages = found["pages"].as_array().cloned().unwrap_or_default();
            if pages.is_empty() {
                println!("No page contains {}.", words.join(" and "));
            }
            for page in &pages {
                println!("{} — {}", text(page, "page"), text(page, "title"));
                for line in page["lines"].as_array().into_iter().flatten() {
                    println!("    {}", line.as_str().unwrap_or(""));
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Lint { json } => {
            let found = memory_call(&home, "GET", "/lint", &[], json!({})).await?;
            if json {
                return print_json(&found);
            }
            let problems = found["problems"].as_array().cloned().unwrap_or_default();
            if problems.is_empty() {
                println!("Nothing to tidy.");
            }
            for problem in &problems {
                match problem["page"].as_str() {
                    Some(page) => println!("{page} {}", text(problem, "detail")),
                    None => println!("{}", text(problem, "detail")),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Index => {
            let index = memory_call(&home, "GET", "/index", &[], json!({})).await?;
            print!("{}", index["markdown"].as_str().unwrap_or(""));
            Ok(ExitCode::SUCCESS)
        }
        MemoryAction::Access { who: None, json, .. } => memory_access_list(&home, json).await,
        MemoryAction::Access { who: Some(who), level: None, .. } => {
            bail!("Say what {who} may do: lx memory access {who} none|read|write")
        }
        MemoryAction::Access { who: Some(who), level: Some(level), .. } => {
            let access = memory::Access::parse(&level).ok_or_else(|| anyhow!("'{level}' is not none, read or write"))?;
            memory_access_set(&home, &who, access).await
        }
        MemoryAction::Open { print } => open_at(home, None, "/#memory", print).await,
    }
}

fn access_words(access: &str) -> &'static str {
    match access {
        "read" => "read",
        "write" => "read and write",
        _ => "off",
    }
}

async fn memory_access_list(home: &Home, as_json: bool) -> anyhow::Result<ExitCode> {
    let (apps, agents_access): (Vec<Value>, String) = match Daemon::find(home) {
        Some(daemon) => {
            let state = daemon.get("/api/state").await?;
            let apps = state["services"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| json!({ "id": s["id"], "name": s["name"], "access": s["memory"]["access"], "wants": s["memory"]["wants"], "reason": s["memory"]["reason"] }))
                .collect();
            (apps, state["memory"]["agents"].as_str().unwrap_or("none").to_string())
        }
        None => {
            let (services, _) = manifest::load_registry(home);
            let state = home.load_state()?;
            let apps = services
                .iter()
                .map(|s| {
                    let memory = s.def.memory.clone().unwrap_or_default();
                    json!({
                        "id": s.id, "name": s.def.name,
                        "access": state.memory.get(&s.id).copied().unwrap_or_default(),
                        "wants": s.def.memory.as_ref().map(|_| memory.wants), "reason": memory.reason,
                    })
                })
                .collect();
            (apps, state.memory_agents.as_str().to_string())
        }
    };
    if as_json {
        return print_json(&json!({ "apps": apps, "agents": agents_access }));
    }
    let mut rows = vec![vec!["WHO".into(), "NAME".into(), "MEMORY".into(), "ASKS FOR".into()]];
    for app in &apps {
        let asks = match (app["wants"].as_str(), app["reason"].as_str()) {
            (Some(wants), Some(reason)) => format!("{}: {}", access_words(wants), clip(reason, 60)),
            (Some(wants), None) => access_words(wants).to_string(),
            _ => "—".into(),
        };
        rows.push(vec![text(app, "id"), text(app, "name"), access_words(app["access"].as_str().unwrap_or("none")).into(), asks]);
    }
    rows.push(vec!["agents".into(), "Your AI agents".into(), access_words(&agents_access).into(), "—".into()]);
    table(rows);
    println!("\nChange it with: lx memory access <who> none|read|write");
    Ok(ExitCode::SUCCESS)
}

async fn memory_access_set(home: &Home, who: &str, access: memory::Access) -> anyhow::Result<ExitCode> {
    let mut problems: Vec<String> = Vec::new();
    match Daemon::find(home) {
        Some(daemon) => {
            if who == "agents" {
                let reply = daemon.post("/api/memory/agents", json!({ "access": access.as_str() })).await?;
                problems = serde_json::from_value(reply["problems"].clone()).unwrap_or_default();
            } else {
                daemon.post(&format!("/api/services/{who}/memory"), json!({ "access": access.as_str() })).await?;
            }
        }
        None => {
            home.ensure()?;
            let mut state = home.load_state()?;
            if who == "agents" {
                state.memory_agents = access;
            } else {
                let (services, _) = manifest::load_registry(home);
                if !services.iter().any(|s| s.id == who) {
                    bail!("Unknown service '{who}'");
                }
                if access == memory::Access::None {
                    state.memory.remove(who);
                } else {
                    state.memory.insert(who.to_string(), access);
                }
            }
            home.save_state(&state)?;
            if who == "agents" {
                problems = republish_offline(home)?;
            }
        }
    }
    let subject = if who == "agents" { "Your agents".to_string() } else { who.to_string() };
    match access {
        memory::Access::None => println!("{subject} may no longer use your memory."),
        memory::Access::Read => println!("{subject} may now read your memory."),
        memory::Access::Write => println!("{subject} may now read and write your memory."),
    }
    if who == "agents" && access.can_read() {
        println!("They learn about it from the `launchx-memory` skill. They run as you, so this decides what they are told, not what they could reach.");
    }
    for problem in problems {
        eprintln!("! {problem}");
    }
    Ok(ExitCode::SUCCESS)
}

async fn open(home: Home, id: Option<String>, print: bool) -> anyhow::Result<ExitCode> {
    open_at(home, id, "/", print).await
}

async fn open_at(home: Home, id: Option<String>, next: &str, print: bool) -> anyhow::Result<ExitCode> {
    let daemon = Daemon::require(&home)?;
    if let Some(id) = &id {
        let state = daemon.get("/api/state").await?;
        let service = state["services"].as_array().into_iter().flatten().find(|s| s["id"] == id.as_str()).cloned();
        let Some(service) = service else { bail!("Unknown service '{id}'") };
        if service["ui"].is_null() {
            bail!("{id} has no browser UI");
        }
        if service["status"] != "running" {
            let reply = daemon.post(&format!("/api/services/{id}/start"), json!({})).await?;
            let mut started: Vec<String> = serde_json::from_value(reply["started"].clone()).unwrap_or_default();
            if started.is_empty() {
                started.push(id.clone());
            }
            if wait_running(&daemon, &started).await? != ExitCode::SUCCESS {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    let body = match &id {
        Some(id) => json!({ "service": id, "next": next }),
        None => json!({ "next": next }),
    };
    let link = daemon.post("/api/link", body).await?;
    let url = link["url"].as_str().ok_or_else(|| anyhow!("LaunchX returned no link"))?;
    if print {
        println!("{url}");
    } else {
        platform::open_url(url)?;
        let what = match (&id, next) {
            (Some(id), _) => id.clone(),
            (None, "/#memory") => "your memory".to_string(),
            (None, _) => "LaunchX".to_string(),
        };
        println!("Opened {what} in your browser.");
    }
    Ok(ExitCode::SUCCESS)
}

async fn logs(home: Home, id: &str, follow: bool, lines: usize) -> anyhow::Result<ExitCode> {
    let (services, _) = manifest::load_registry(&home);
    if !services.iter().any(|s| s.id == id) {
        bail!("Unknown service '{id}'");
    }
    let path = home.log_path(id);
    for line in supervisor::tail(&path, lines) {
        println!("{line}");
    }
    if !follow {
        return Ok(ExitCode::SUCCESS);
    }
    let mut position = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let length = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if length < position {
            position = 0; // rotated
        }
        if length > position {
            let mut file = fs::File::open(&path)?;
            file.seek(SeekFrom::Start(position))?;
            let mut chunk = String::new();
            file.read_to_string(&mut chunk)?;
            print!("{chunk}");
            position = length;
        }
    }
}

async fn add(home: Home, path: &std::path::Path) -> anyhow::Result<ExitCode> {
    let file = manifest::locate(path)?;
    let Some(services) = manifest::register(&home, &file)? else {
        println!("Already registered: {}", file.display());
        return Ok(ExitCode::SUCCESS);
    };
    for service in &services {
        println!("Registered {} ({}), level {}, from {}", service.id, service.def.name, service.def.level, file.display());
    }
    if let Some(daemon) = Daemon::find(&home) {
        daemon.post("/api/reload", json!({})).await?;
        println!("Start it with: lx start {}", services[0].id);
    } else {
        println!("LaunchX is not running; it will see this when it starts (lx up).");
    }
    if services.iter().any(|s| s.def.level > 0) {
        println!("Verify the level it claims with: lx check {}", services[0].id);
    }
    Ok(ExitCode::SUCCESS)
}

async fn rm(home: Home, id: &str) -> anyhow::Result<ExitCode> {
    let (services, _) = manifest::load_registry(&home);
    let Some(service) = services.iter().find(|s| s.id == id) else { bail!("Unknown service '{id}'") };
    let siblings: Vec<String> = services.iter().filter(|s| s.entry == service.entry).map(|s| s.id.clone()).collect();
    if siblings.len() > 1 {
        println!("{} also defines {}; unregistering all of them.", service.manifest.display(), siblings.join(", "));
    }
    let daemon = Daemon::find(&home);
    if let Some(daemon) = &daemon {
        for sibling in &siblings {
            daemon.post(&format!("/api/services/{sibling}/stop"), json!({})).await?;
        }
    }
    let removed = manifest::unregister(&home, id)?;
    if let Some(daemon) = &daemon {
        daemon.post("/api/reload", json!({})).await?;
    }
    println!("Unregistered {} (the manifest itself is untouched).", removed.join(", "));
    if let Some(package) = install::Installs::load(&home).owner_of(id) {
        println!("It was installed with lx install; to remove the app itself: lx uninstall {}", package.name);
    }
    Ok(ExitCode::SUCCESS)
}

// --- Installing apps (§10.8) ----------------------------------------------------

/// Stopping and starting services through a running launcher, if there is one.
struct ViaDaemon(Option<Daemon>);

impl install::Services for ViaDaemon {
    async fn stop(&self, ids: &[String]) -> Vec<String> {
        let Some(daemon) = &self.0 else { return Vec::new() };
        let Ok(state) = daemon.get("/api/state").await else { return Vec::new() };
        let mut stopped = Vec::new();
        for id in ids {
            let running = state["services"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|s| s["id"] == id.as_str() && !matches!(s["status"].as_str(), Some("stopped" | "failed")));
            if !running {
                continue;
            }
            if let Ok(reply) = daemon.post(&format!("/api/services/{id}/stop"), json!({})).await {
                for other in reply["stopped"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    if !stopped.iter().any(|s| s == other) {
                        stopped.push(other.to_string());
                    }
                }
            }
        }
        stopped
    }

    async fn reload_and_start(&self, ids: &[String]) {
        let Some(daemon) = &self.0 else { return };
        let _ = daemon.post("/api/reload", json!({})).await;
        for id in ids {
            if let Err(error) = daemon.post(&format!("/api/services/{id}/start"), json!({})).await {
                eprintln!("! could not start {id} again: {error:#}");
            }
        }
    }
}

fn terminal_out() -> install::Out {
    std::sync::Arc::new(|line: &str| println!("  {line}"))
}

fn install_env(home: &Home, action: &str) -> Vec<(String, String)> {
    install::environment(home, std::env::vars().collect(), action)
}

/// Ask a yes/no question on the terminal; without one, refuse unless --yes.
fn confirm(question: &str, yes: bool) -> anyhow::Result<bool> {
    use std::io::{IsTerminal, Write};
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("Not asking without a terminal; pass --yes to go ahead");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn list_installs(home: Home, as_json: bool) -> anyhow::Result<ExitCode> {
    let installs = install::Installs::load(&home);
    if as_json {
        return print_json(&json!({ "packages": installs.packages.values().map(install::Package::to_json).collect::<Vec<_>>() }));
    }
    if installs.packages.is_empty() {
        println!("Nothing is installed with lx install yet. Install an app with: lx install <git-url | owner/repo | folder>");
        return Ok(ExitCode::SUCCESS);
    }
    let mut rows = vec![vec!["NAME".into(), "SERVICES".into(), "VERSION".into(), "UPDATED".into(), "SOURCE".into()]];
    for package in installs.packages.values() {
        let mut version = package.label();
        if let Some(available) = &package.available {
            version.push_str(&format!(" ({available} available)"));
        } else if let Some(behind) = package.behind.filter(|n| *n > 0) {
            version.push_str(&format!(" ({behind} new)"));
        }
        let when = package.updated_at.as_deref().unwrap_or(&package.installed_at);
        let source = if package.cloned { package.source.clone() } else { format!("{} (folder)", tilde(&package.dir)) };
        rows.push(vec![package.name.clone(), package.services.join(", "), version, local(&json!(when), "%Y-%m-%d"), source]);
    }
    table(rows);
    Ok(ExitCode::SUCCESS)
}

async fn install_app(home: Home, source: &str, name: Option<&str>, yes: bool) -> anyhow::Result<ExitCode> {
    let _lock = install::lock(&home)?;
    let out = terminal_out();
    let env = install_env(&home, "install");
    let plan = {
        let (home, env, out, source, name) = (home.clone(), env.clone(), out.clone(), source.to_string(), name.map(str::to_string));
        tokio::task::spawn_blocking(move || install::prepare(&home, &env, &source, name.as_deref(), &out)).await??
    };
    for line in plan.describe() {
        println!("{line}");
    }
    if !plan.install.run.is_empty() && !confirm("Run the installer?", yes)? {
        println!("Nothing installed.");
        return Ok(ExitCode::SUCCESS);
    }
    let env = if plan.existing.is_some() { install_env(&home, "update") } else { env };
    let name = plan.name.clone();
    let ids = plan.ids();
    let package = install::install(&home, &env, plan, &ViaDaemon(Daemon::find(&home)), &out).await?;
    println!("✓ Installed {name} ({})", package.label());
    if let Some(first) = ids.first() {
        if Daemon::find(&home).is_some() {
            println!("Start it with: lx start {first}");
        } else {
            println!("Start LaunchX with `lx up`, then: lx start {first}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The installed apps these names or service ids mean; all of them for none.
fn chosen(installs: &install::Installs, names: &[String]) -> anyhow::Result<Vec<install::Package>> {
    if names.is_empty() {
        return Ok(installs.packages.values().cloned().collect());
    }
    names
        .iter()
        .map(|name| installs.find(name).cloned().ok_or_else(|| anyhow!("Nothing named '{name}' is installed (see lx install)")))
        .collect()
}

async fn update_apps(home: Home, names: Vec<String>, check: bool, force: bool) -> anyhow::Result<ExitCode> {
    let _lock = install::lock(&home)?;
    let packages = chosen(&install::Installs::load(&home), &names)?;
    if packages.is_empty() {
        println!("Nothing is installed with lx install yet.");
        return Ok(ExitCode::SUCCESS);
    }
    let env = install_env(&home, "update");
    let mut failed = false;
    if check {
        for package in packages {
            let (h, e, n) = (home.clone(), env.clone(), package.name.clone());
            match tokio::task::spawn_blocking(move || install::check(&h, &e, &n)).await? {
                Ok(Some(standing)) => match (&standing.available, standing.behind) {
                    (Some(release), _) => println!("{}: {release} available — lx update {}", package.name, package.name),
                    (None, 0) => println!("{}: up to date{}", package.name, standing.version.map(|v| format!(" ({v})")).unwrap_or_default()),
                    (None, behind) => println!("{}: {behind} new commit{} — lx update {}", package.name, if behind == 1 { "" } else { "s" }, package.name),
                },
                Ok(None) => println!("{}: installed from the folder {}; it updates when you change it", package.name, tilde(&package.dir)),
                Err(error) => {
                    failed = true;
                    println!("{}: could not check: {error:#}", package.name);
                }
            }
        }
        return Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS });
    }
    let services = ViaDaemon(Daemon::find(&home));
    let out = terminal_out();
    for package in packages {
        println!("Updating {}…", package.name);
        match install::update(&home, &env, &package, force, &services, &out).await {
            Ok(install::Updated::UpToDate) => println!("✓ {} is up to date ({})", package.name, package.label()),
            Ok(install::Updated::Installed(new)) => println!("✓ Updated {} {} → {}", package.name, package.label(), new.label()),
            Err(error) => {
                failed = true;
                println!("✗ {}: {error:#}", package.name);
            }
        }
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

async fn uninstall_app(home: Home, name: &str, yes: bool) -> anyhow::Result<ExitCode> {
    let _lock = install::lock(&home)?;
    let Some(package) = install::Installs::load(&home).find(name).cloned() else {
        let (registered, _) = manifest::load_registry(&home);
        if registered.iter().any(|s| s.id == name) {
            bail!("{name} was not installed with lx install. To stop LaunchX running it: lx rm {name}");
        }
        bail!("Nothing named '{name}' is installed (see lx install)");
    };
    println!("Uninstall {}", package.name);
    println!("  services  {}", package.services.join(", "));
    if package.cloned {
        println!("  deletes   {}", tilde(&package.dir));
    } else {
        println!("  keeps     {} (LaunchX did not clone it)", tilde(&package.dir));
    }
    for data in &package.data {
        println!("  keeps     {data} (your data)");
    }
    if !confirm("Uninstall?", yes)? {
        println!("Nothing removed.");
        return Ok(ExitCode::SUCCESS);
    }
    let env = install_env(&home, "uninstall");
    install::uninstall(&home, &env, &package, &ViaDaemon(Daemon::find(&home)), &terminal_out()).await?;
    println!("✓ Uninstalled {}", package.name);
    if !package.data.is_empty() {
        println!("Your data is still in {}. Delete it yourself if you no longer want it.", package.data.join(", "));
    }
    Ok(ExitCode::SUCCESS)
}
