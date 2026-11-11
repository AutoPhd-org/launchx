//! Runs a real `lx daemon` against fixture services and checks the contract end to end:
//! dependencies, the gateway's sign-in, header swap, Origin check, streaming,
//! Upgrade, restart policy, `lx check`, the agents switch, and shutdown.

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
    port: u16,
    daemon: Option<Child>,
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(mut daemon) = self.daemon.take() {
            unsafe { libc_kill(daemon.id() as i32, 15) };
            let _ = daemon.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
}

impl Env {
    fn lx(&self, args: &[&str]) -> Output {
        Command::new(LX)
            .args(args)
            .env("LAUNCHX_HOME", &self.home)
            .env("LAUNCHX_INHERIT_ENV", "1")
            .env("HOME", &self.root)
            .output()
            .unwrap()
    }

    fn lx_ok(&self, args: &[&str]) -> String {
        let output = self.lx(args);
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "lx {args:?} failed:\n{text}");
        text
    }

    fn host(&self, name: &str) -> String {
        format!("{name}.localhost:{}", self.port)
    }

    fn run_file(&self, id: &str) -> Option<serde_json::Value> {
        fs::read_to_string(self.home.join("run").join(format!("{id}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok())
    }

    fn state(&self) -> serde_json::Value {
        serde_json::from_str(&self.lx_ok(&["state"])).unwrap()
    }

    fn service(&self, id: &str) -> serde_json::Value {
        self.state()["services"].as_array().unwrap().iter().find(|s| s["id"] == id).unwrap().clone()
    }
}

struct Reply {
    status: u16,
    headers: String,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim().to_string())
        })
    }
    fn cookie(&self) -> String {
        self.header("set-cookie").unwrap().split(';').next().unwrap().to_string()
    }
}

fn request(port: u16, method: &str, host: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (key, value) in headers {
        text.push_str(&format!("{key}: {value}\r\n"));
    }
    text.push_str("\r\n");
    text.push_str(body);
    stream.write_all(text.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Reply { status, headers: head.to_string(), body: body.to_string() }
}

fn path_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.find('/').map(|i| rest[i..].to_string()).unwrap_or_else(|| "/".into())
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

fn fixture() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/svc.py");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn setup() -> Env {
    let root = std::env::temp_dir().join(format!("lx-e2e-{}-{}", std::process::id(), free_port()));
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let port = free_port();
    fs::write(home.join("config.toml"), format!("gateway_port = {port}\nbackoff_unit_ms = 40\n")).unwrap();
    let svc = fixture();
    let dir = svc.parent().unwrap().display().to_string();

    let project = root.join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("launchx.toml"),
        format!(
            r#"launchx = "0.1"

[service.alpha]
name = "Alpha"
level = 2
run = ["./svc.py"]
cwd = "{dir}"
cli = "./svc.py"

[service.alpha.http]
ui = "/"
state = "/api/state"
events = "/api/events"

[service.alpha.agents]
skills = "skills"

[service.beta]
name = "Beta"
level = 1
run = ["./svc.py"]
cwd = "{dir}"
requires = ["alpha"]
stop_timeout = 5

[service.beta.http]
ui = "/"

[service.crashy]
name = "Crashy"
level = 1
run = ["./svc.py"]
cwd = "{dir}"
env = {{ FIXTURE_MODE = "crash" }}

[service.crashy.http]

[service.sloppy]
name = "Sloppy"
level = 2
run = ["./svc.py"]
cwd = "{dir}"
cli = "./svc.py"
env = {{ FIXTURE_MODE = "noauth" }}

[service.sloppy.http]
state = "/api/state"
"#
        ),
    )
    .unwrap();
    let hello = project.join("skills/hello");
    fs::create_dir_all(&hello).unwrap();
    fs::write(hello.join("SKILL.md"), "---\nname: hello\ndescription: Say hello to Alpha.\n---\n\nRun `svc.py state`.\n").unwrap();
    // An agent's skill folder (HOME is the test root), with a skill of the user's own.
    let mine = root.join(".claude/skills/mine");
    fs::create_dir_all(&mine).unwrap();
    fs::write(mine.join("SKILL.md"), "---\nname: mine\ndescription: The user's own skill.\n---\n").unwrap();
    let plain = root.join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("launchx.toml"), "launchx = \"0.1\"\n\n[service.sleeper]\nname = \"Sleeper\"\nrun = [\"sleep\", \"300\"]\n").unwrap();

    let mut env = Env { root, home, port, daemon: None };
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
    wait_for("the daemon", Duration::from_secs(10), || env.run_file("launchx").is_some());
    env
}

#[test]
fn launcher_end_to_end() {
    let mut env = setup();
    let gw = env.port;

    // Register while running; the daemon reloads.
    let added = env.lx_ok(&["add", env.root.join("project").to_str().unwrap()]);
    assert!(added.contains("Registered alpha"), "{added}");
    env.lx_ok(&["add", env.root.join("plain").to_str().unwrap()]);
    let again = env.lx(&["add", env.root.join("project").to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&again.stdout).contains("Already registered"));

    // Starting beta starts alpha first; beta learns alpha's address.
    let started = env.lx_ok(&["start", "beta"]);
    assert!(started.contains("✓ alpha running") && started.contains("✓ beta running"), "{started}");
    let alpha_run = env.run_file("alpha").expect("alpha run file");
    let beta_run = env.run_file("beta").expect("beta run file");
    let beta_state = request(
        beta_run["url"].as_str().unwrap().rsplit(':').next().unwrap().parse().unwrap(),
        "GET",
        "127.0.0.1",
        "/api/state",
        &[("Authorization", &format!("Bearer {}", beta_run["token"].as_str().unwrap()))],
        "",
    );
    assert!(beta_state.body.contains(alpha_run["url"].as_str().unwrap()), "beta did not get LAUNCHX_DEP_ALPHA_URL: {}", beta_state.body);
    assert_eq!(env.service("alpha")["summary"], "counter at 0");
    assert_eq!(env.service("alpha")["version"], "1.2.3");

    // The launcher's own API: bearer for programs, nothing for strangers.
    let launcher_token = env.run_file("launchx").unwrap()["token"].as_str().unwrap().to_string();
    let bearer = format!("Bearer {launcher_token}");
    assert_eq!(request(gw, "GET", &format!("127.0.0.1:{gw}"), "/api/health", &[], "").status, 401);
    assert_eq!(request(gw, "GET", &format!("127.0.0.1:{gw}"), "/api/health", &[("Authorization", &bearer)], "").status, 200);
    assert_eq!(request(gw, "GET", &env.host("launchx"), "/api/state", &[], "").status, 401);
    assert_eq!(request(gw, "GET", &env.host("nosuch"), "/", &[], "").status, 421);
    assert_eq!(request(gw, "GET", "evil.example", "/", &[], "").status, 421);

    // A browser without a session is sent to sign in; an API call is refused.
    let page = request(gw, "GET", &env.host("alpha"), "/x?y=1", &[("Accept", "text/html")], "");
    assert_eq!(page.status, 302);
    assert!(page.header("location").unwrap().contains("/.launchx/authorize?service=alpha&next=/x%3Fy%3D1"));
    assert_eq!(request(gw, "GET", &env.host("alpha"), "/api/state", &[], "").status, 401);

    // `lx open alpha` link: launcher session, then a ticket to alpha's origin.
    let link = env.lx_ok(&["open", "alpha", "--print"]).trim().lines().last().unwrap().to_string();
    let first = request(gw, "GET", &env.host("launchx"), &path_of(&link), &[], "");
    assert_eq!(first.status, 303, "{}", first.headers);
    let launcher_cookie = first.cookie();
    assert!(first.header("set-cookie").unwrap().contains("HttpOnly; SameSite=Lax"));
    assert_eq!(request(gw, "GET", &env.host("launchx"), &path_of(&link), &[], "").status, 401, "codes are single-use");
    let authorize = request(gw, "GET", &env.host("launchx"), &first.header("location").unwrap(), &[("Cookie", &launcher_cookie)], "");
    assert_eq!(authorize.status, 302);
    let ticket = authorize.header("location").unwrap();
    assert!(ticket.starts_with(&format!("http://{}/.launchx/login?code=", env.host("alpha"))));
    assert_eq!(request(gw, "GET", &env.host("beta"), &path_of(&ticket), &[], "").status, 401, "a ticket only works on its own host");
    let authorize = request(gw, "GET", &env.host("launchx"), &first.header("location").unwrap(), &[("Cookie", &launcher_cookie)], "");
    let landed = request(gw, "GET", &env.host("alpha"), &path_of(&authorize.header("location").unwrap()), &[], "");
    assert_eq!(landed.status, 303);
    let alpha_cookie = landed.cookie();
    assert_eq!(request(gw, "GET", &env.host("alpha"), "/api/state", &[("Cookie", &launcher_cookie.replace("lx_session", "lx_session"))], "").status, 401, "sessions are bound to their host");

    // Proxying: the page, the token swap, and cookies the service set itself.
    let home = request(gw, "GET", &env.host("alpha"), "/", &[("Cookie", &alpha_cookie)], "");
    assert_eq!(home.status, 200);
    assert!(home.body.contains("fixture page for alpha"));
    let seen = request(
        gw,
        "GET",
        &env.host("alpha"),
        "/headers",
        &[("Cookie", &format!("theirs=1; {alpha_cookie}")), ("Authorization", "Bearer forged")],
        "",
    );
    let seen: serde_json::Value = serde_json::from_str(&seen.body).unwrap();
    assert_eq!(seen["authorization"], format!("Bearer {}", alpha_run["token"].as_str().unwrap()));
    assert_eq!(seen["cookie"], "theirs=1");
    assert_eq!(seen["x-forwarded-host"], env.host("alpha"));

    // Writes need the page's own Origin.
    assert_eq!(request(gw, "POST", &env.host("alpha"), "/api/bump", &[("Cookie", &alpha_cookie)], "{}").status, 403);
    assert_eq!(request(gw, "POST", &env.host("alpha"), "/api/bump", &[("Cookie", &alpha_cookie), ("Origin", "http://evil.example")], "{}").status, 403);
    let bumped = request(gw, "POST", &env.host("alpha"), "/api/bump", &[("Cookie", &alpha_cookie), ("Origin", &format!("http://{}", env.host("alpha")))], "{}");
    assert_eq!(bumped.status, 200);
    assert!(bumped.body.contains("\"counter\": 1"));

    // Streams pass through unbuffered.
    let mut stream = TcpStream::connect(("127.0.0.1", gw)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(stream, "GET /api/events HTTP/1.1\r\nHost: {}\r\nCookie: {alpha_cookie}\r\n\r\n", env.host("alpha")).unwrap();
    let mut buffer = [0u8; 4096];
    let mut seen_events = String::new();
    while !seen_events.contains("heartbeat") {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0, "stream closed early: {seen_events}");
        seen_events.push_str(&String::from_utf8_lossy(&buffer[..n]));
    }
    drop(stream);

    // Upgrade (as WebSockets use) is spliced through.
    let mut stream = TcpStream::connect(("127.0.0.1", gw)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(stream, "GET /echo HTTP/1.1\r\nHost: {}\r\nCookie: {alpha_cookie}\r\nConnection: Upgrade\r\nUpgrade: echo\r\n\r\n", env.host("alpha")).unwrap();
    let mut head = Vec::new();
    while !String::from_utf8_lossy(&head).contains("\r\n\r\n") {
        let n = stream.read(&mut buffer).unwrap();
        assert!(n > 0);
        head.extend_from_slice(&buffer[..n]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"), "{}", String::from_utf8_lossy(&head));
    stream.write_all(b"ping").unwrap();
    let n = stream.read(&mut buffer).unwrap();
    assert_eq!(&buffer[..n], b"echo:ping");
    drop(stream);

    // Restart policy: a crashing service is retried, then marked failed.
    let crashed = env.lx(&["start", "crashy"]);
    assert!(!crashed.status.success());
    let crashy = env.service("crashy");
    assert_eq!(crashy["status"], "failed");
    assert!(crashy["error"].as_str().unwrap().contains("Gave up after 5"), "{crashy}");
    assert_eq!(crashy["restarts"], 4);
    let logs = env.lx_ok(&["logs", "crashy", "-n", "50"]);
    assert!(logs.contains("crashing on purpose") && logs.contains("[launchx] restarting in"), "{logs}");

    // A stopped service shows a page instead of a connection error.
    let stopped = request(gw, "GET", &env.host("crashy"), "/", &[("Accept", "text/html")], "");
    assert_eq!(stopped.status, 302, "needs a session first");

    // lx check passes a conforming service and catches one that skips auth.
    let check = env.lx_ok(&["check", "alpha"]);
    assert!(check.contains("checks passed") && !check.contains('✗'), "{check}");
    let sloppy = env.lx(&["check", "sloppy", "--json"]);
    assert!(!sloppy.status.success());
    let report: serde_json::Value = serde_json::from_slice(&sloppy.stdout).unwrap();
    let failed: Vec<&str> = report["results"].as_array().unwrap().iter().filter(|r| r["ok"] == false).map(|r| r["check"].as_str().unwrap()).collect();
    assert_eq!(failed, ["rejects a request without the token", "rejects a wrong token"], "{report}");

    // Agents: every service starts hidden; nothing is published.
    let agent_dir = env.root.join(".claude/skills");
    let agents_json = |env: &Env| -> serde_json::Value { serde_json::from_str(&env.lx_ok(&["agents", "--json"])).unwrap() };
    assert_eq!(env.service("alpha")["agents"]["discoverable"], false);
    assert_eq!(env.service("alpha")["agents"]["skills"][0], "hello");
    assert!(agents_json(&env)["apps"].as_array().unwrap().is_empty());
    assert!(env.lx_ok(&["agents"]).contains("No app is discoverable"));
    assert!(agent_dir.join("launchx").symlink_metadata().is_err());

    // Switching alpha on lists it for agents and publishes its skill and LaunchX's own.
    let on = env.lx_ok(&["agents", "on", "alpha"]);
    assert!(on.contains("alpha is now discoverable by agents"), "{on}");
    assert!(!env.lx(&["agents", "on", "nosuch"]).status.success());
    let index = agents_json(&env);
    let apps = index["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 1, "{index}");
    assert_eq!(apps[0]["id"], "alpha");
    assert_eq!(apps[0]["status"], "running");
    assert_eq!(apps[0]["skills"][0]["name"], "hello");
    assert!(agent_dir.join("hello/SKILL.md").is_file());
    let own = fs::read_to_string(agent_dir.join("launchx/SKILL.md")).unwrap();
    assert!(own.contains("## Alpha (`alpha`)") && own.contains("`hello`") && !own.contains("Beta"), "{own}");
    assert!(env.lx_ok(&["ls"]).lines().any(|l| l.starts_with("alpha") && l.contains(" on ")));

    // The page's switch uses the same API, and needs the page's own Origin.
    let launcher_origin = format!("http://{}", env.host("launchx"));
    let switch_beta = |headers: &[(&str, &str)]| request(gw, "POST", &env.host("launchx"), "/api/services/beta/agents", headers, r#"{"discoverable": true}"#);
    assert_eq!(switch_beta(&[("Cookie", &launcher_cookie)]).status, 403);
    let switched = switch_beta(&[("Cookie", &launcher_cookie), ("Origin", &launcher_origin), ("Content-Type", "application/json")]);
    assert_eq!(switched.status, 200, "{}", switched.body);
    assert_eq!(env.service("beta")["agents"]["discoverable"], true);
    assert!(fs::read_to_string(agent_dir.join("launchx/SKILL.md")).unwrap().contains("## Beta (`beta`)"));

    // Switching off withdraws what LaunchX published, and nothing else.
    env.lx_ok(&["agents", "off", "alpha", "beta"]);
    assert!(agents_json(&env)["apps"].as_array().unwrap().is_empty());
    assert!(agent_dir.join("hello").symlink_metadata().is_err() && agent_dir.join("launchx").symlink_metadata().is_err());
    assert!(agent_dir.join("mine/SKILL.md").is_file());

    // Stopping alpha stops beta first; run files go away.
    let stopped = env.lx_ok(&["stop", "alpha"]);
    assert!(stopped.contains("Stopping beta first") && stopped.contains("Stopped beta, alpha"), "{stopped}");
    assert!(env.run_file("alpha").is_none() && env.run_file("beta").is_none());
    assert_eq!(env.service("alpha")["last_exit"], "exited with code 0");

    // Unregistering removes the link, never the manifest.
    env.lx_ok(&["start", "sleeper"]);
    let sleeper_pid = env.service("sleeper")["pid"].as_u64().unwrap() as i32;
    env.lx_ok(&["rm", "sleeper"]);
    assert!(env.root.join("plain/launchx.toml").exists());
    assert!(!env.state()["services"].as_array().unwrap().iter().any(|s| s["id"] == "sleeper"));
    assert!(unsafe { libc_kill(sleeper_pid, 0) } != 0, "removing a service stops it");

    // Shutdown stops what is running and removes the launcher's run file.
    env.lx_ok(&["start", "alpha"]);
    let alpha_pid = env.service("alpha")["pid"].as_u64().unwrap() as i32;
    let down = env.lx_ok(&["down"]);
    assert!(down.contains("LaunchX stopped"), "{down}");
    assert!(env.run_file("launchx").is_none());
    assert!(unsafe { libc_kill(alpha_pid, 0) } != 0, "shutdown stops services");
    if let Some(mut old) = env.daemon.take() {
        let _ = old.wait();
    }

    // The next launcher starts what the user left running (alpha), not what they stopped (beta).
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
    wait_for("alpha to come back", Duration::from_secs(15), || env.run_file("alpha").is_some());
    assert_eq!(env.service("beta")["status"], "stopped");
}
