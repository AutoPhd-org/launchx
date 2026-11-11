//! `lx check <id>`: start a service on a spare port and test it against the
//! level it claims (contract §10.6).

use std::fs;
use std::time::{Duration, Instant};

use anyhow::bail;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::home::{Config, Home, RunFile};
use crate::supervisor::{self, HttpClient, Probe};
use crate::{manifest, platform, util};

struct Report {
    results: Vec<(String, bool, String)>,
}

impl Report {
    fn add(&mut self, name: impl Into<String>, ok: bool, note: impl Into<String>) -> bool {
        self.results.push((name.into(), ok, note.into()));
        ok
    }
}

async fn get(client: &HttpClient, url: &str, token: Option<&str>) -> Result<(u16, Bytes), String> {
    let mut request = Request::get(url);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let request = request.body(Empty::<Bytes>::new()).map_err(|e| e.to_string())?;
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let body = tokio::time::timeout(Duration::from_secs(5), response.into_body().collect())
        .await
        .map_err(|_| "body timed out".to_string())?
        .map_err(|e| e.to_string())?
        .to_bytes();
    Ok((status, body))
}

async fn run_cli(program: &str, args: &[&str], env: &[(String, String)], cwd: &std::path::Path) -> Result<(bool, String, String), String> {
    let path = platform::lookup(env, "PATH").unwrap_or("/usr/bin:/bin").to_string();
    let resolved = if program.contains('/') { cwd.join(program) } else { platform::which(program, &path).ok_or(format!("`{program}` is not on PATH"))? };
    let output = tokio::time::timeout(
        Duration::from_secs(90),
        tokio::process::Command::new(&resolved)
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| format!("`{program} {}` did not finish within 90s", args.join(" ")))?
    .map_err(|e| e.to_string())?;
    Ok((output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned()))
}

pub async fn check(home: &Home, config: &Config, id: &str, as_json: bool) -> anyhow::Result<bool> {
    let (services, _) = manifest::load_registry(home);
    let Some(service) = services.into_iter().find(|s| s.id == id) else { bail!("Unknown service '{id}'. Register it with `lx add <path>`.") };
    if let Some(error) = &service.error {
        bail!("{id} cannot start: {error}");
    }
    let def = service.def.clone();
    let level = def.level;
    let scratch = std::env::temp_dir().join(format!("lx-check-{}-{}", id, &util::token()[..8]));
    fs::create_dir_all(scratch.join("run"))?;
    let port = def.http.as_ref().map(|_| supervisor::free_port()).transpose()?;
    let token = util::token();
    let base = platform::environment();
    // Dependencies: borrow the running ones' addresses, if any.
    let deps: Vec<(String, String, String)> =
        def.requires.iter().filter_map(|dep| home.read_run(dep).map(|run| (dep.clone(), run.url, run.token))).collect();
    let launch = match supervisor::build_launch(&service, port, &token, &base, &deps, config) {
        Ok(launch) => launch,
        Err(message) => bail!("{id} cannot start: {message}"),
    };
    let client = supervisor::http_client();
    let mut report = Report { results: Vec::new() };
    let mut child = match supervisor::spawn(&launch) {
        Ok(child) => child,
        Err(message) => bail!(message),
    };
    let pid = child.id().unwrap_or(0);
    let mut output = String::new();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let collector = tokio::spawn(async move {
        let mut text = Vec::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_end(&mut text).await;
        }
        if let Some(err) = stderr.as_mut() {
            let _ = err.read_to_end(&mut text).await;
        }
        String::from_utf8_lossy(&text).into_owned()
    });

    let begun = Instant::now();
    let deadline = begun + Duration::from_secs(def.start_timeout);
    let mut ready = false;
    let mut health = Value::Null;
    let mut last_reason = String::from("no answer yet");
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            last_reason = supervisor::describe(status);
            break;
        }
        match (&def.http, port) {
            (Some(http), Some(port)) => match supervisor::probe(&client, id, port, &token, http, level).await {
                Probe::Healthy { .. } => {
                    ready = true;
                    if level >= 1 {
                        if let Ok((_, body)) = get(&client, &format!("http://127.0.0.1:{port}{}", http.health), Some(&token)).await {
                            health = serde_json::from_slice(&body).unwrap_or(Value::Null);
                        }
                    }
                    break;
                }
                Probe::Unhealthy(reason) => last_reason = reason,
            },
            _ if begun.elapsed() >= Duration::from_secs(1) => {
                ready = true;
                break;
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let what = match &def.http {
        Some(http) => format!("starts and answers {} within {}s", http.health, def.start_timeout),
        None => "starts".to_string(),
    };
    let started = report.add(what, ready, if ready { format!("{:.1}s", begun.elapsed().as_secs_f64()) } else { last_reason });

    if started {
        if level >= 1 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let alive = matches!(child.try_wait(), Ok(None));
            report.add("stays in the foreground", alive, if alive { "" } else { "the process exited after becoming ready" });
        }
        if let (Some(http), Some(port), true) = (&def.http, port, level >= 1) {
            let url = format!("http://127.0.0.1:{port}{}", http.health);
            let none = get(&client, &url, None).await.map(|r| r.0);
            report.add("rejects a request without the token", none == Ok(401), format!("got {}", show(&none)));
            let wrong = get(&client, &url, Some("wrong-token")).await.map(|r| r.0);
            report.add("rejects a wrong token", wrong == Ok(401), format!("got {}", show(&wrong)));
            let id_ok = health.get("id").and_then(Value::as_str) == Some(id);
            let version_ok = health.get("version").and_then(Value::as_str).is_some_and(|v| !v.is_empty());
            let status_ok = matches!(health.get("status").and_then(Value::as_str), Some("ok" | "degraded"));
            report.add("health has id, version and status", id_ok && version_ok && status_ok, compact(&health));

            if level >= 2 {
                let state_path = http.state.clone().unwrap_or_default();
                let state = get(&client, &format!("http://127.0.0.1:{port}{state_path}"), Some(&token)).await;
                let mut keys: Vec<String> = Vec::new();
                let note = match &state {
                    Ok((200, body)) => match serde_json::from_slice::<Value>(body) {
                        Ok(Value::Object(map)) => {
                            keys = map.keys().cloned().collect();
                            if String::from_utf8_lossy(body).contains(&token) { "contains the token".into() } else { format!("{} top-level keys", keys.len()) }
                        }
                        _ => "not a JSON object".into(),
                    },
                    Ok((code, _)) => format!("returned {code}"),
                    Err(e) => e.clone(),
                };
                report.add(format!("{state_path} is a JSON object without the token"), note.ends_with("top-level keys"), note);

                if let Some(cli) = &def.cli {
                    let run = RunFile {
                        id: id.to_string(),
                        pid,
                        url: format!("http://127.0.0.1:{port}"),
                        token: token.clone(),
                        ui: None,
                        version: None,
                        started_at: util::now_rfc3339(),
                    };
                    fs::write(scratch.join("run").join(format!("{id}.json")), serde_json::to_vec(&run)?)?;
                    let mut env = base.clone();
                    env.push(("LAUNCHX_HOME".into(), scratch.display().to_string()));
                    env.push(("LAUNCHX_SERVICE".into(), id.to_string()));
                    let cwd = service.cwd();
                    let state_cli = run_cli(cli, &["state"], &env, &cwd).await;
                    let (ok, note) = match &state_cli {
                        Ok((true, stdout, _)) => match serde_json::from_str::<Value>(stdout) {
                            Ok(Value::Object(map)) => {
                                let mut theirs: Vec<String> = map.keys().cloned().collect();
                                theirs.sort();
                                let mut ours = keys.clone();
                                ours.sort();
                                (theirs == ours, if theirs == ours { "same keys".to_string() } else { format!("keys {theirs:?} vs {ours:?}") })
                            }
                            _ => (false, "did not print a JSON object".into()),
                        },
                        Ok((false, _, stderr)) => (false, format!("failed: {}", stderr.lines().last().unwrap_or(""))),
                        Err(e) => (false, e.clone()),
                    };
                    report.add(format!("`{cli} state` prints the same top-level keys"), ok, note);
                    let version = health.get("version").and_then(Value::as_str).unwrap_or("").to_string();
                    let version_cli = run_cli(cli, &["--version"], &env, &cwd).await;
                    let (ok, note) = match &version_cli {
                        Ok((_, stdout, stderr)) => {
                            let text = format!("{stdout}{stderr}");
                            (!version.is_empty() && text.contains(&version), text.trim().to_string())
                        }
                        Err(e) => (false, e.clone()),
                    };
                    report.add(format!("`{cli} --version` matches health ({version})"), ok, note);
                }
                if let Some(events) = &http.events {
                    let ok = heartbeat(&client, &format!("http://127.0.0.1:{port}{events}"), &token).await;
                    report.add(format!("{events} streams and sends a heartbeat within 35s"), ok.is_ok(), ok.err().unwrap_or_default());
                }
            }
        }
    }

    // Stop it the way the launcher would.
    let stop = Duration::from_secs(def.stop_timeout);
    let stopped_at = Instant::now();
    let description = supervisor::terminate(&mut child, pid, stop).await;
    if level >= 1 && started {
        let clean = description == "exited with code 0";
        report.add(format!("exits 0 within {}s of SIGTERM", def.stop_timeout), clean, format!("{description} after {:.1}s", stopped_at.elapsed().as_secs_f64()));
    }
    output.push_str(&tokio::time::timeout(Duration::from_secs(2), collector).await.ok().and_then(Result::ok).unwrap_or_default());
    let _ = fs::remove_dir_all(&scratch);

    let passed = report.results.iter().filter(|r| r.1).count();
    let total = report.results.len();
    if as_json {
        let results: Vec<Value> = report.results.iter().map(|(name, ok, note)| json!({ "check": name, "ok": ok, "note": note })).collect();
        println!("{}", serde_json::to_string_pretty(&json!({ "id": id, "level": level, "passed": passed, "total": total, "results": results }))?);
    } else {
        println!("lx check {id} (claims level {level})");
        for (name, ok, note) in &report.results {
            let note = if note.is_empty() { String::new() } else { format!("  — {note}") };
            println!("  {} {name}{note}", if *ok { "✓" } else { "✗" });
        }
        println!("{passed} of {total} checks passed.");
        if passed < total {
            let lines: Vec<&str> = output.lines().collect();
            if !lines.is_empty() {
                println!("\nLast output of {id}:");
                for line in &lines[lines.len().saturating_sub(20)..] {
                    println!("  {line}");
                }
            }
        }
    }
    Ok(passed == total)
}

fn show(result: &Result<u16, String>) -> String {
    match result {
        Ok(code) => code.to_string(),
        Err(e) => e.clone(),
    }
}

fn compact(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 160 { format!("{}…", &text[..text.char_indices().nth(160).map(|(i, _)| i).unwrap_or(text.len())]) } else { text }
}

async fn heartbeat(client: &HttpClient, url: &str, token: &str) -> Result<(), String> {
    let request = Request::get(url).header("authorization", format!("Bearer {token}")).body(Empty::<Bytes>::new()).map_err(|e| e.to_string())?;
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request)).await.map_err(|_| "timed out".to_string())?.map_err(|e| e.to_string())?;
    if response.status() != 200 {
        return Err(format!("returned {}", response.status()));
    }
    let mut body = response.into_body();
    let frame = tokio::time::timeout(Duration::from_secs(35), body.frame()).await.map_err(|_| "no data within 35s".to_string())?;
    match frame {
        Some(Ok(_)) => Ok(()),
        _ => Err("stream ended".into()),
    }
}
