//! Installing, updating and removing apps from their repositories (contract §4.9, §10.8).
//!
//! The work here is synchronous: fetching with git, running an app's installer,
//! registering its manifest and recording it in `installs.json`. Stopping and
//! starting services around that work is the caller's, through [`Services`],
//! since the command line does it through the daemon's API and the daemon does
//! it directly.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::home::Home;
use crate::manifest::{self, InstallDef, Service};
use crate::platform;

/// Where a line of output goes: the terminal, or the job log.
pub type Out = Arc<dyn Fn(&str) + Send + Sync>;

// --- What is installed ----------------------------------------------------------

/// One installed app, as `installs.json` records it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Package {
    pub name: String,
    /// A git URL, or the absolute path of a folder.
    pub source: String,
    /// The checkout the app was installed from.
    pub dir: PathBuf,
    /// Whether LaunchX cloned `dir`, and so may pull and delete it.
    pub cloned: bool,
    /// The services the checkout's manifest defines.
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub commit: Option<String>,
    /// Where the app keeps the user's data (`[install] data`).
    #[serde(default)]
    pub data: Vec<String>,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// New commits upstream at the last check, for a cloned app.
    #[serde(default)]
    pub behind: Option<u32>,
    #[serde(default)]
    pub checked_at: Option<String>,
}

impl Package {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "source": self.source,
            "dir": self.dir,
            "cloned": self.cloned,
            "services": self.services,
            "commit": self.commit,
            "data": self.data,
            "installed_at": self.installed_at,
            "updated_at": self.updated_at,
            "behind": self.behind,
            "checked_at": self.checked_at,
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Installs {
    #[serde(default)]
    pub packages: BTreeMap<String, Package>,
}

impl Installs {
    pub fn path(home: &Home) -> PathBuf {
        home.root.join("installs.json")
    }

    /// What is installed; empty when nothing is, or the file cannot be read.
    pub fn load(home: &Home) -> Installs {
        fs::read_to_string(Self::path(home)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self, home: &Home) -> anyhow::Result<()> {
        home.ensure()?;
        crate::home::write_private(&Self::path(home), &serde_json::to_vec_pretty(self)?)
    }

    /// The installed app that defines service `id`.
    pub fn owner_of(&self, id: &str) -> Option<&Package> {
        self.packages.values().find(|p| p.services.iter().any(|s| s == id))
    }

    /// An app by its name, or by the id of one of its services.
    pub fn find(&self, name_or_id: &str) -> Option<&Package> {
        self.packages.get(name_or_id).or_else(|| self.owner_of(name_or_id))
    }
}

pub fn sources_dir(home: &Home) -> PathBuf {
    home.root.join("sources")
}

// --- One job at a time ------------------------------------------------------------

/// Held while an install, update or uninstall runs, from the page or the
/// command line. Released when dropped.
pub struct Lock(#[allow(dead_code)] fs::File);

pub fn try_lock(home: &Home) -> anyhow::Result<Option<Lock>> {
    home.ensure()?;
    let file = fs::OpenOptions::new().create(true).truncate(false).write(true).open(home.root.join("install.lock"))?;
    // SAFETY: flock(2) on a descriptor we own; released when the file closes.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    Ok(locked.then_some(Lock(file)))
}

pub fn lock(home: &Home) -> anyhow::Result<Lock> {
    try_lock(home)?.ok_or_else(|| anyhow::anyhow!("Another install, update or uninstall is running. Try again when it has finished."))
}

// --- Sources ----------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Git(String),
    Local(PathBuf),
}

impl Source {
    pub fn parse(text: &str) -> anyhow::Result<Source> {
        let text = text.trim();
        if text.is_empty() {
            bail!("Name a source: a git URL, owner/repo on GitHub, or a folder");
        }
        let path = manifest::expand_tilde(text);
        if path.exists() {
            let path = fs::canonicalize(&path)?;
            if !path.is_dir() {
                bail!("{} is not a folder", path.display());
            }
            return Ok(Source::Local(path));
        }
        if ["https://", "http://", "ssh://", "git://", "file://", "git@"].iter().any(|scheme| text.starts_with(scheme)) {
            return Ok(Source::Git(text.trim_end_matches('/').to_string()));
        }
        let parts: Vec<&str> = text.split('/').collect();
        let plain = |part: &str| !part.is_empty() && !part.starts_with('.') && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        if parts.len() == 2 && parts.iter().all(|part| plain(part)) && !text.starts_with('.') {
            return Ok(Source::Git(format!("https://github.com/{}", text.trim_end_matches(".git"))));
        }
        bail!("{text} is not a folder, a git URL, or owner/repo on GitHub")
    }

    pub fn text(&self) -> String {
        match self {
            Source::Git(url) => url.clone(),
            Source::Local(path) => path.display().to_string(),
        }
    }

    /// A name for the app: the repository's or the folder's, made safe.
    pub fn default_name(&self) -> String {
        let last = match self {
            Source::Git(url) => url.rsplit(['/', ':']).next().unwrap_or("").trim_end_matches(".git").to_string(),
            Source::Local(path) => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        clean_name(&last)
    }
}

pub fn clean_name(text: &str) -> String {
    let mapped: String = text.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let name = mapped.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
    if name.is_empty() { "app".into() } else { name.chars().take(48).collect() }
}

// --- Running commands -------------------------------------------------------------

/// The environment for git and installers: `base` (the user's login
/// environment), the directory of this `lx` first on PATH, and LAUNCHX_HOME.
pub fn environment(home: &Home, base: Vec<(String, String)>, action: &str) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = base.into_iter().filter(|(key, _)| !key.starts_with("LAUNCHX")).collect();
    let mut path = platform::lookup(&env, "PATH").unwrap_or("/usr/bin:/bin").to_string();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        path = format!("{}:{path}", dir.display());
    }
    env.retain(|(key, _)| key != "PATH");
    env.push(("PATH".into(), path));
    let root = std::path::absolute(&home.root).unwrap_or_else(|_| home.root.clone());
    env.push(("LAUNCHX_HOME".into(), root.display().to_string()));
    env.push(("LAUNCHX_INSTALL".into(), action.into()));
    // Never wait for a password nobody can type.
    env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    if platform::lookup(&env, "GIT_SSH_COMMAND").is_none() {
        env.push(("GIT_SSH_COMMAND".into(), "ssh -o BatchMode=yes".into()));
    }
    env
}

/// Run a command in `cwd` with stdin closed, sending each line of its output
/// (stdout and stderr together) to `out`.
pub fn run(args: &[String], cwd: &Path, env: &[(String, String)], out: &Out) -> anyhow::Result<()> {
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exec 2>&1; exec \"$@\"", "sh"])
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not run {}", args.join(" ")))?;
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&bytes);
        // A progress bar rewrites its line with \r; keep what it ended on.
        let line = text.trim_end_matches(['\n', '\r']).rsplit('\r').next().unwrap_or("");
        out(line);
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("`{}` failed ({})", args.join(" "), crate::supervisor::describe(status));
    }
    Ok(())
}

/// A git command's output, quietly.
fn git(dir: &Path, env: &[(String, String)], args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .output()
        .context("could not run git; is it installed?")?;
    if !output.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_loud(dir: &Path, env: &[(String, String)], args: &[&str], out: &Out) -> anyhow::Result<()> {
    out(&format!("$ git {}", args.join(" ")));
    run(&std::iter::once("git").chain(args.iter().copied()).map(String::from).collect::<Vec<_>>(), dir, env, out)
}

fn head(dir: &Path, env: &[(String, String)]) -> Option<String> {
    git(dir, env, &["rev-parse", "HEAD"]).ok().filter(|commit| !commit.is_empty())
}

fn same_remote(a: &str, b: &str) -> bool {
    let norm = |url: &str| url.trim_end_matches('/').trim_end_matches(".git").to_ascii_lowercase();
    norm(a) == norm(b)
}

// --- Plans ------------------------------------------------------------------------

/// What installing would do, after the source has been fetched.
#[derive(Clone, Debug)]
pub struct Plan {
    pub name: String,
    pub source: Source,
    pub dir: PathBuf,
    pub manifest: PathBuf,
    pub services: Vec<Service>,
    pub install: InstallDef,
    /// The app as installed now, when this is an update.
    pub existing: Option<Package>,
}

impl Plan {
    pub fn ids(&self) -> Vec<String> {
        self.services.iter().map(|s| s.id.clone()).collect()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "source": self.source.text(),
            "dir": self.dir,
            "cloned": matches!(self.source, Source::Git(_)),
            "services": self.services.iter().map(|s| json!({
                "id": s.id, "name": s.def.name, "description": s.def.description, "level": s.def.level,
            })).collect::<Vec<_>>(),
            "run": self.install.run,
            "uninstall": self.install.uninstall,
            "needs": self.install.needs,
            "data": self.install.data,
            "installed": self.existing.is_some(),
        })
    }

    /// The plan, as the command line shows it before asking.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![format!("{} {}", if self.existing.is_some() { "Reinstall" } else { "Install" }, self.name)];
        lines.push(format!("  from      {}", self.source.text()));
        if matches!(self.source, Source::Git(_)) {
            lines.push(format!("  checkout  {}", self.dir.display()));
        }
        for service in &self.services {
            lines.push(format!("  service   {} ({}), level {}", service.id, service.def.name, service.def.level));
        }
        if self.install.run.is_empty() {
            lines.push("  installer none: LaunchX registers the manifest and runs the app from the checkout".into());
        } else {
            lines.push(format!("  runs      {}", self.install.run.join(" ")));
        }
        if !self.install.needs.is_empty() {
            lines.push(format!("  needs     {}", self.install.needs.join(", ")));
        }
        for data in &self.install.data {
            lines.push(format!("  data      {data}"));
        }
        lines
    }
}

/// Read what a checkout would install.
fn plan_from(home: &Home, name: &str, source: Source, dir: &Path) -> anyhow::Result<Plan> {
    let file = dir.join("launchx.toml");
    if !file.is_file() {
        bail!("{} has no launchx.toml, so it is not a LaunchX app", dir.display());
    }
    let (services, install) = manifest::parse_with_install(&file, &file)?;
    let existing = Installs::load(home).packages.get(name).cloned();
    Ok(Plan { name: name.into(), source, dir: dir.to_path_buf(), manifest: file, services, install: install.unwrap_or_default(), existing })
}

/// Fetch a source and read what installing it would do. A git source is
/// cloned into `sources/<name>`, or pulled when it already is.
pub fn prepare(home: &Home, env: &[(String, String)], text: &str, name: Option<&str>, out: &Out) -> anyhow::Result<Plan> {
    let source = Source::parse(text)?;
    let installs = Installs::load(home);
    // Installing what is installed updates it, under the name it has.
    let known = installs.packages.values().find(|p| match &source {
        Source::Git(url) => p.cloned && same_remote(&p.source, url),
        Source::Local(path) => !p.cloned && p.dir == *path,
    });
    let name = match (name, known) {
        (Some(given), _) => clean_name(given),
        (None, Some(package)) => package.name.clone(),
        (None, None) => source.default_name(),
    };
    if let Some(other) = installs.packages.get(&name) {
        if known.map(|p| &p.name) != Some(&other.name) {
            bail!("An app named '{name}' is already installed from {}. Choose another name with --name.", other.source);
        }
    }
    let dir = match &source {
        Source::Local(path) => path.clone(),
        Source::Git(url) => {
            let dir = sources_dir(home).join(&name);
            if dir.join(".git").exists() {
                let origin = git(&dir, env, &["remote", "get-url", "origin"]).unwrap_or_default();
                if !same_remote(&origin, url) {
                    bail!("{} is a checkout of {origin}, not {url}. Choose another name with --name.", dir.display());
                }
                git_loud(&dir, env, &["pull", "--ff-only"], out)?;
            } else {
                if dir.exists() {
                    bail!("{} exists and is not a git checkout; move it away first", dir.display());
                }
                fs::create_dir_all(sources_dir(home))?;
                git_loud(&sources_dir(home), env, &["clone", url, &name], out)?;
            }
            dir
        }
    };
    plan_from(home, &name, source, &dir)
}

// --- Install, update, uninstall: the file work -------------------------------------

/// Run the installer, register the app and record it.
pub fn install_files(home: &Home, env: &[(String, String)], plan: &Plan, out: &Out) -> anyhow::Result<Package> {
    let path = platform::lookup(env, "PATH").unwrap_or("/usr/bin:/bin").to_string();
    let missing: Vec<&String> = plan.install.needs.iter().filter(|need| platform::which(need, &path).is_none()).collect();
    if !missing.is_empty() {
        bail!("{} needs {} on PATH; install that first", plan.name, missing.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", "));
    }
    let ids = plan.ids();
    let (registered, _) = manifest::load_registry(home);
    if plan.install.run.is_empty() {
        // The checkout's manifest is what runs, so a service of the same id
        // registered from elsewhere would run instead of it.
        for service in registered.iter().filter(|s| ids.contains(&s.id)) {
            if service.manifest != plan.manifest {
                bail!("'{}' is already registered from {}. Remove it first: lx rm {}", service.id, service.manifest.display(), service.id);
            }
        }
    } else {
        out(&format!("$ {}", plan.install.run.join(" ")));
        run(&plan.install.run, &plan.dir, env, out)?;
    }
    let (registered, _) = manifest::load_registry(home);
    let present: Vec<&String> = ids.iter().filter(|id| registered.iter().any(|s| &&s.id == id)).collect();
    if present.is_empty() {
        manifest::register(home, &plan.manifest)?;
        out(&format!("Registered {} from {}", ids.join(", "), plan.manifest.display()));
    } else if present.len() < ids.len() {
        out(&format!("The installer registered {} but not the rest of {}", present.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), ids.join(", ")));
    }
    let now = crate::util::now_rfc3339();
    let package = Package {
        name: plan.name.clone(),
        source: plan.source.text(),
        dir: plan.dir.clone(),
        cloned: matches!(plan.source, Source::Git(_)),
        services: ids,
        commit: head(&plan.dir, env),
        data: plan.install.data.clone(),
        installed_at: plan.existing.as_ref().map(|p| p.installed_at.clone()).unwrap_or_else(|| now.clone()),
        updated_at: plan.existing.as_ref().map(|_| now.clone()),
        behind: plan.existing.as_ref().and(Some(0)),
        checked_at: plan.existing.as_ref().and_then(|p| p.checked_at.clone()),
    };
    let mut installs = Installs::load(home);
    installs.packages.insert(package.name.clone(), package.clone());
    installs.save(home)?;
    Ok(package)
}

/// Pull a cloned app. Returns the plan to install from, or None when there is
/// nothing new and `force` is not set. An app from a folder always reinstalls.
pub fn prepare_update(home: &Home, env: &[(String, String)], package: &Package, force: bool, out: &Out) -> anyhow::Result<Option<Plan>> {
    let source = if package.cloned { Source::Git(package.source.clone()) } else { Source::Local(package.dir.clone()) };
    if !package.dir.is_dir() {
        bail!("{} is gone. Install it again with: lx install {}", package.dir.display(), package.source);
    }
    if package.cloned {
        let before = head(&package.dir, env);
        git_loud(&package.dir, env, &["pull", "--ff-only"], out)?;
        let after = head(&package.dir, env);
        if before == after && package.commit == after && !force {
            let mut installs = Installs::load(home);
            if let Some(record) = installs.packages.get_mut(&package.name) {
                record.behind = Some(0);
                record.checked_at = Some(crate::util::now_rfc3339());
                installs.save(home)?;
            }
            return Ok(None);
        }
    }
    plan_from(home, &package.name, source, &package.dir).map(Some)
}

/// Run the app's uninstall command, unregister it, delete the checkout if
/// LaunchX cloned it, and forget it. The app's data stays.
pub fn uninstall_files(home: &Home, env: &[(String, String)], package: &Package, out: &Out) -> anyhow::Result<()> {
    let uninstall = manifest::parse_with_install(&package.dir.join("launchx.toml"), &package.dir.join("launchx.toml"))
        .ok()
        .and_then(|(_, install)| install)
        .map(|install| install.uninstall)
        .unwrap_or_default();
    if !uninstall.is_empty() {
        out(&format!("$ {}", uninstall.join(" ")));
        run(&uninstall, &package.dir, env, out)?;
    }
    let (registered, _) = manifest::load_registry(home);
    for id in &package.services {
        if registered.iter().any(|s| &s.id == id) {
            match manifest::unregister(home, id) {
                Ok(removed) => out(&format!("Unregistered {}", removed.join(", "))),
                // A sibling's removal already took it.
                Err(_) if !manifest::load_registry(home).0.iter().any(|s| &s.id == id) => {}
                Err(error) => return Err(error),
            }
        }
    }
    if package.cloned && package.dir.starts_with(sources_dir(home)) && package.dir.is_dir() {
        fs::remove_dir_all(&package.dir).with_context(|| format!("removing {}", package.dir.display()))?;
        out(&format!("Deleted the checkout {}", package.dir.display()));
    }
    let mut installs = Installs::load(home);
    installs.packages.remove(&package.name);
    installs.save(home)?;
    Ok(())
}

/// Fetch a cloned app and count the commits it is behind. Records the answer.
pub fn check(home: &Home, env: &[(String, String)], name: &str) -> anyhow::Result<Option<u32>> {
    let Some(package) = Installs::load(home).packages.get(name).cloned() else { bail!("Nothing named '{name}' is installed") };
    if !package.cloned {
        return Ok(None);
    }
    git(&package.dir, env, &["fetch", "--quiet"])?;
    let behind: u32 = git(&package.dir, env, &["rev-list", "--count", "HEAD..@{upstream}"])?.parse().unwrap_or(0);
    let mut installs = Installs::load(home);
    if let Some(record) = installs.packages.get_mut(name) {
        record.behind = Some(behind);
        record.checked_at = Some(crate::util::now_rfc3339());
        installs.save(home)?;
    }
    Ok(Some(behind))
}

// --- Around the file work: stopping and starting services ---------------------------

/// How the caller stops and starts services: through the daemon's API, or
/// the daemon itself. Each is a no-op when no launcher runs.
pub trait Services: Sync {
    /// Stop these services and what requires them; return what was running.
    fn stop(&self, ids: &[String]) -> impl Future<Output = Vec<String>> + Send;
    /// Re-read the registered manifests, then start these services.
    fn reload_and_start(&self, ids: &[String]) -> impl Future<Output = ()> + Send;
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(work).await.map_err(|error| anyhow::anyhow!("install task failed: {error}"))?
}

/// Install from a plan: stop the app's running services, run the file work,
/// then start again what was stopped.
pub async fn install<S: Services>(home: &Home, env: &[(String, String)], plan: Plan, services: &S, out: &Out) -> anyhow::Result<Package> {
    let stopped = services.stop(&plan.ids()).await;
    if !stopped.is_empty() {
        out(&format!("Stopped {} while installing", stopped.join(", ")));
    }
    let (home2, env2, out2) = (home.clone(), env.to_vec(), out.clone());
    let result = blocking(move || install_files(&home2, &env2, &plan, &out2)).await;
    services.reload_and_start(&stopped).await;
    if !stopped.is_empty() {
        out(&format!("Started {} again", stopped.join(", ")));
    }
    result
}

/// What one update did.
pub enum Updated {
    UpToDate,
    Installed(Box<Package>),
}

pub async fn update<S: Services>(home: &Home, env: &[(String, String)], package: &Package, force: bool, services: &S, out: &Out) -> anyhow::Result<Updated> {
    let (home2, env2, out2, package2) = (home.clone(), env.to_vec(), out.clone(), package.clone());
    match blocking(move || prepare_update(&home2, &env2, &package2, force, &out2)).await? {
        None => Ok(Updated::UpToDate),
        Some(plan) => install(home, env, plan, services, out).await.map(|package| Updated::Installed(Box::new(package))),
    }
}

pub async fn uninstall<S: Services>(home: &Home, env: &[(String, String)], package: &Package, services: &S, out: &Out) -> anyhow::Result<()> {
    let stopped = services.stop(&package.services).await;
    if !stopped.is_empty() {
        out(&format!("Stopped {}", stopped.join(", ")));
    }
    let (home2, env2, out2, package2) = (home.clone(), env.to_vec(), out.clone(), package.clone());
    let result = blocking(move || uninstall_files(&home2, &env2, &package2, &out2)).await;
    // Whatever was stopped and is still registered (a dependent) comes back.
    services.reload_and_start(&stopped.into_iter().filter(|id| !package.services.contains(id)).collect::<Vec<_>>()).await;
    result
}

/// Text without terminal colour codes, for the page's log.
pub fn plain(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        text.push(c);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_and_names() {
        assert_eq!(Source::parse("AutoPhd-org/launchx").unwrap(), Source::Git("https://github.com/AutoPhd-org/launchx".into()));
        assert_eq!(Source::parse("https://github.com/a/b.git/").unwrap(), Source::Git("https://github.com/a/b.git".into()));
        assert_eq!(Source::parse("git@github.com:AutoPhd-org/your-paper-reading-agent.git").unwrap().default_name(), "your-paper-reading-agent");
        assert_eq!(Source::parse("https://github.com/AutoPhd-org/Schedule_Everything").unwrap().default_name(), "schedule-everything");
        assert!(Source::parse("not a source").is_err());
        assert!(Source::parse("./missing/dir").is_err());
        let here = std::env::temp_dir();
        assert!(matches!(Source::parse(&here.display().to_string()).unwrap(), Source::Local(_)));
        assert_eq!(clean_name("--Odd  Name!!"), "odd-name");
        assert!(same_remote("https://github.com/A/b.git", "https://github.com/a/b/"));
    }

    #[test]
    fn colour_codes_are_stripped() {
        assert_eq!(plain("\u{1b}[0;32m[SUCCESS]\u{1b}[0m done"), "[SUCCESS] done");
    }
}
