//! Agent discovery (contract §10.7): which services LLM agents are told about,
//! and the skills LaunchX publishes into agents' skill folders for them.
//!
//! The switch is per service and off by default. It decides what agents are
//! told, not what they can reach: an agent running as the user can still read
//! run files. LaunchX's own `launchx` skill asks agents to leave hidden apps alone.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::home::{Config, Home};
use crate::manifest::Service;

/// The skill LaunchX writes itself. No service skill may use the name.
pub const OWN_SKILL: &str = "launchx";

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The folder holding SKILL.md.
    pub dir: PathBuf,
}

/// What a service offers agents (manifest §4.7), as found on disk.
#[derive(Clone, Debug, Default)]
pub struct Offer {
    pub skills: Vec<Skill>,
    pub docs: Option<PathBuf>,
    pub problems: Vec<String>,
}

pub fn offer(service: &Service) -> Offer {
    let mut offer = Offer::default();
    let Some(agents) = &service.def.agents else { return offer };
    if let Some(docs) = &agents.docs {
        let path = service.path(docs);
        if path.is_file() {
            offer.docs = Some(path);
        } else {
            offer.problems.push(format!("{}: agents.docs {} does not exist", service.id, path.display()));
        }
    }
    if let Some(skills) = &agents.skills {
        let root = service.path(skills);
        let dirs: Vec<PathBuf> = if root.join("SKILL.md").is_file() {
            vec![root]
        } else {
            match fs::read_dir(&root) {
                Ok(read) => {
                    let mut dirs: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| p.join("SKILL.md").is_file()).collect();
                    dirs.sort();
                    dirs
                }
                Err(_) => {
                    offer.problems.push(format!("{}: agents.skills {} does not exist", service.id, root.display()));
                    Vec::new()
                }
            }
        };
        for dir in dirs {
            match read_skill(&dir) {
                Ok(skill) => offer.skills.push(skill),
                Err(problem) => offer.problems.push(format!("{}: {problem}", service.id)),
            }
        }
    }
    offer
}

fn read_skill(dir: &Path) -> Result<Skill, String> {
    let file = dir.join("SKILL.md");
    let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let fields = frontmatter(&text);
    let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let name = fields.get("name").cloned().unwrap_or(folder);
    if !valid_skill_name(&name) {
        return Err(format!("{}: skill name {name:?} must be lowercase letters, digits and '-'", file.display()));
    }
    if name == OWN_SKILL {
        return Err(format!("{}: the skill name '{OWN_SKILL}' is reserved", file.display()));
    }
    let description = fields.get("description").cloned().unwrap_or_default();
    if description.is_empty() {
        return Err(format!("{}: no description in its front matter", file.display()));
    }
    Ok(Skill { name, description, dir: dir.to_path_buf() })
}

pub fn valid_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-'
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Top-level `key: value` pairs of a SKILL.md front matter. Handles plain,
/// quoted and folded (`>`, `|`) values, which is all a name and a description need.
pub fn frontmatter(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return fields;
    }
    let mut current: Option<String> = None;
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        if line.starts_with([' ', '\t']) {
            if let Some(key) = &current {
                let value: &mut String = fields.entry(key.clone()).or_default();
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(line.trim());
            }
            continue;
        }
        current = None;
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim().to_string();
        let value = value.trim();
        let value = if value.starts_with(['>', '|']) {
            String::new()
        } else if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            serde_json::from_str(value).unwrap_or_else(|_| value[1..value.len() - 1].to_string())
        } else if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
            value[1..value.len() - 1].replace("''", "'")
        } else {
            value.to_string()
        };
        fields.insert(key.clone(), value);
        current = Some(key);
    }
    fields
}

/// The service's command-line tool as an agent should run it.
fn cli(service: &Service) -> Option<String> {
    let cli = service.def.cli.as_ref()?;
    Some(if cli.contains('/') { service.cwd().join(cli).display().to_string() } else { cli.clone() })
}

fn ui(service: &Service, config: &Config) -> Option<String> {
    let path = service.def.http.as_ref()?.ui.as_ref()?;
    Some(format!("{}{}", config.service_url(&service.id), path.trim_start_matches('/')))
}

/// One entry of `lx agents --json` / `GET /api/agents`.
pub fn describe(service: &Service, offer: &Offer, home: &Home, config: &Config, status: Option<String>) -> Value {
    let def = &service.def;
    let http = def.http.as_ref();
    json!({
        "id": service.id,
        "name": def.name,
        "description": def.description,
        "level": def.level,
        "status": status,
        "cli": cli(service),
        "ui": ui(service, config),
        "state": http.and_then(|h| h.state.clone()),
        "events": http.and_then(|h| h.events.clone()),
        "run_file": home.run_path(&service.id),
        "docs": offer.docs,
        "skills": offer.skills.iter().map(|s| json!({
            "name": s.name,
            "description": s.description,
            "path": s.dir.join("SKILL.md"),
        })).collect::<Vec<_>>(),
    })
}

/// What agents are told: the discoverable services only.
pub fn index(home: &Home, config: &Config, services: &[Service], discoverable: &BTreeSet<String>, status: impl Fn(&str) -> Option<String>) -> Value {
    let apps: Vec<Value> = services
        .iter()
        .filter(|s| discoverable.contains(&s.id))
        .map(|s| describe(s, &offer(s), home, config, status(&s.id)))
        .collect();
    json!({ "apps": apps })
}

fn one_line(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= limit {
        return text;
    }
    let cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// SKILL.md of the `launchx` skill: which apps the agent may use, and how.
pub fn own_skill(apps: &[(&Service, Offer)], home: &Home, config: &Config) -> String {
    let short: Vec<String> = apps.iter().map(|(s, _)| format!("{} ({})", s.def.name, s.id)).collect();
    let long: Vec<String> = apps
        .iter()
        .map(|(s, _)| match &s.def.description {
            Some(d) => format!("{} ({}): {}", s.def.name, s.id, one_line(d, 120)),
            None => format!("{} ({})", s.def.name, s.id),
        })
        .collect();
    let mut list = long.join("; ");
    if list.chars().count() > 700 {
        list = one_line(&short.join(", "), 700);
    }
    let description = format!(
        "Use the user's local apps that LaunchX runs: {list}. Use when the user asks about one of these apps or its data, or wants something done in it. Tells you each app's command-line tool, JSON state and skills."
    );
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {OWN_SKILL}\n"));
    // A JSON string is a valid YAML double-quoted scalar.
    out.push_str(&format!("description: {}\n", serde_json::to_string(&description).unwrap_or_default()));
    out.push_str("---\n\n");
    out.push_str("<!-- Written by LaunchX from the apps the user made discoverable by agents. Do not edit; it is rewritten. -->\n\n");
    out.push_str("# The user's apps in LaunchX\n\n");
    out.push_str("LaunchX runs local apps for the user and gives each one a page in the browser. The user chose which apps agents may know about. These are the apps:\n");
    for (service, offer) in apps {
        let def = &service.def;
        out.push_str(&format!("\n## {} (`{}`)\n\n", def.name, service.id));
        if let Some(description) = &def.description {
            out.push_str(&format!("{}\n\n", one_line(description, 400)));
        }
        match (cli(service), def.level) {
            (Some(cli), 2) => out.push_str(&format!(
                "- Command-line tool: `{cli}`. `{cli} state` prints the app's whole state as JSON, and commands that read take `--json`.\n"
            )),
            (Some(cli), _) => out.push_str(&format!("- Command-line tool: `{cli}`.\n")),
            (None, _) => out.push_str(&format!("- No command-line tool. `lx start {0}`, `lx stop {0}` and `lx logs {0}` work on it.\n", service.id)),
        }
        if let Some(http) = &def.http {
            if let Some(state) = &http.state {
                let events = http.events.as_ref().map(|e| format!(", and `GET {e}` streams change events")).unwrap_or_default();
                out.push_str(&format!("- Over HTTP, `GET {state}` returns its state{events}.\n"));
            }
        }
        if let Some(ui) = ui(service, config) {
            out.push_str(&format!("- Its page, for the user: {ui} (`lx open {}` opens it signed in).\n", service.id));
        }
        if let Some(docs) = &offer.docs {
            out.push_str(&format!("- Notes for agents: {}\n", docs.display()));
        }
        if !offer.skills.is_empty() {
            out.push_str("- Skills (read the one that matches the task):\n");
            for skill in &offer.skills {
                out.push_str(&format!("  - `{}`: {} ({})\n", skill.name, one_line(&skill.description, 200), skill.dir.join("SKILL.md").display()));
            }
        }
    }
    let run = home.root.join("run");
    out.push_str(&format!(
        "\n## Working with these apps\n\n\
- `lx agents --json` lists these apps with their live status. An app that is not running answers nothing: start it with `lx start <id>` and tell the user you did.\n\
- Prefer an app's command-line tool. What its page can do, the tool can do, and the page shows the change at once.\n\
- To call an app over HTTP, read `{}/<id>.json` for its `url` and `token` and send `Authorization: Bearer <token>`. Never print, log or store the token. The page address is for the user's browser, not for programs.\n\
\n## Apps that are not listed\n\n\
The user has hidden any other LaunchX app from agents. Do not look for one (for example with `lx ls` or `lx state`), read its state or run its tools unless the user names it and asks you to.\n",
        run.display()
    ));
    out
}

/// The result of bringing the agents' skill folders in line with the switches.
#[derive(Debug, Default)]
pub struct Published {
    /// Skill names of every registered service, discoverable or not, for the UI.
    pub skills: BTreeMap<String, Vec<String>>,
    pub problems: Vec<String>,
}

/// Publish the skills of the discoverable services, and LaunchX's own skill,
/// into each agent skill folder that exists; remove what LaunchX published
/// before and is no longer wanted. Every published skill is a link through
/// `$LAUNCHX_HOME/agents/skills`, and only links into that folder are ever
/// removed from an agent's folder. Anything else there is left alone.
pub fn publish(home: &Home, config: &Config, services: &[Service], discoverable: &BTreeSet<String>) -> Published {
    let mut published = Published::default();
    let mut apps: Vec<(&Service, Offer)> = Vec::new();
    for service in services {
        let offer = offer(service);
        published.skills.insert(service.id.clone(), offer.skills.iter().map(|s| s.name.clone()).collect());
        if discoverable.contains(&service.id) {
            published.problems.extend(offer.problems.iter().cloned());
            apps.push((service, offer));
        }
    }

    // Skill name → (service, folder). The first service to claim a name keeps it.
    let mut wanted: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    for (service, offer) in &apps {
        for skill in &offer.skills {
            match wanted.get(&skill.name) {
                Some((owner, _)) => published.problems.push(format!(
                    "{}: skill '{}' is also published by {owner}; not published again",
                    service.id, skill.name
                )),
                None => {
                    wanted.insert(skill.name.clone(), (service.id.clone(), skill.dir.clone()));
                }
            }
        }
    }

    let staging = home.agent_skills_dir();
    if let Err(error) = fs::create_dir_all(&staging) {
        published.problems.push(format!("cannot create {}: {error}", staging.display()));
        return published;
    }

    // LaunchX's own skill: present while at least one app is discoverable.
    let own = staging.join(OWN_SKILL);
    if apps.is_empty() {
        if own.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            let _ = fs::remove_dir_all(&own);
        }
    } else {
        let text = own_skill(&apps, home, config);
        let file = own.join("SKILL.md");
        if fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
            if let Err(error) = fs::create_dir_all(&own).and_then(|_| fs::write(&file, &text)) {
                published.problems.push(format!("cannot write {}: {error}", file.display()));
            }
        }
    }

    // The staging folder: one link per published service skill.
    for entry in fs::read_dir(&staging).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Ok(target) = fs::read_link(entry.path()) {
            if wanted.get(&name).map(|(_, dir)| dir) != Some(&target) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    for (name, (_, dir)) in &wanted {
        let link = staging.join(name);
        if link.symlink_metadata().is_err() {
            if let Err(error) = symlink(dir, &link) {
                published.problems.push(format!("cannot link {}: {error}", link.display()));
            }
        }
    }

    // The agents' folders: a link to each staged skill.
    let mut visible: BTreeSet<String> = wanted.keys().cloned().collect();
    if !apps.is_empty() {
        visible.insert(OWN_SKILL.to_string());
    }
    for dir in config.agent_skill_dirs() {
        if !dir.is_dir() || fs::canonicalize(&dir).ok() == fs::canonicalize(&staging).ok() {
            continue;
        }
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(target) = fs::read_link(entry.path()) else { continue };
            if !target.starts_with(&staging) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !visible.contains(&name) || target != staging.join(&name) {
                let _ = fs::remove_file(entry.path());
            }
        }
        for name in &visible {
            let link = dir.join(name);
            let target = staging.join(name);
            if fs::read_link(&link).ok().as_ref() == Some(&target) {
                continue;
            }
            if link.symlink_metadata().is_ok() {
                published.problems.push(format!("{} already exists and is not LaunchX's, so the {name} skill is not published there", link.display()));
            } else if let Err(error) = symlink(&target, &link) {
                published.problems.push(format!("cannot link {}: {error}", link.display()));
            }
        }
    }
    published
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lx-agents-{}-{}", std::process::id(), crate::util::token()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn skill(dir: &Path, name: &str, front: &str) {
        fs::create_dir_all(dir.join(name)).unwrap();
        fs::write(dir.join(name).join("SKILL.md"), format!("---\n{front}\n---\n\n# {name}\n")).unwrap();
    }

    #[test]
    fn reads_front_matter() {
        let fields = frontmatter("---\nname: plan-day\ndescription: Plan the day: fill blocks.\nother: x\n---\nbody: no\n");
        assert_eq!(fields["name"], "plan-day");
        assert_eq!(fields["description"], "Plan the day: fill blocks.");
        assert!(!fields.contains_key("body"));
        let fields = frontmatter("---\nname: 'a-b'\ndescription: \"Say \\\"hi\\\": now\"\n---\n");
        assert_eq!(fields["name"], "a-b");
        assert_eq!(fields["description"], "Say \"hi\": now");
        let fields = frontmatter("---\ndescription: >\n  Folded over\n  two lines.\nname: x\n---\n");
        assert_eq!(fields["description"], "Folded over two lines.");
        assert_eq!(fields["name"], "x");
        assert!(frontmatter("no front matter\nname: x\n").is_empty());
        assert!(valid_skill_name("plan-day") && !valid_skill_name("Plan") && !valid_skill_name("-x") && !valid_skill_name(""));
    }

    #[test]
    fn publishes_and_withdraws_without_touching_other_skills() {
        let root = temp();
        let home = Home { root: root.join("home") };
        home.ensure().unwrap();
        let project = root.join("project");
        skill(&project.join("skills"), "plan-day", "name: plan-day\ndescription: Plan the day.");
        skill(&project.join("skills"), "taken", "name: taken\ndescription: Clashes with the user's own skill.");
        skill(&project.join("skills"), "bad", "name: Bad Name\ndescription: x");
        fs::write(
            project.join("launchx.toml"),
            "launchx = \"0.1\"\n[service.app]\nname = \"App\"\ndescription = \"Does things\"\nlevel = 2\nrun = [\"app\", \"serve\"]\ncli = \"app\"\n\
             [service.app.agents]\nskills = \"skills\"\n[service.other]\nname = \"Other\"\nrun = [\"other\"]\n",
        )
        .unwrap();
        let file = project.join("launchx.toml");
        let services = manifest::parse(&fs::canonicalize(&file).unwrap(), &file).unwrap();
        let agent = root.join("agent-skills");
        skill(&agent, "mine", "name: mine\ndescription: The user's own.");
        skill(&agent, "taken", "name: taken\ndescription: The user's own too.");
        let config = Config { agent_skill_dirs: vec![agent.display().to_string(), root.join("missing").display().to_string()], ..Config::default() };

        // Off: nothing published, but the UI still learns the skill names.
        let off = publish(&home, &config, &services, &BTreeSet::new());
        assert_eq!(off.skills["app"], ["plan-day", "taken"]);
        assert!(off.problems.is_empty(), "{:?}", off.problems);
        assert!(!agent.join(OWN_SKILL).exists() && !agent.join("plan-day").exists());

        // On: the app's skills and LaunchX's own skill appear; the user's are untouched.
        let on: BTreeSet<String> = ["app".to_string()].into();
        let published = publish(&home, &config, &services, &on);
        assert!(agent.join("plan-day/SKILL.md").is_file());
        let own = fs::read_to_string(agent.join(OWN_SKILL).join("SKILL.md")).unwrap();
        assert!(own.contains("App (app): Does things") && own.contains("`app state`") && !own.contains("Other"), "{own}");
        assert_eq!(frontmatter(&own)["name"], OWN_SKILL);
        assert!(fs::read_to_string(agent.join("taken/SKILL.md")).unwrap().contains("The user's own too."));
        assert!(published.problems.iter().any(|p| p.contains("taken already exists")), "{:?}", published.problems);
        assert!(published.problems.iter().any(|p| p.contains("Bad Name")), "{:?}", published.problems);
        assert!(!root.join("missing").exists(), "a missing agent folder is not created");

        // The index lists only what is discoverable.
        let index = index(&home, &config, &services, &on, |_| Some("running".into()));
        let apps = index["apps"].as_array().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0]["skills"][0]["name"], "plan-day");

        // Off again: LaunchX's links go, everything else stays.
        publish(&home, &config, &services, &BTreeSet::new());
        assert!(agent.join("mine/SKILL.md").is_file() && agent.join("taken/SKILL.md").is_file());
        assert!(agent.join("plan-day").symlink_metadata().is_err() && agent.join(OWN_SKILL).symlink_metadata().is_err());
        assert!(!home.agent_skills_dir().join(OWN_SKILL).exists() && !home.agent_skills_dir().join("plan-day").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
