//! `launchx.toml` manifests (contract §4) and the registry of them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::home::Home;

pub const CONTRACT_VERSION: &str = "0.1";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    launchx: String,
    #[serde(default)]
    service: BTreeMap<String, ServiceDef>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServiceDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub level: u8,
    pub run: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub cli: Option<String>,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default)]
    pub restart: Restart,
    #[serde(default = "default_start_timeout")]
    pub start_timeout: u64,
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout: u64,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub http: Option<HttpDef>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HttpDef {
    #[serde(default = "default_health")]
    pub health: String,
    #[serde(default)]
    pub ui: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub events: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
pub enum Restart {
    #[serde(rename = "no")]
    No,
    #[default]
    #[serde(rename = "on-failure")]
    OnFailure,
    #[serde(rename = "always")]
    Always,
}

fn default_start_timeout() -> u64 {
    30
}
fn default_stop_timeout() -> u64 {
    10
}
fn default_health() -> String {
    "/api/health".into()
}

/// One service as registered: its definition and where it came from.
#[derive(Clone, Debug)]
pub struct Service {
    pub id: String,
    pub def: ServiceDef,
    /// Directory of the real manifest file; relative paths resolve against it.
    pub dir: PathBuf,
    /// The manifest file (after following the registration link).
    pub manifest: PathBuf,
    /// The registration entry in `$LAUNCHX_HOME/services`.
    pub entry: PathBuf,
    /// Why this service cannot start, if it cannot.
    pub error: Option<String>,
}

impl Service {
    pub fn cwd(&self) -> PathBuf {
        match &self.def.cwd {
            Some(cwd) => self.dir.join(expand_tilde(cwd)),
            None => self.dir.clone(),
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 31
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => crate::home::home_dir().join(rest),
        None => PathBuf::from(path),
    }
}

/// Resolve a path given to `lx add`: a manifest file, or a directory holding `launchx.toml`.
pub fn locate(path: &Path) -> anyhow::Result<PathBuf> {
    let path = fs::canonicalize(path).with_context(|| format!("{} does not exist", path.display()))?;
    let file = if path.is_dir() { path.join("launchx.toml") } else { path };
    if !file.is_file() {
        bail!("No launchx.toml in {}", file.parent().unwrap_or(&file).display());
    }
    Ok(file)
}

/// Parse and check one manifest file.
pub fn parse(file: &Path, entry: &Path) -> anyhow::Result<Vec<Service>> {
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let manifest: ManifestFile = toml::from_str(&text).map_err(|err| anyhow::anyhow!("{}: {}", file.display(), err.message()))?;
    if manifest.launchx != CONTRACT_VERSION {
        bail!("{}: launchx = \"{}\" is not supported (expected \"{CONTRACT_VERSION}\")", file.display(), manifest.launchx);
    }
    if manifest.service.is_empty() {
        bail!("{}: defines no [service.<id>] table", file.display());
    }
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut services = Vec::new();
    for (id, def) in manifest.service {
        let problem = |message: String| anyhow::anyhow!("{}: service {id}: {message}", file.display());
        if !valid_id(&id) {
            return Err(problem("ids are lowercase letters, digits and '-', start with a letter, at most 31 characters".into()));
        }
        if id == "launchx" {
            return Err(problem("the id 'launchx' is reserved".into()));
        }
        if def.run.is_empty() || def.run[0].trim().is_empty() {
            return Err(problem("run must name a command".into()));
        }
        if def.level > 2 {
            return Err(problem("level must be 0, 1 or 2".into()));
        }
        if def.start_timeout == 0 || def.stop_timeout == 0 {
            return Err(problem("timeouts must be at least 1 second".into()));
        }
        if let Some(http) = &def.http {
            for path in [Some(&http.health), http.ui.as_ref(), http.state.as_ref(), http.events.as_ref()].into_iter().flatten() {
                if !path.starts_with('/') || path.starts_with("/.launchx") {
                    return Err(problem(format!("http path {path:?} must start with '/' and not use /.launchx")));
                }
            }
        }
        if def.level == 2 && def.cli.is_none() {
            return Err(problem("level 2 needs cli".into()));
        }
        if def.level == 2 && def.http.as_ref().is_some_and(|http| http.state.is_none()) {
            return Err(problem("level 2 with an HTTP interface needs http.state".into()));
        }
        services.push(Service { id, def, dir: dir.clone(), manifest: file.to_path_buf(), entry: entry.to_path_buf(), error: None });
    }
    Ok(services)
}

/// Everything registered in `$LAUNCHX_HOME/services`. Manifest-level problems
/// come back as messages; dependency problems mark the affected services.
pub fn load_registry(home: &Home) -> (Vec<Service>, Vec<String>) {
    let mut services: Vec<Service> = Vec::new();
    let mut errors = Vec::new();
    let mut entries: Vec<PathBuf> = match fs::read_dir(home.services_dir()) {
        Ok(read) => read.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    for entry in entries {
        if entry.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let file = match fs::canonicalize(&entry) {
            Ok(file) => file,
            Err(_) => {
                errors.push(format!("{} points to a manifest that no longer exists", entry.display()));
                continue;
            }
        };
        match parse(&file, &entry) {
            Ok(found) => {
                for service in found {
                    if let Some(other) = services.iter().find(|s| s.id == service.id) {
                        errors.push(format!(
                            "service {} is defined in both {} and {}; the second is ignored",
                            service.id,
                            other.manifest.display(),
                            service.manifest.display()
                        ));
                    } else {
                        services.push(service);
                    }
                }
            }
            Err(err) => errors.push(format!("{err:#}")),
        }
    }
    check_dependencies(&mut services);
    (services, errors)
}

/// Mark services that require unknown services or sit on a cycle.
pub fn check_dependencies(services: &mut [Service]) {
    let ids: BTreeSet<String> = services.iter().map(|s| s.id.clone()).collect();
    let requires: BTreeMap<String, Vec<String>> = services.iter().map(|s| (s.id.clone(), s.def.requires.clone())).collect();
    for service in services.iter_mut() {
        if let Some(missing) = service.def.requires.iter().find(|r| !ids.contains(*r)) {
            service.error = Some(format!("requires unknown service '{missing}'"));
        } else if let Some(cycle) = find_cycle(&service.id, &requires) {
            service.error = Some(format!("dependency cycle: {}", cycle.join(" → ")));
        }
    }
    // A service whose dependency cannot start cannot start either.
    loop {
        let broken: BTreeMap<String, String> = services
            .iter()
            .filter(|s| s.error.is_some())
            .map(|s| (s.id.clone(), s.error.clone().unwrap_or_default()))
            .collect();
        let mut changed = false;
        for service in services.iter_mut().filter(|s| s.error.is_none()) {
            if let Some(dep) = service.def.requires.iter().find(|r| broken.contains_key(*r)) {
                service.error = Some(format!("requires '{dep}', which cannot start"));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn find_cycle(start: &str, requires: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    fn visit(node: &str, start: &str, requires: &BTreeMap<String, Vec<String>>, path: &mut Vec<String>, seen: &mut BTreeSet<String>) -> bool {
        for next in requires.get(node).into_iter().flatten() {
            if next == start {
                path.push(next.clone());
                return true;
            }
            if seen.insert(next.clone()) {
                path.push(next.clone());
                if visit(next, start, requires, path, seen) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }
    let mut path = vec![start.to_string()];
    visit(start, start, requires, &mut path, &mut BTreeSet::new()).then_some(path)
}

/// `id` and everything it requires, dependencies first.
pub fn with_dependencies(id: &str, services: &BTreeMap<String, ServiceDef>) -> Vec<String> {
    fn visit(id: &str, services: &BTreeMap<String, ServiceDef>, order: &mut Vec<String>) {
        if order.iter().any(|seen| seen == id) {
            return;
        }
        for dep in services.get(id).map(|d| d.requires.clone()).unwrap_or_default() {
            visit(&dep, services, order);
        }
        if !order.iter().any(|seen| seen == id) {
            order.push(id.to_string());
        }
    }
    let mut order = Vec::new();
    visit(id, services, &mut order);
    order
}

/// Services that require `id`, directly or not, the most dependent first.
pub fn dependents(id: &str, services: &BTreeMap<String, ServiceDef>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut frontier = vec![id.to_string()];
    while let Some(current) = frontier.pop() {
        for (other, def) in services {
            if def.requires.contains(&current) && !found.contains(other) && other != id {
                found.push(other.clone());
                frontier.push(other.clone());
            }
        }
    }
    // Order so that a service comes before anything it requires.
    let mut ordered: Vec<String> = Vec::new();
    while !found.is_empty() {
        let next = found
            .iter()
            .position(|candidate| !found.iter().any(|other| services.get(other).is_some_and(|d| d.requires.contains(candidate))))
            .unwrap_or(0);
        ordered.push(found.remove(next));
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, text: &str) -> PathBuf {
        let file = dir.join("launchx.toml");
        fs::write(&file, text).unwrap();
        file
    }

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lx-manifest-{}-{}", std::process::id(), crate::util::token()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_defaults_and_rejects_mistakes() {
        let dir = temp();
        let file = write(&dir, "launchx = \"0.1\"\n[service.a]\nname = \"A\"\nrun = [\"a\"]\n[service.a.http]\nui = \"/\"\n");
        let services = parse(&file, &file).unwrap();
        assert_eq!(services[0].def.restart, Restart::OnFailure);
        assert_eq!(services[0].def.http.as_ref().unwrap().health, "/api/health");
        assert_eq!(services[0].def.start_timeout, 30);
        for bad in [
            "launchx = \"0.2\"\n[service.a]\nname=\"A\"\nrun=[\"a\"]\n",
            "launchx = \"0.1\"\n[service.A]\nname=\"A\"\nrun=[\"a\"]\n",
            "launchx = \"0.1\"\n[service.launchx]\nname=\"A\"\nrun=[\"a\"]\n",
            "launchx = \"0.1\"\n[service.a]\nname=\"A\"\nrun=[]\n",
            "launchx = \"0.1\"\n[service.a]\nname=\"A\"\nrun=[\"a\"]\ntypo=1\n",
            "launchx = \"0.1\"\n[service.a]\nname=\"A\"\nrun=[\"a\"]\nlevel=2\n",
            "launchx = \"0.1\"\n[service.a]\nname=\"A\"\nrun=[\"a\"]\n[service.a.http]\nui=\"/.launchx/x\"\n",
        ] {
            let file = write(&dir, bad);
            assert!(parse(&file, &file).is_err(), "accepted: {bad}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    fn def(requires: &[&str]) -> ServiceDef {
        ServiceDef {
            name: "x".into(),
            description: None,
            level: 0,
            run: vec!["x".into()],
            cwd: None,
            env: BTreeMap::new(),
            requires: requires.iter().map(|s| s.to_string()).collect(),
            needs: vec![],
            cli: None,
            autostart: false,
            restart: Restart::OnFailure,
            start_timeout: 30,
            stop_timeout: 10,
            homepage: None,
            icon: None,
            http: None,
        }
    }

    fn service(id: &str, requires: &[&str]) -> Service {
        Service { id: id.into(), def: def(requires), dir: PathBuf::new(), manifest: PathBuf::new(), entry: PathBuf::new(), error: None }
    }

    #[test]
    fn dependency_order_cycles_and_unknown_ids() {
        let defs: BTreeMap<String, ServiceDef> =
            [("a", def(&[])), ("b", def(&["a"])), ("c", def(&["b", "a"])), ("d", def(&[]))].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert_eq!(with_dependencies("c", &defs), ["a", "b", "c"]);
        assert_eq!(dependents("a", &defs), ["c", "b"]);
        assert!(dependents("d", &defs).is_empty());

        let mut services = vec![service("a", &["b"]), service("b", &["a"]), service("c", &["zzz"]), service("d", &["c"]), service("e", &[])];
        check_dependencies(&mut services);
        assert!(services[0].error.as_deref().unwrap().contains("cycle"));
        assert!(services[2].error.as_deref().unwrap().contains("unknown service 'zzz'"));
        assert!(services[3].error.as_deref().unwrap().contains("requires 'c'"));
        assert!(services[4].error.is_none());
    }
}
