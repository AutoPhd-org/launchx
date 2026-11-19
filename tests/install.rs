//! `lx install`, `lx update` and `lx uninstall` against a real daemon and a
//! local git repository (contract §4.9, §10.8), from the command line and the API.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const LX: &str = env!("CARGO_BIN_EXE_lx");

struct Env {
    root: PathBuf,
    home: PathBuf,
    daemon: Option<Child>,
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            let _ = Command::new("kill").arg(daemon.id().to_string()).status();
            let _ = daemon.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Env {
    fn lx(&self, args: &[&str]) -> Output {
        Command::new(LX)
            .args(args)
            .env("LAUNCHX_HOME", &self.home)
            .env("LAUNCHX_INHERIT_ENV", "1")
            .env("HOME", &self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn lx_text(&self, args: &[&str]) -> (bool, String) {
        let output = self.lx(args);
        (output.status.success(), format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
    }

    fn lx_ok(&self, args: &[&str]) -> String {
        let (ok, text) = self.lx_text(args);
        assert!(ok, "lx {args:?} failed:\n{text}");
        text
    }

    fn state(&self) -> serde_json::Value {
        serde_json::from_str(&self.lx_ok(&["state"])).unwrap()
    }

    fn service(&self, id: &str) -> Option<serde_json::Value> {
        self.state()["services"].as_array().unwrap().iter().find(|s| s["id"] == id).cloned()
    }

    fn installs(&self) -> serde_json::Value {
        fs::read_to_string(self.home.join("installs.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn history(&self) -> String {
        fs::read_to_string(self.root.join("install-history")).unwrap_or_default()
    }

    /// Call the launcher's API with its bearer token.
    fn api(&self, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
        let run: serde_json::Value = serde_json::from_str(&fs::read_to_string(self.home.join("run/launchx.json")).unwrap()).unwrap();
        let port: u16 = run["url"].as_str().unwrap().rsplit(':').next().unwrap().parse().unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        let text = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            run["token"].as_str().unwrap(),
            body.len()
        );
        stream.write_all(text.as_bytes()).unwrap();
        let mut raw = String::new();
        let _ = stream.read_to_string(&mut raw);
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        (status, serde_json::from_str(body).unwrap_or_default())
    }
}

fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A repository holding an app: the fixture service, an installer that
/// records each run (and fails if it gets a terminal), and an uninstaller.
fn setup() -> Env {
    let root = std::env::temp_dir().join(format!("lx-install-{}-{}", std::process::id(), free_port()));
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join("config.toml"), format!("gateway_port = {}\nbackoff_unit_ms = 40\n", free_port())).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/svc.py");
    executable(&fixture, &fs::read_to_string(&fixture).unwrap());

    let repo = root.join("echo-app");
    fs::create_dir_all(&repo).unwrap();
    fs::write(
        repo.join("launchx.toml"),
        format!(
            r#"launchx = "0.1"

[service.echo]
name = "Echo"
level = 1
run = ["{}"]

[service.echo.http]
ui = "/"

[install]
run = ["./install.sh"]
uninstall = ["./uninstall.sh"]
needs = ["git"]
data = ["~/echo-data"]
"#,
            fixture.display()
        ),
    )
    .unwrap();
    executable(
        &repo.join("install.sh"),
        "#!/bin/sh\nset -e\n[ -t 0 ] && exit 9\nmkdir -p \"$HOME/echo-data\"\necho \"$LAUNCHX_INSTALL $(cat version)\" >> \"$HOME/install-history\"\necho installing version $(cat version)\n",
    );
    executable(&repo.join("uninstall.sh"), "#!/bin/sh\necho uninstall >> \"$HOME/install-history\"\n");
    fs::write(repo.join("version"), "1\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);

    let mut env = Env { root, home, daemon: None };
    let daemon = Command::new(LX)
        .arg("daemon")
        .env("LAUNCHX_HOME", &env.home)
        .env("LAUNCHX_INHERIT_ENV", "1")
        .env("HOME", &env.root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    env.daemon = Some(daemon);
    wait_for("the daemon", Duration::from_secs(10), || env.home.join("run/launchx.json").exists());
    env
}

#[test]
fn install_update_uninstall() {
    let env = setup();
    let repo = env.root.join("echo-app");
    let url = format!("file://{}", repo.display());

    // Nothing yet; asking without a terminal is refused.
    assert!(env.lx_ok(&["install"]).contains("Nothing is installed"));
    let (ok, text) = env.lx_text(&["install", &url]);
    assert!(!ok && text.contains("--yes"), "{text}");
    assert!(env.service("echo").is_none(), "nothing may be registered before the user says yes");

    // Install from a URL: cloned, installer run without a terminal, registered, recorded.
    let text = env.lx_ok(&["install", &url, "--yes"]);
    assert!(text.contains("runs      ./install.sh") && text.contains("installing version 1") && text.contains("✓ Installed echo-app"), "{text}");
    assert!(env.home.join("sources/echo-app/.git").is_dir());
    assert_eq!(env.history(), "install 1\n");
    let package = &env.installs()["packages"]["echo-app"];
    assert_eq!(package["cloned"], true);
    assert_eq!(package["services"], serde_json::json!(["echo"]));
    assert_eq!(package["data"], serde_json::json!(["~/echo-data"]));
    let service = env.service("echo").expect("echo is registered");
    assert_eq!(service["package"], "echo-app");
    assert_eq!(env.state()["packages"][0]["name"], "echo-app");
    assert!(env.lx_ok(&["install"]).contains("echo-app"));

    // Up to date, then a new commit upstream.
    env.lx_ok(&["start", "echo"]);
    assert!(env.lx_ok(&["update", "--check"]).contains("echo-app: up to date"));
    assert!(env.lx_ok(&["update"]).contains("is up to date"));
    fs::write(repo.join("version"), "2\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "two"]);
    assert!(env.lx_ok(&["update", "--check"]).contains("1 new commit"));
    assert_eq!(env.installs()["packages"]["echo-app"]["behind"], 1);
    let pid = env.service("echo").unwrap()["pid"].clone();
    let text = env.lx_ok(&["update", "echo"]);
    assert!(text.contains("installing version 2") && text.contains("✓ Updated echo-app"), "{text}");
    assert!(text.contains("Stopped echo while installing") && text.contains("Started echo again"), "{text}");
    assert_eq!(env.history(), "install 1\nupdate 2\n");
    wait_for("echo running again", Duration::from_secs(10), || env.service("echo").unwrap()["status"] == "running");
    assert_ne!(env.service("echo").unwrap()["pid"], pid, "the update restarts what was running");
    assert_eq!(env.installs()["packages"]["echo-app"]["behind"], 0);

    // The same through the API, as the page does it: a forced update runs as a job.
    let (status, reply) = env.api("POST", "/api/packages/echo-app/update", r#"{"force": true}"#);
    assert_eq!(status, 202, "{reply}");
    let (status, _) = env.api("POST", "/api/packages/echo-app/update", "{}");
    assert_eq!(status, 409, "one job at a time");
    wait_for("the update job", Duration::from_secs(30), || env.state()["job"]["status"] == "done");
    assert_eq!(env.history(), "install 1\nupdate 2\nupdate 2\n");
    let (_, log) = env.api("GET", "/api/install/log", "");
    assert!(log["lines"].as_array().unwrap().iter().any(|l| l == "installing version 2"), "{log}");
    let (status, plan) = env.api("POST", "/api/install/plan", &format!(r#"{{"source": "{url}"}}"#));
    assert_eq!(status, 200, "{plan}");
    assert_eq!(plan["installed"], true);
    assert_eq!(plan["run"], serde_json::json!(["./install.sh"]));
    let (status, _) = env.api("POST", "/api/install/plan", r#"{"source": "not a source"}"#);
    assert_eq!(status, 422);

    // lx rm points at uninstall; uninstall without a terminal needs --yes.
    let (ok, text) = env.lx_text(&["uninstall", "echo-app"]);
    assert!(!ok && text.contains("--yes"), "{text}");
    let text = env.lx_ok(&["uninstall", "echo", "--yes"]);
    assert!(text.contains("✓ Uninstalled echo-app") && text.contains("still in ~/echo-data"), "{text}");
    assert!(env.history().ends_with("uninstall\n"));
    assert!(!env.home.join("sources/echo-app").exists(), "the checkout LaunchX cloned is deleted");
    assert!(env.root.join("echo-data").is_dir(), "the app's data stays");
    assert!(env.service("echo").is_none());
    assert!(env.installs()["packages"].as_object().unwrap().is_empty());

    // From a folder: installed in place, never deleted.
    let text = env.lx_ok(&["install", repo.to_str().unwrap(), "--yes", "--name", "Local Echo"]);
    assert!(text.contains("✓ Installed local-echo"), "{text}");
    assert_eq!(env.installs()["packages"]["local-echo"]["cloned"], false);
    let (ok, text) = env.lx_text(&["install", &url, "--yes", "--name", "local-echo"]);
    assert!(!ok && text.contains("already installed from"), "{text}");
    assert!(env.lx_ok(&["update", "--check"]).contains("installed from the folder"));
    env.lx_ok(&["uninstall", "local-echo", "--yes"]);
    assert!(repo.join("launchx.toml").is_file(), "a folder LaunchX did not clone is left alone");

    // Through the API, as the page's Install button does it.
    let (status, reply) = env.api("POST", "/api/install", &format!(r#"{{"source": "{url}"}}"#));
    assert_eq!(status, 202, "{reply}");
    wait_for("the install job", Duration::from_secs(30), || env.state()["job"]["status"] != "running");
    assert_eq!(env.state()["job"]["status"], "done", "{}", env.state()["job"]);
    assert_eq!(env.state()["job"]["name"], "echo-app");
    let (status, _) = env.api("POST", "/api/packages/echo-app/uninstall", "");
    assert_eq!(status, 202);
    wait_for("the uninstall job", Duration::from_secs(30), || env.state()["job"]["status"] != "running");
    assert_eq!(env.state()["job"]["status"], "done", "{}", env.state()["job"]);
    assert!(env.service("echo").is_none());
}
