//! `lx`: the LaunchX launcher — daemon, gateway and command line in one executable.

mod check;
mod client;
mod gateway;
mod home;
mod manifest;
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
    /// Start a service on a spare port and test it against its declared level
    Check {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Print the launcher's state document
    State,
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
        if as_json {
            let list: Vec<Value> = services.iter().map(|s| json!({ "id": s.id, "name": s.def.name, "status": "unknown", "error": s.error })).collect();
            println!("{}", serde_json::to_string_pretty(&json!({ "running": false, "services": list, "errors": errors }))?);
            return Ok(ExitCode::SUCCESS);
        }
        println!("LaunchX is not running. Start it with `lx up`.\n");
        let mut rows = vec![vec!["ID".into(), "NAME".into(), "MANIFEST".into()]];
        rows.extend(services.iter().map(|s| vec![s.id.clone(), s.def.name.clone(), s.manifest.display().to_string()]));
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
        let mut rows = vec![vec!["ID".into(), "STATUS".into(), "HEALTH".into(), "VERSION".into(), "PORT".into(), "SUMMARY".into()]];
        for s in &services {
            let summary = match s.get("error").and_then(Value::as_str) {
                Some(error) => error.to_string(),
                None => s.get("summary").or(s.get("detail")).and_then(Value::as_str).unwrap_or("").to_string(),
            };
            rows.push(vec![text(s, "id"), text(s, "status"), text(s, "health"), text(s, "version"), text(s, "port"), summary]);
        }
        table(rows);
    }
    for error in state["errors"].as_array().into_iter().flatten() {
        eprintln!("! {}", error.as_str().unwrap_or(""));
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

async fn open(home: Home, id: Option<String>, print: bool) -> anyhow::Result<ExitCode> {
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
        Some(id) => json!({ "service": id, "next": "/" }),
        None => json!({ "next": "/" }),
    };
    let link = daemon.post("/api/link", body).await?;
    let url = link["url"].as_str().ok_or_else(|| anyhow!("LaunchX returned no link"))?;
    if print {
        println!("{url}");
    } else {
        platform::open_url(url)?;
        println!("Opened {} in your browser.", id.as_deref().unwrap_or("LaunchX"));
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
    home.ensure()?;
    let file = manifest::locate(path)?;
    let services = manifest::parse(&file, &file)?;
    let (existing, _) = manifest::load_registry(&home);
    if let Some(same) = existing.iter().find(|s| s.manifest == file) {
        println!("Already registered: {} ({})", same.entry.display(), file.display());
        return Ok(ExitCode::SUCCESS);
    }
    for service in &services {
        if let Some(other) = existing.iter().find(|s| s.id == service.id) {
            bail!("A service named '{}' is already registered from {}. Remove it first: lx rm {}", service.id, other.manifest.display(), service.id);
        }
    }
    let stem: String = file
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect())
        .unwrap_or_else(|| services[0].id.clone());
    let mut link = home.services_dir().join(format!("{stem}.toml"));
    let mut n = 2;
    while link.symlink_metadata().is_ok() {
        link = home.services_dir().join(format!("{stem}-{n}.toml"));
        n += 1;
    }
    std::os::unix::fs::symlink(&file, &link).with_context(|| format!("linking {}", link.display()))?;
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
    let entry = service.entry.clone();
    let siblings: Vec<&str> = services.iter().filter(|s| s.entry == entry).map(|s| s.id.as_str()).collect();
    if siblings.len() > 1 {
        println!("{} also defines {}; unregistering all of them.", service.manifest.display(), siblings.join(", "));
    }
    let daemon = Daemon::find(&home);
    if let Some(daemon) = &daemon {
        for sibling in &siblings {
            daemon.post(&format!("/api/services/{sibling}/stop"), json!({})).await?;
        }
    }
    if !entry.starts_with(home.services_dir()) {
        bail!("refusing to remove {}, which is outside {}", entry.display(), home.services_dir().display());
    }
    fs::remove_file(&entry).with_context(|| format!("removing {}", entry.display()))?;
    if let Some(daemon) = &daemon {
        daemon.post("/api/reload", json!({})).await?;
    }
    println!("Unregistered {} (the manifest itself is untouched).", siblings.join(", "));
    Ok(ExitCode::SUCCESS)
}
