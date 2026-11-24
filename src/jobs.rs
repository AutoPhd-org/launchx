//! Installs, updates and uninstalls the launcher's page starts (contract §10.8).
//! Each runs as a job in the background; its output goes to `logs/install.log`
//! and, line by line, to the event stream as `log` events with the id `.install`.

use std::fs;
use std::future::Future;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use crate::install::{self, Installs, Out};
use crate::supervisor::{Event, Launcher, Status};
use crate::util;

/// The id log events of a job carry.
pub const LOG_ID: &str = ".install";

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    /// `install`, `update` or `uninstall`.
    pub action: String,
    /// The app, or for an install, the source until its name is known.
    pub name: String,
    /// `running`, `done` or `failed`.
    pub status: String,
    pub message: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl install::Services for Arc<Launcher> {
    async fn stop(&self, ids: &[String]) -> Vec<String> {
        let mut stopped: Vec<String> = Vec::new();
        for id in ids {
            let running = self.runtime(id).is_some_and(|rt| !matches!(rt.status, Status::Stopped | Status::Failed));
            if running {
                for other in Launcher::stop(self, id).await.unwrap_or_default() {
                    if !stopped.contains(&other) {
                        stopped.push(other);
                    }
                }
            }
        }
        stopped
    }

    async fn reload_and_start(&self, ids: &[String]) {
        self.reload().await;
        for id in ids {
            let _ = self.start(id);
        }
    }
}

fn log_path(launcher: &Launcher) -> std::path::PathBuf {
    launcher.home.logs_dir().join("install.log")
}

fn changed(launcher: &Launcher) {
    let _ = launcher.events.send(Event::Change(String::new()));
}

/// Output of the current job: appended to its log and sent to the page.
fn job_out(launcher: &Arc<Launcher>) -> Out {
    let launcher = launcher.clone();
    Arc::new(move |line: &str| {
        let line = install::plain(line);
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(log_path(&launcher)) {
            let _ = writeln!(file, "{line}");
        }
        let _ = launcher.events.send(Event::Log { id: LOG_ID.into(), line });
    })
}

pub fn log_tail(launcher: &Launcher, lines: usize) -> Vec<String> {
    crate::supervisor::tail(&log_path(launcher), lines)
}

/// Start a job, unless one is running here or on the command line.
pub fn begin<F, Fut>(launcher: &Arc<Launcher>, action: &str, name: &str, work: F) -> Result<Job, String>
where
    F: FnOnce(Arc<Launcher>, Out) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<String>> + Send + 'static,
{
    let busy = "Another install, update or uninstall is running. Try again when it has finished.".to_string();
    let lock = install::try_lock(&launcher.home).map_err(|e| e.to_string())?.ok_or_else(|| busy.clone())?;
    let job = {
        let mut slot = launcher.job.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|job| job.status == "running") {
            return Err(busy);
        }
        let job = Job {
            action: action.into(),
            name: name.into(),
            status: "running".into(),
            message: None,
            started_at: util::now_rfc3339(),
            finished_at: None,
        };
        *slot = Some(job.clone());
        job
    };
    let _ = fs::create_dir_all(launcher.home.logs_dir());
    let _ = fs::write(log_path(launcher), "");
    changed(launcher);
    let out = job_out(launcher);
    out(&format!("[launchx] {action} {name} — {}", job.started_at));
    let launcher = launcher.clone();
    tokio::spawn(async move {
        let _lock = lock;
        let result = work(launcher.clone(), out.clone()).await;
        let (status, message) = match result {
            Ok(message) => ("done", message),
            Err(error) => ("failed", format!("{error:#}")),
        };
        out(&format!("[launchx] {message}"));
        if let Some(job) = launcher.job.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            job.status = status.into();
            job.message = Some(message);
            job.finished_at = Some(util::now_rfc3339());
        }
        changed(&launcher);
    });
    Ok(job)
}

fn env(launcher: &Launcher, action: &str) -> Vec<(String, String)> {
    install::environment(&launcher.home, launcher.env(), action)
}

/// Fetch a source and say what installing it would do, without installing.
pub async fn plan(launcher: &Arc<Launcher>, source: String, name: Option<String>) -> Result<Value, String> {
    let launcher = launcher.clone();
    tokio::task::spawn_blocking(move || {
        let _lock = install::try_lock(&launcher.home)
            .map_err(|e| e.to_string())?
            .ok_or("Another install, update or uninstall is running. Try again when it has finished.")?;
        let quiet: Out = Arc::new(|_: &str| {});
        let plan = install::prepare(&launcher.home, &env(&launcher, "install"), &source, name.as_deref(), &quiet).map_err(|e| format!("{e:#}"))?;
        Ok(plan.to_json())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn start_install(launcher: &Arc<Launcher>, source: String, name: Option<String>) -> Result<Job, String> {
    let label = name.clone().unwrap_or_else(|| source.clone());
    begin(launcher, "install", &label, move |launcher, out| async move {
        let home = launcher.home.clone();
        let plan = {
            let (home, env, out) = (home.clone(), env(&launcher, "install"), out.clone());
            tokio::task::spawn_blocking(move || install::prepare(&home, &env, &source, name.as_deref(), &out)).await??
        };
        let action = if plan.existing.is_some() { "update" } else { "install" };
        let app = plan.name.clone();
        if let Some(job) = launcher.job.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
            job.name = app.clone();
        }
        let package = install::install(&home, &env(&launcher, action), plan, &launcher, &out).await?;
        Ok(format!("Installed {app} ({})", package.label()))
    })
}

pub fn start_update(launcher: &Arc<Launcher>, name: &str, force: bool) -> Result<Job, String> {
    let package = Installs::load(&launcher.home).find(name).cloned().ok_or_else(|| format!("Nothing named '{name}' is installed"))?;
    begin(launcher, "update", &package.name.clone(), move |launcher, out| async move {
        let env = env(&launcher, "update");
        match install::update(&launcher.home, &env, &package, force, &launcher, &out).await? {
            install::Updated::UpToDate => Ok(format!("{} is up to date ({})", package.name, package.label())),
            install::Updated::Installed(new) => Ok(format!("Updated {} {} → {}", package.name, package.label(), new.label())),
        }
    })
}

pub fn start_uninstall(launcher: &Arc<Launcher>, name: &str) -> Result<Job, String> {
    let package = Installs::load(&launcher.home).find(name).cloned().ok_or_else(|| format!("Nothing named '{name}' is installed"))?;
    begin(launcher, "uninstall", &package.name.clone(), move |launcher, out| async move {
        let env = env(&launcher, "uninstall");
        install::uninstall(&launcher.home, &env, &package, &launcher, &out).await?;
        let kept = if package.data.is_empty() { String::new() } else { format!("; your data is still in {}", package.data.join(", ")) };
        Ok(format!("Uninstalled {}{kept}", package.name))
    })
}

/// Look for updates to every cloned app, without pulling. Skipped while a job runs.
pub async fn check_updates(launcher: &Arc<Launcher>) -> Value {
    let worker = launcher.clone();
    let problems = tokio::task::spawn_blocking(move || {
        let Ok(Some(_lock)) = install::try_lock(&worker.home) else { return vec!["An install is running; check again later".to_string()] };
        let env = env(&worker, "update");
        let mut problems = Vec::new();
        for package in Installs::load(&worker.home).packages.values().filter(|p| p.cloned) {
            if let Err(error) = install::check(&worker.home, &env, &package.name) {
                problems.push(format!("{}: {error:#}", package.name));
            }
        }
        problems
    })
    .await
    .unwrap_or_default();
    changed(launcher);
    let installs = Installs::load(&launcher.home);
    json!({ "packages": installs.packages.values().map(install::Package::to_json).collect::<Vec<_>>(), "errors": problems })
}

/// Check for updates a minute after start, then every twelve hours.
pub fn watch_updates(launcher: &Arc<Launcher>) {
    let launcher = launcher.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            if !Installs::load(&launcher.home).packages.is_empty() {
                check_updates(&launcher).await;
            }
            tokio::time::sleep(Duration::from_secs(12 * 3600)).await;
        }
    });
}
