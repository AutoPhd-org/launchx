//! `$LAUNCHX_HOME` (default `~/.launchx`): settings, registered manifests,
//! run files, logs, and the assigned ports and tokens.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::util;

pub const DEFAULT_GATEWAY_PORT: u16 = 7600;

#[derive(Clone, Debug)]
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    pub fn from_env() -> Home {
        let root = match std::env::var_os("LAUNCHX_HOME") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => home_dir().join(".launchx"),
        };
        Home { root }
    }

    pub fn ensure(&self) -> anyhow::Result<()> {
        for dir in [self.root.clone(), self.services_dir(), self.run_dir(), self.logs_dir()] {
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    pub fn services_dir(&self) -> PathBuf {
        self.root.join("services")
    }
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn run_path(&self, id: &str) -> PathBuf {
        self.run_dir().join(format!("{id}.json"))
    }
    pub fn log_path(&self, id: &str) -> PathBuf {
        self.logs_dir().join(format!("{id}.log"))
    }
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn lock_path(&self) -> PathBuf {
        self.root.join("daemon.lock")
    }
    /// Where LaunchX keeps the skills it publishes to agents (contract §10.7).
    pub fn agent_skills_dir(&self) -> PathBuf {
        let root = std::path::absolute(&self.root).unwrap_or_else(|_| self.root.clone());
        root.join("agents").join("skills")
    }

    pub fn config(&self) -> anyhow::Result<Config> {
        match fs::read_to_string(self.config_path()) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", self.config_path().display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn load_state(&self) -> anyhow::Result<Persisted> {
        let mut state: Persisted = match fs::read_to_string(self.state_path()) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {}", self.state_path().display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Persisted::default(),
            Err(err) => return Err(err.into()),
        };
        if state.secret.is_empty() {
            state.secret = util::token();
        }
        if state.launcher_token.is_empty() {
            state.launcher_token = util::token();
        }
        Ok(state)
    }

    pub fn save_state(&self, state: &Persisted) -> anyhow::Result<()> {
        write_private(&self.state_path(), &serde_json::to_vec_pretty(state)?)
    }

    pub fn write_run(&self, run: &RunFile) -> anyhow::Result<()> {
        write_private(&self.run_path(&run.id), &serde_json::to_vec_pretty(run)?)
    }

    pub fn remove_run(&self, id: &str) {
        let _ = fs::remove_file(self.run_path(id));
    }

    /// The run file of a live process, or None.
    pub fn read_run(&self, id: &str) -> Option<RunFile> {
        let run: RunFile = serde_json::from_str(&fs::read_to_string(self.run_path(id)).ok()?).ok()?;
        util::pid_alive(run.pid).then_some(run)
    }
}

/// Write a file only the owner can read, atomically.
pub fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Port of the gateway; every browser URL uses it.
    pub gateway_port: u16,
    /// Unit of the restart back-off (1, 2, 4 … 60 units).
    pub backoff_unit_ms: u64,
    /// Agents' skill folders that LaunchX publishes skills into (contract §10.7).
    /// A folder that does not exist is skipped.
    pub agent_skill_dirs: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            gateway_port: DEFAULT_GATEWAY_PORT,
            backoff_unit_ms: 1000,
            agent_skill_dirs: vec!["~/.claude/skills".into(), "~/.agents/skills".into()],
        }
    }
}

impl Config {
    pub fn gateway_host(&self, name: &str) -> String {
        if self.gateway_port == 80 { name.to_string() } else { format!("{name}:{}", self.gateway_port) }
    }
    pub fn launcher_url(&self) -> String {
        format!("http://{}/", self.gateway_host("launchx.localhost"))
    }
    pub fn service_url(&self, id: &str) -> String {
        format!("http://{}/", self.gateway_host(&format!("{id}.localhost")))
    }
    pub fn agent_skill_dirs(&self) -> Vec<PathBuf> {
        self.agent_skill_dirs.iter().map(|dir| crate::manifest::expand_tilde(dir)).collect()
    }
}

/// What the launcher remembers between runs. Mode 0600.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Persisted {
    /// Signs browser sessions.
    #[serde(default)]
    pub secret: String,
    /// Bearer token of the launcher's own API.
    #[serde(default)]
    pub launcher_token: String,
    #[serde(default)]
    pub services: BTreeMap<String, Assigned>,
    /// Services the user started and has not stopped; started again with the launcher.
    #[serde(default)]
    pub running: std::collections::BTreeSet<String>,
    /// Services the user made discoverable by agents (contract §10.7). Off by default.
    #[serde(default)]
    pub agents: std::collections::BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assigned {
    pub port: u16,
    pub token: String,
}

/// `run/<id>.json`: how programs find a running service (contract §10.2).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunFile {
    pub id: String,
    pub pid: u32,
    pub url: String,
    pub token: String,
    #[serde(default)]
    pub ui: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    pub started_at: String,
}
