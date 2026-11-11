//! Operating-system glue: the login environment, PATH lookup, opening URLs,
//! and registering the daemon with launchd (macOS) or systemd (Linux).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::home::Home;

const LAUNCHD_LABEL: &str = "local.launchx.daemon";
const MARKER: &str = "\0__LAUNCHX_ENV__\0";

/// The environment services start with: the user's login-shell environment,
/// so a daemon started by launchd or systemd still finds `~/.local/bin` and
/// friends. `LAUNCHX_INHERIT_ENV=1` uses this process's environment instead.
pub fn environment() -> Vec<(String, String)> {
    let inherited: Vec<(String, String)> = std::env::vars().collect();
    let mut env = if std::env::var("LAUNCHX_INHERIT_ENV").as_deref() == Ok("1") {
        inherited
    } else {
        login_environment().unwrap_or(inherited)
    };
    env.retain(|(key, _)| !key.starts_with("LAUNCHX") && !matches!(key.as_str(), "_" | "SHLVL" | "PWD" | "OLDPWD"));
    env
}

fn login_environment() -> Option<Vec<(String, String)>> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
    let script = format!("printf '{}'; /usr/bin/env -0", MARKER.replace('\0', "\\0"));
    let mut child = Command::new(&shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let bytes = reader.join().ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let (_, env) = text.split_once(MARKER)?;
    let pairs: Vec<(String, String)> =
        env.split('\0').filter_map(|item| item.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    pairs.iter().any(|(key, _)| key == "PATH").then_some(pairs)
}

pub fn lookup<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// Find an executable on a PATH string.
pub fn which(program: &str, path: &str) -> Option<PathBuf> {
    path.split(':').filter(|dir| !dir.is_empty()).map(|dir| Path::new(dir).join(program)).find(|candidate| executable(candidate))
}

pub fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

pub fn open_url(url: &str) -> anyhow::Result<()> {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let status = Command::new(opener).arg(url).stdout(Stdio::null()).stderr(Stdio::null()).status()?;
    if !status.success() {
        bail!("{opener} could not open {url}");
    }
    Ok(())
}

fn uid() -> u32 {
    // SAFETY: getuid(2) cannot fail.
    unsafe { libc::getuid() }
}

fn plist_path() -> PathBuf {
    crate::home::home_dir().join("Library/LaunchAgents").join(format!("{LAUNCHD_LABEL}.plist"))
}

fn unit_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| crate::home::home_dir().join(".config"));
    base.join("systemd/user/launchx.service")
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Register `exe daemon` to run now and at every login. Returns what was done.
pub fn install(exe: &Path, home: &Home) -> anyhow::Result<String> {
    let log = home.logs_dir().join("launchx.log");
    if cfg!(target_os = "macos") {
        let plist = plist_path();
        std::fs::create_dir_all(plist.parent().unwrap())?;
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>daemon</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ExitTimeOut</key><integer>60</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
  <key>EnvironmentVariables</key><dict><key>LAUNCHX_HOME</key><string>{home}</string></dict>
</dict>
</plist>
"#,
            exe = xml(&exe.display().to_string()),
            log = xml(&log.display().to_string()),
            home = xml(&home.root.display().to_string()),
        );
        std::fs::write(&plist, body).with_context(|| format!("writing {}", plist.display()))?;
        let domain = format!("gui/{}", uid());
        let _ = Command::new("launchctl").args(["bootout", &format!("{domain}/{LAUNCHD_LABEL}")]).stderr(Stdio::null()).status();
        let status = Command::new("launchctl").args(["bootstrap", &domain]).arg(&plist).status()?;
        if !status.success() {
            bail!("launchctl bootstrap failed for {}", plist.display());
        }
        Ok(format!("Registered with launchd: {}", plist.display()))
    } else {
        let unit = unit_path();
        std::fs::create_dir_all(unit.parent().unwrap())?;
        let body = format!(
            "[Unit]\nDescription=LaunchX launcher\n\n[Service]\nExecStart={} daemon\nEnvironment=LAUNCHX_HOME={}\nRestart=on-failure\nTimeoutStopSec=60\nStandardOutput=append:{}\nStandardError=append:{}\n\n[Install]\nWantedBy=default.target\n",
            exe.display(),
            home.root.display(),
            log.display(),
            log.display()
        );
        std::fs::write(&unit, body).with_context(|| format!("writing {}", unit.display()))?;
        let reload = Command::new("systemctl").args(["--user", "daemon-reload"]).status()?;
        let enable = Command::new("systemctl").args(["--user", "enable", "--now", "launchx.service"]).status()?;
        if !reload.success() || !enable.success() {
            bail!("systemctl could not enable {}", unit.display());
        }
        Ok(format!("Registered with systemd: {}", unit.display()))
    }
}

/// Undo `install`. Returns false when nothing was registered.
pub fn uninstall() -> anyhow::Result<bool> {
    if cfg!(target_os = "macos") {
        let plist = plist_path();
        if !plist.exists() {
            return Ok(false);
        }
        let _ = Command::new("launchctl").args(["bootout", &format!("gui/{}/{LAUNCHD_LABEL}", uid())]).status();
        std::fs::remove_file(&plist)?;
        Ok(true)
    } else {
        let unit = unit_path();
        if !unit.exists() {
            return Ok(false);
        }
        let _ = Command::new("systemctl").args(["--user", "disable", "--now", "launchx.service"]).status();
        std::fs::remove_file(&unit)?;
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
        Ok(true)
    }
}
