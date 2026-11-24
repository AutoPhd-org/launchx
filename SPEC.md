# LaunchX Service Contract

Version 0.1 — draft, 2026-11-17

## 1. What this is

LaunchX is a launcher for self-hosted services that run as ordinary processes on one person's machine. This document defines the contract between a **service** and the **launcher**. A service that follows it can be started, stopped, observed and opened from one place, and can depend on other services.

The contract rests on four principles:

1. **Open state.** A service's backend state is readable as JSON by anyone holding its token. The GUI has no private channel.
2. **The CLI is a peer.** Everything the GUI can do, a command can do.
3. **The GUI lives in the browser.** A service ships no native window; its UI is a web page served by its own backend.
4. **One owner.** The launcher starts every service, hands it its port and credentials, and is the one place to see what is running.

The words MUST, SHOULD and MAY are used as in RFC 2119.

## 2. Terms

| Term | Meaning |
|---|---|
| Service | One long-running process, identified by an `id`. |
| Launcher | The daemon (`lx daemon`), its CLI `lx`, and its browser UI. One executable. |
| Manifest | A `launchx.toml` file that describes one or more services. |
| Managed mode | The service was started by the launcher. The `LAUNCHX` environment variable is set. |
| Standalone mode | The service was started any other way. This contract does not constrain it. |
| Gateway | The launcher's reverse proxy. It is the only way a browser reaches a managed service. |
| Direct URL | `http://127.0.0.1:<port>` of a managed service. Programs use it; browsers do not. |

## 3. Conformance levels

| Level | Name | The service must | What it gets |
|---|---|---|---|
| 0 | Described | Have a manifest. No code changes. | Start, stop, restart, logs, dependency ordering. |
| 1 | Managed | Also follow the process contract (§5) and, if it speaks HTTP, the HTTP contract (§6). | A launcher-assigned port and token, health reporting, a protected UI at a stable URL. |
| 2 | Open | Also expose its state (§7) and ship a CLI (§8). | Scripts, agents and other services can read it and drive it. |

Level 0 exists so the launcher is useful for software that has never heard of this contract. A manifest declares its level; `lx check <id>` verifies the claim (§10.6).

## 4. Manifest

A manifest is a TOML file named `launchx.toml`. A project ships it at its root; a user can also write one by hand for third-party software. One file MAY define several services.

```toml
launchx = "0.1"

[service.vmc]
name        = "Machine Console"
description = "Live CPU, RAM, GPU and disk usage of the servers in your SSH config"
level       = 2
run         = ["vmc", "serve"]
cli         = "vmc"
needs       = ["ssh"]

[service.vmc.http]
ui    = "/"
state = "/api/state"
```

### 4.1 Service id

The id is the table key (`vmc` above). It becomes a hostname, so it MUST match `^[a-z][a-z0-9-]{0,30}$`. The id `launchx` is reserved.

### 4.2 Service fields

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | required | Display name. |
| `description` | string | none | One line. |
| `level` | 0, 1 or 2 | 0 | Conformance level the service claims. |
| `run` | array of strings | required | Command and arguments. It MUST run in the foreground. |
| `cwd` | string | manifest directory | Working directory. |
| `env` | table | empty | Extra environment variables. |
| `requires` | array of ids | empty | Services that must be running first (§9). |
| `needs` | array of strings | empty | Executables that must be on `PATH` before start. |
| `cli` | string | none | Name of the service's CLI executable (§8). |
| `autostart` | boolean | false | Start when the launcher starts. |
| `restart` | `"no"`, `"on-failure"`, `"always"` | `"on-failure"` | What to do when the process exits (§10.3). |
| `start_timeout` | seconds | 30 | Time allowed to become ready. |
| `stop_timeout` | seconds | 10 | Time allowed to exit after SIGTERM. |
| `homepage`, `icon` | string | none | Shown in the launcher UI. `icon` is a path relative to the manifest. |

### 4.3 HTTP fields

The `[service.<id>.http]` table is omitted for a service with no HTTP interface.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `health` | path | `/api/health` | Health endpoint (§6.3). |
| `ui` | path | none | Entry page of the browser UI. Absent means the service has no UI. |
| `state` | path | none | State document (§7.1). Required at level 2. |
| `events` | path | none | Change stream (§7.2). |
| `port` | integer | launcher-assigned | Preferred port. The launcher MAY assign another if it is taken. |

### 4.4 Placeholders

For level 0 software that cannot read the environment, `{port}` and `{host}` in `run` and `env` values are replaced with the assigned values:

```toml
launchx = "0.1"

[service.notes]
name = "Notes"
run  = ["some-notes-app", "--listen", "{host}:{port}"]

[service.notes.http]
ui     = "/"
health = "/healthz"
```

There is no `{token}` placeholder: a token in the argument list would be visible in the process list.

### 4.5 Registration

`lx add <file-or-directory>` registers a manifest by linking it into `$LAUNCHX_HOME/services/`. Relative paths in a manifest resolve against the directory the real file is in. An installer SHOULD run `lx add` when `lx` is on `PATH`.

### 4.6 Private configuration

A service's repository and manifest MUST NOT need private data to work. Names of a person's machines, organisation or accounts belong in a configuration file the service reads from a documented location outside its repository (for example `~/.config/<service>/`). The manifest stays generic and public; the private file stays on the machine. vmc's site profile (Appendix A) is the worked example.

### 4.7 Agent fields

The optional `[service.<id>.agents]` table says what the service offers LLM agents beyond its CLI and state. Paths are relative to the manifest.

| Key | Type | Meaning |
|---|---|---|
| `skills` | path | One skill folder (a folder holding `SKILL.md`) or a folder of them, in the [Agent Skills](https://agentskills.io) format. A skill's `name` and `description` come from its front matter. The name `launchx` is reserved. |
| `docs` | path | A document for agents: the shape of the state document and the actions (§7). |

```toml
[service.rmd.agents]
skills = "plugins/schedule-everything/skills"
```

The manifest says what is on offer. Whether agents are told about it is the user's choice (§10.7).

### 4.8 Memory fields

The optional `[service.<id>.memory]` table says what the service would like to do with the user's shared memory (§12), and why. It grants nothing: it is shown to the user next to the switch they set.

| Key | Type | Meaning |
|---|---|---|
| `wants` | `"read"` or `"write"` | The access the service asks for. |
| `reason` | string | One line the user sees, for example "Keeps your schedule page current". |

```toml
[service.rmd.memory]
wants  = "write"
reason = "Keeps your schedule page current and suggests tasks that fit your profile"
```

### 4.9 Install fields

The optional top-level `[install]` table says how to build and install the project from a checkout of its repository, so that `lx install` and `lx update` can do it (§10.8). LaunchX reads it only from the `launchx.toml` at the root of a checkout; a copy of the manifest elsewhere may carry it and it is ignored there.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `run` | array of strings | none | The installer: command and arguments, run in the checkout. |
| `uninstall` | array of strings | none | Undoes what `run` did (commands, launch agents, installed files) and keeps the user's data. |
| `needs` | array of strings | empty | Executables that must be on `PATH` before `run`, for example `cargo`. |
| `data` | array of strings | empty | Where the app keeps the user's data. Shown on uninstall, and what to copy when moving to another machine. LaunchX never deletes it. |

```toml
[install]
run       = ["./install.sh", "--yes"]
uninstall = ["./uninstall.sh"]
needs     = ["cargo"]
data      = ["~/.schedule_management"]
```

An installer:

- MUST NOT prompt. Its standard input is closed.
- MUST be safe to run again over an earlier install, since that is how updates run, and MUST keep the user's data.
- MAY register a manifest of its own with `lx add`, for example one written next to the files it installed. When it has registered none of the services the checkout's manifest defines, LaunchX registers the checkout's manifest.

It runs with the user's login environment, the directory of `lx` first on `PATH`, `LAUNCHX_HOME`, and `LAUNCHX_INSTALL` set to `install` or `update`.

Without `run`, installing only registers the checkout's manifest. That suits a project that runs from its checkout.

## 5. Process contract (level 1)

### 5.1 Foreground

The process started by `run` is the service. It MUST NOT fork into the background or detach. When it exits, the service has stopped.

### 5.2 Environment

The service inherits the user's login environment, then the manifest's `env`, then these:

| Variable | Value |
|---|---|
| `LAUNCHX` | The contract version, `0.1`. Its presence means managed mode. |
| `LAUNCHX_SERVICE` | The service id. |
| `LAUNCHX_HOST` | Address to bind. `127.0.0.1` unless the user changed it. |
| `LAUNCHX_PORT` | Port to listen on. |
| `LAUNCHX_TOKEN` | The bearer token the service must require (§6.2). |
| `LAUNCHX_URL` | The service's browser URL, for example `http://vmc.localhost:7600`. Use it to build links shown to people. |
| `LAUNCHX_DEP_<ID>_URL` | Direct URL of a required service (§9). `<ID>` is the id in upper case with `-` replaced by `_`. |
| `LAUNCHX_DEP_<ID>_TOKEN` | That service's token. |
| `LAUNCHX_MEMORY_URL` | The shared memory's API (§12.4), for example `http://127.0.0.1:7600/api/memory`. |
| `LAUNCHX_MEMORY_TOKEN` | The service's own memory token (§12.3). Given to every managed service, with or without HTTP. |

The port and token of a service stay the same across restarts until the user resets them.

### 5.3 Ready

A service with a health endpoint is ready when that endpoint answers 200. A service without one is ready when its process has stayed up for one second. It MUST become ready within `start_timeout`.

### 5.4 Stop

On SIGTERM the service MUST finish or abandon in-flight work and exit with code 0 within `stop_timeout`. After that the launcher sends SIGKILL. Any other exit is a failure and is handled by the restart policy.

### 5.5 Logs

The service writes logs to stdout and stderr, one event per line. It MUST NOT log tokens. The launcher captures and keeps them; the service SHOULD NOT keep log files of its own in managed mode.

### 5.6 No surprises

In managed mode the service MUST NOT open a browser, prompt on the terminal, or start a second copy of itself. The launcher runs at most one instance per id. A service that cannot share its data with another copy of itself MUST guard that itself (a lock) and exit non-zero with a one-line reason.

## 6. HTTP contract (level 1)

### 6.1 Listen

The service MUST listen on exactly `LAUNCHX_HOST:LAUNCHX_PORT`, in plain HTTP. If it cannot bind, it exits non-zero.

### 6.2 Auth

Every request, to every path, MUST carry `Authorization: Bearer <LAUNCHX_TOKEN>`. Anything else gets `401` with no detail. The service:

- SHOULD compare the token in constant time.
- MUST NOT accept the token in a query string or cookie.
- MUST NOT send CORS headers that allow other origins.

This is the whole of a managed service's security duty. It needs no login page, session, cookie, Host check or CSRF defence; the gateway (§10.4) does those once for every service.

### 6.3 Health

`GET <http.health>` returns `200` and a JSON object:

```json
{
  "id": "vmc",
  "version": "0.3.1",
  "status": "ok",
  "summary": "11 of 12 machines online"
}
```

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | The service id. The launcher treats a mismatch as "wrong process on this port". |
| `version` | yes | The service's version string. |
| `status` | yes | `"ok"`, or `"degraded"` when the service works but something it relies on does not. |
| `summary` | no | One short line the launcher shows on the service's card. |
| `detail` | no | One line explaining a `degraded` status. |

The endpoint MUST answer within 2 seconds and have no side effects. Before the service is ready it returns `503`. A non-200 answer or a timeout counts as unhealthy.

At level 0, any `2xx` from `http.health` counts as healthy and the body is ignored.

### 6.4 UI

The UI is served by the same listener, starting at `http.ui`. The page:

- MUST reach its backend with same-origin relative URLs. It never hard-codes a host or port.
- MUST NOT handle the token. The gateway adds it to every request.
- MUST work from the root of its origin. Each service gets its own origin, so no path prefix is involved.
- MUST NOT use paths under `/.launchx/`, which belong to the gateway.

The service's `GET` and `HEAD` requests MUST NOT change state. Browser sessions are `SameSite=Lax`, so a cross-site link can make a signed-in `GET` reach the service; only other methods are guarded by the gateway's Origin check.

Server-sent events and WebSockets are allowed; the gateway passes both through.

## 7. Open state (level 2)

### 7.1 The state document

`GET <http.state>` returns `200` and one JSON object describing the service's current state. It is:

- **Complete.** Everything the service's GUI displays MUST come from this document or from other documented endpoints behind the same token. A large collection MAY appear as a count and a link, for example `{"count": 412, "href": "/api/reports"}`.
- **Free of secrets.** No tokens, passwords or API keys, including ones the service stores for the user.
- **Cheap.** Read-only, no side effects. It SHOULD answer within 1 second by serving the last known snapshot instead of waiting on slow sources.
- **Cacheable.** It SHOULD return an `ETag` and honour `If-None-Match`.
- **Stable.** Its shape is documented. Within a major version, fields are only added.

### 7.2 Change events

A level 2 service SHOULD offer `GET <http.events>` as a server-sent event stream. It sends `event: change` whenever the state document changes; clients then fetch the document again. The `data` field is a JSON object and MAY name what changed:

```
event: change
data: {"keys": ["tasks"]}
```

The stream sends a comment line as a heartbeat at least every 30 seconds.

### 7.3 Actions

Every change the GUI can make MUST be available as a documented HTTP request behind the same token. The shape is the service's choice: resource endpoints and a single command endpoint are both fine. There are no GUI-only endpoints.

### 7.4 The test

A service meets this section when a third party, given only its documentation and its token, could rebuild its GUI.

## 8. CLI (level 2)

A level 2 service ships a command-line program and names it in `cli`.

### 8.1 Reserved subcommands

| Command | Meaning |
|---|---|
| `<cli> serve` | Run the service in the foreground under §5. This SHOULD be what `run` calls. |
| `<cli> state` | Print the state document (§7.1) as JSON on stdout. |
| `<cli> --version` | Print the same version string the health endpoint reports. |

### 8.2 Parity

Every action in the GUI has a command. The CLI and the GUI are peers: a change made through one MUST show up in the other without a restart. Either architecture satisfies this:

- The CLI calls the running service's HTTP API, which it finds through the run file (§10.2). It reads its own id from `LAUNCHX_SERVICE` when set, and otherwise uses a documented default.
- The CLI and the service both work on the same store, and the service watches the store.

### 8.3 Conventions

- Exit code 0 on success, non-zero on failure.
- Data on stdout, diagnostics on stderr.
- Commands that read SHOULD offer `--json`.
- No prompts when stdin is not a terminal.

### 8.4 Convenience commands

A command whose job is "start it and open the page" (for example `rmd web`, or `vmc` with no arguments) SHOULD, when a launcher is running, hand over to `lx open <id>`. When none is running it behaves as it always did.

### 8.5 Agents

This section is not normative. A CLI with `--json` and an open state document are what let an AI agent use a service without scraping a page. Skills built on the CLI (§4.7) are a third client beside the GUI and the person at the terminal. The user decides which services agents are told about (§10.7).

## 9. Dependencies

`requires = ["b"]` in service `a` means:

- **Start.** `b` must be running and healthy before `a` is started. Starting `a` starts `b` first.
- **Credentials.** `a` receives `LAUNCHX_DEP_B_URL` and `LAUNCHX_DEP_B_TOKEN`. A service gets credentials only for services it declares.
- **Stop.** Stopping `b` stops `a` first. The launcher names the services this includes before it does it.
- **Failure.** If `b` fails while `a` runs, `a` is left running and shown as degraded. `a` MUST tolerate `b` being unreachable. Because `b` keeps its port and token, `a` needs no restart when `b` returns.
- **Errors.** A cycle or an unknown id is a manifest error. The affected services cannot start, and the message names the id.

`needs` lists executables rather than services. The launcher checks them before start and names the missing one.

Version constraints between services are not part of 0.1.

## 10. Launcher

### 10.1 Home directory

`$LAUNCHX_HOME`, by default `~/.launchx`, mode `0700`:

```
config.toml          launcher settings: gateway_port (7600), backoff_unit_ms (1000),
                     agent_skill_dirs (["~/.claude/skills", "~/.agents/skills"])
services/            registered manifests (links or files)
run/<id>.json        one per running service, mode 0600
logs/<id>.log        captured output, rotated
agents/skills/       skills published to agents (§10.7)
memory/              the user's shared memory (§12.1)
sources/<name>/      checkouts that lx install cloned (§10.8)
installs.json        what was installed from where (§10.8)
state.json           assigned ports and tokens, which services the user left running,
                     which are discoverable by agents and what each may do with
                     memory; mode 0600
```

The launcher runs services with the `PATH` of the user's login shell, not the bare one that launchd or systemd provide.

### 10.2 Run files

After a service becomes ready the launcher writes `run/<id>.json`, and removes it when the service stops:

```json
{
  "id": "vmc",
  "pid": 4242,
  "url": "http://127.0.0.1:53001",
  "token": "a-long-random-string",
  "ui": "http://vmc.localhost:7600/",
  "version": "0.3.1",
  "started_at": "2026-11-10T16:40:02Z"
}
```

This is how a CLI, a script or an agent finds a running service. A reader MUST treat a file whose `pid` is not alive as absent. The launcher writes one for itself as `run/launchx.json`.

### 10.3 Lifecycle

| Status | Meaning |
|---|---|
| `stopped` | Not running, by request or never started. |
| `waiting` | Waiting for a required service to become healthy. |
| `starting` | Process started, not yet ready. |
| `running` | Ready. Carries a health of `ok`, `degraded` or `unhealthy`. |
| `stopping` | SIGTERM sent. |
| `failed` | Start timed out, a `needs` check failed, or restarts were exhausted. |

- **Health checks.** Every 250 ms while starting and every 5 seconds while running. Three failures in a row make a running service `unhealthy`. An unhealthy process is reported, not restarted.
- **Remembered services.** The launcher records which services the user started and has not stopped, and starts them again when it starts, together with those marked `autostart`. Stopping a service removes it from the record; shutting the launcher down does not.
- **Restarts.** On exit, according to `restart`. Delays start at 1 second and double up to 60. The delay resets after 60 seconds of running. Five failures in a row end in `failed`.

### 10.4 Gateway

The gateway listens on `127.0.0.1:7600` by default.

| Host | Goes to |
|---|---|
| `launchx.localhost:7600` | The launcher's own UI and API. |
| `<id>.localhost:7600` | The UI of service `<id>`. |
| anything else | Rejected. |

Names under `.localhost` resolve to loopback (RFC 6761), so no DNS or hosts-file setup is needed. Programs do not use the gateway; they use the direct URL and token from the run file.

The gateway MUST:

- **Authenticate the browser.** `lx open` produces a one-time link that starts a session. Each origin gets its own host-only, `HttpOnly`, `SameSite=Lax` cookie (Lax, not Strict, because WebKit counts each `<id>.localhost` as a separate site, so the sign-in hop between origins is a cross-site navigation); the gateway carries the session to a new service origin with a one-time ticket under `/.launchx/`: a page request to a service without a session is redirected to `launchx.localhost/.launchx/authorize`, which, when the launcher session is valid, sends back a ticket for that origin. Sessions are signed with a secret kept in `state.json`, so they survive a daemon restart. Requests without any session get a `401` page that says to run `lx open`. Loopback is reachable by every local user, which is why a session is required.
- **Route by Host.** A request whose Host is not one of the names above is rejected. This is what defeats DNS rebinding.
- **Check Origin.** A request that is not `GET` or `HEAD` MUST carry an `Origin` equal to the origin it is addressed to.
- **Swap credentials.** Remove any client `Authorization` header and the launcher's cookie, then add `Authorization: Bearer <service token>`. Set `X-Forwarded-Host` and `X-Forwarded-Proto`.
- **Stream.** Pass responses through unbuffered, support WebSocket upgrades, and apply no idle timeout to streams.
- **Explain a stopped service.** When the service is not running, show its status and a Start button instead of a connection error.

A level 0 service is proxied the same way, but its own port is not token-protected. The launcher UI marks it so.

### 10.5 The launcher's own interface

The launcher is itself a level 2 service with the id `launchx`. A platform app (LaunchX.app on macOS) is only a control window onto it: it shows the launcher's own page in a web view signed in with `/api/link`, its menu calls the same API, and it keeps no state of its own. Service pages are not shown in the app; Open hands a one-time `/api/link` URL to the default browser, in keeping with principle 3.

```
GET  /api/health
GET  /api/state
GET  /api/events
POST /api/services/{id}/start
POST /api/services/{id}/stop
POST /api/services/{id}/restart
POST /api/services/{id}/agents         body {"discoverable": true|false} (§10.7)
POST /api/services/{id}/memory         body {"access": "none"|"read"|"write"} (§12.3)
POST /api/memory/agents                body {"access": "none"|"read"|"write"} (§12.6)
*    /api/memory/…                     the shared memory (§12.4)
GET  /api/services/{id}/logs?tail=200
GET  /api/agents                       what agents are told (§10.7)
POST /api/install/plan                 body {"source", "name"?}: fetch and say what installing would do (§10.8)
POST /api/install                      body {"source", "name"?}: install, as a job
POST /api/packages/{name}/update       body {"force"?}: update, as a job
POST /api/packages/{name}/uninstall    remove, as a job
POST /api/packages/check               look for updates now
GET  /api/install/log?tail=200         output of the current or last job
POST /api/link                         one-time sign-in link; body {"service": id?, "next": path?}
POST /api/reload
POST /api/shutdown                     bearer token only
```

Its state document:

```json
{
  "launcher": { "version": "0.1.0", "gateway": "http://launchx.localhost:7600" },
  "services": [
    {
      "id": "vmc",
      "name": "Machine Console",
      "level": 2,
      "status": "running",
      "health": "ok",
      "summary": "11 of 12 machines online",
      "version": "0.3.1",
      "pid": 4242,
      "port": 53001,
      "ui": "http://vmc.localhost:7600/",
      "requires": [],
      "since": "2026-11-10T16:40:02Z",
      "restarts": 0,
      "last_exit": null,
      "agents": { "discoverable": false, "skills": [] },
      "memory": { "access": "read", "wants": "read", "reason": "Shows the machines your profile names" }
    }
  ],
  "memory": { "agents": "none", "pages": 6, "dir": "/Users/me/.launchx/memory" }
}
```

Its CLI:

| Command | Does |
|---|---|
| `lx daemon` | Run the launcher in the foreground. |
| `lx up` / `lx down` | Start or stop the launcher daemon. `lx up` registers `lx daemon` with launchd (`local.launchx.daemon`) or systemd (`launchx.service`), the only thing LaunchX registers with the operating system; `lx down` removes it. |
| `lx ls` | List services with status and health. |
| `lx start <id>` / `lx stop <id>` / `lx restart <id>` | Control one service, and its dependencies as §9 describes. |
| `lx open [id]` | Open the launcher, or a service's UI, in the browser with a session. |
| `lx logs <id> [-f]` | Show or follow captured output. |
| `lx add <path>` / `lx rm <id>` | Register or unregister a manifest. |
| `lx install [<source>]` | Install an app from a repository or folder, or with no source, list what is installed (§10.8). |
| `lx update [<name>…]` | Update installed apps, all of them by default; `--check` only reports. |
| `lx uninstall <name>` | Remove an installed app, keeping its data. |
| `lx check <id>` | Verify the service against its declared level. |
| `lx state` | Print the launcher's state document. |
| `lx agents` | List the services agents can discover (§10.7). |
| `lx agents on <id>…` / `lx agents off <id>…` | Make services discoverable by agents, or hide them. |
| `lx memory` | The shared memory (§12.7): `ls`, `show`, `write`, `append`, `rm`, `history`, `log`, `search`, `lint`, `index`, `open`. |
| `lx memory access [<id>\|agents <none\|read\|write>]` | Who may use memory, or set it. |

Every command that reads takes `--json`.

### 10.6 `lx check`

`lx check <id>` starts the service on a spare port with a fresh token and reports each of these as pass or fail. It runs the service's CLI with `LAUNCHX_HOME` pointing at a scratch directory holding a run file for the test copy, and `LAUNCHX_SERVICE` set to the id.

| Level | Checks |
|---|---|
| 1 | Stays in the foreground. Binds the assigned host and port. Rejects a request with no token and one with a wrong token. Health answers in time with the right `id`. Exits 0 on SIGTERM within `stop_timeout`. |
| 2 | State endpoint returns a JSON object and no value that equals the token. `<cli> state` prints the same top-level keys. `<cli> --version` matches health. If `events` is declared, the stream opens and sends a heartbeat. |

### 10.7 Agent discovery

Each service has a switch, **discoverable by agents**, that the user sets on its card in the launcher's page, in the app's menu, or with `lx agents on|off <id>`. It is off for every service until the user turns it on, and the launcher keeps it in `state.json`. Unregistering a service forgets it.

For each service that is switched on, the launcher:

- **Lists it** in `lx agents` and `GET /api/agents`, with its id, name, description, level, status, CLI, `ui`, state and event paths, run file, `docs`, and each skill's name, description and path. Services that are switched off are not listed.
- **Publishes its skills** (§4.7) into each folder in `agent_skill_dirs` that exists. A folder that does not exist is skipped, not created.
- **Names it in the `launchx` skill**, which the launcher writes while at least one service is switched on. The skill tells an agent which apps it may use and how: each app's CLI, `<cli> state`, run file and skills. It asks the agent to leave every other app alone unless the user names it.

Every published skill is a link from the agent's folder to `$LAUNCHX_HOME/agents/skills/<name>`. The launcher removes only links that point there, so switching everything off leaves nothing of LaunchX's in an agent's folder. An entry it did not make is never replaced; the clash is reported in the state document's `errors`, as is a skill name two services share (the first keeps it).

The switch decides what agents are told, not what they can reach. An agent runs as the user, so it can still read run files and call a service it was not told about (§11).

### 10.8 Installing, updating and removing apps

`lx install <source>` installs an app. The source is a git URL, `owner/repo` for a GitHub repository, or a folder on this machine.

- **A URL** is cloned into `$LAUNCHX_HOME/sources/<name>`, where the name is the repository's. LaunchX owns that checkout: it moves it forward to update and deletes it on uninstall.
- **A folder** is installed where it is. LaunchX never pulls, changes or deletes it. This is how to install something that is not published, or a working copy.

LaunchX then reads the checkout's `launchx.toml`, shows its services, the installer command (§4.9) and where the app keeps data, and asks before going on (`--yes` skips the question). It checks `needs`, stops the app's running services, runs the installer, registers the app (§4.9), records it, and starts again what it stopped. Installing an app that is already installed updates it.

A cloned app follows the repository's releases. A **release** is a tag of the form `v1.2.3` or `1.2`, with two to four numbers and an optional `v`; a GitHub release creates one. Pre-releases such as `v1.3.0-rc.1` and other tags are not releases. When the repository has releases, LaunchX installs the newest one, and only a newer release is an update: commits pushed between releases are not. When it has none, LaunchX follows the default branch, and every new commit is an update, until the first release. A checkout installed ahead of the newest release (from the branch, before the repository had releases) stays where it is until a release beyond it. A release deleted from the repository stops counting at the next fetch.

`lx update [<name>…]` fetches each cloned app and, when there is a newer release (or, without releases, new commits on its branch), checks it out and runs its installer again (`--force` runs it anyway). An app installed from a folder is reinstalled from the folder as it is. Running services are stopped first and started again after. `lx update --check` fetches and reports without installing anything. The launcher also checks at start and every twelve hours, without installing, and shows the apps that have updates.

`lx uninstall <name>` stops the app's services, runs its `uninstall` command, unregisters its manifests, deletes the checkout if LaunchX cloned it, and forgets it. The app's data stays, and the command says where. A service id works in place of the name.

`installs.json` records each app: its source, checkout, whether LaunchX cloned it, its services, the commit installed and the release it is (`version`), its `data` paths, and when it was installed and updated. After a check it also records `behind`, the commits an update would bring, `available`, the newer release when there is one, and `checked_at`.

The launcher's page does the same: an Install button takes a source and shows the plan before installing, and each installed app's card has Update and Uninstall. On the page each of these runs as a job whose output streams to the log panel. One install, update or uninstall runs at a time, from the page or the command line.

## 11. Security model

| Threat | Handled by |
|---|---|
| Another local user connecting to a service's port | The service's bearer token (§6.2). |
| Another local user connecting to the gateway | The gateway's browser session (§10.4). |
| A web page sending requests to localhost | The gateway's Origin check; no CORS on services. |
| DNS rebinding | The gateway routes by Host; services require a token the attacker's page cannot have. |
| One service reaching into another | Tokens are per service; only declared dependencies receive one (§9). |
| A service reading or changing memory the user did not allow | The service's memory token, and the user's switch, checked on every request (§12.3). |
| One service using memory under another's name | Memory tokens are per service and never handed to dependents; writes are logged under the caller's id, whatever it claims. |
| A lost update to a shared page | Every update names the revision it was based on; a stale one is refused (§12.5). |
| Other devices on the network | Everything binds to loopback by default. |
| Installing runs code from a repository | LaunchX installs only from a source the user names, shows the installer command and asks first. The page's install API needs the launcher session, like every other action. |

Out of scope: a hostile process running as the same user. It can read the run files, as it can read everything else the user owns. This includes an AI agent the user runs: hiding a service from agents (§10.7) keeps it out of what they are told, and a well-behaved agent stays away, but only running the agent as another user, or in a sandbox, keeps it out. The same holds for memory: its files are the user's, so a service that reads them directly, rather than through the API, is not stopped by the switch.

Memory is read by agents, so it is a path for prompt injection: a service with write access could write instructions where an agent will read them. The memory skill (§12.6) tells agents to treat pages as information, not instructions, and every page and log entry says who wrote it. Give write access to services you trust with that.

## 12. Shared memory

The launcher keeps one memory for the user: a small wiki of pages about the person (who they are, how they like to work, their schedule, resume and projects) that their services and agents read and write together, so that the person does not have to tell each of them the same things. The user decides, for each service and for their agents, whether it may read memory, read and write it, or neither.

Its design follows the components of agent memory: a log of every write (the transcript), linked pages (the representation), and a short index meant to be loaded at the start of every session (the injected context).

### 12.1 Store

`$LAUNCHX_HOME/memory`, mode `0700`:

```
pages/<page>.md        one page per topic; <page> may hold '/' (projects/thesis)
history/<page>/        the versions writes replaced: <time>-<rev>.md, the last 20
log.jsonl              every write: time, by, role (user or app), action, page, rev, base, note
```

A page is Markdown with a front matter block the launcher writes:

```markdown
---
title: Schedule
summary: Weekly routine and the deadlines that matter beyond one app
updated: 2026-11-17T18:02:11Z
updated_by: rmd
---

- Writing block 9:00–12:00, Monday to Friday
- 1:1 with [[people/ann]] on Tuesdays at 11:00
```

- **Page names** are one to four parts of lowercase letters, digits and `-`, separated by `/`, each at most 48 characters.
- **Links** between pages are written `[[page]]` or `[[page|label]]`.
- **The summary** is one line of at most 300 characters. The index (§12.4) is made of summaries, so a page without one is listed by its title only.
- **Revisions.** A page's `rev` is the first 12 hex digits of the SHA-256 of its file. A hand edit changes it too.
- **Limits.** A page holds at most 64 KB, and memory at most 1000 pages.

The files are the user's: they may read and edit them with any editor. The log and history record only writes made through the launcher.

### 12.2 Well-known pages

So that services agree on where things go without knowing about each other:

| Page | Holds |
|---|---|
| `profile` | Who the user is: name, role, field, institution, time zone, languages. |
| `preferences` | How the user likes things done: working hours, tone, formats, tools. |
| `schedule` | The shape of the user's time: routine, fixed commitments, deadlines that matter beyond one app. Not a copy of a task list. |
| `resume` | Education, positions, publications, awards and skills. |
| `projects` | What the user is working on, one line each, linking to `projects/<name>`. |
| `people/<name>` | People the user works with. |

A service SHOULD NOT copy into memory what its own state document already shows (§7.1). Memory holds what stays true for a while and is useful to other services; live data is read from the service that has it.

### 12.3 Access

The user sets one of `none`, `read` or `write` for each service, on its card in the launcher's page, in the page's Memory view, or with `lx memory access <id> <level>`. It is `none` until the user changes it, and a manifest cannot change it (§4.8 only asks). `write` includes reading.

Each managed service receives `LAUNCHX_MEMORY_URL` and its own `LAUNCHX_MEMORY_TOKEN` (§5.2). The token stays the same across restarts, identifies the service to the memory API, and opens nothing but `/api/memory`. It is not the service's `LAUNCHX_TOKEN`, which its dependents hold, so no service can use memory under another's name. The launcher checks the user's switch on every request, so a change applies at once, without a restart.

A request beyond the service's access gets `403` with a message the service may show the user, and the field `"access"`. `GET /api/memory/me` always answers, so a service can tell the user that memory is off and where to turn it on.

### 12.4 API

Programs call the launcher's direct URL with a bearer token: a service its memory token, `lx` and the user's own scripts the launcher's token. The launcher's page uses the same API with its session. Bodies are JSON. Paths are relative to `/api/memory`.

| Request | Needs | Does |
|---|---|---|
| `GET /me` | — | `{"role": "app", "id": "rmd", "access": "read"}` |
| `GET /` | read | Every page without its body (`page`, `title`, `summary`, `rev`, `size`, `updated`, `updated_by`, `links`), the well-known pages not yet written (`suggested`), `count`, a `rev` of the whole listing, and `limits`. |
| `GET /index` | read | `{"markdown": …, "pages": n, "truncated": bool}`: one line per page, well-known pages first, at most 200 lines and 25 KB. Meant to be put into a model's context as it is. |
| `GET /pages/<page>` | read | The page: the fields above and `body` and `text` (the whole file). `?rev=<rev>` returns an earlier version. |
| `PUT /pages/<page>` | write | Create or update (§12.5). Body: `text` (front matter allowed) or `body`, optional `title`, `summary`, `note`, and `base`. |
| `POST /append/<page>` | write | Add `text` to the end of the page, creating it (with `title`) if needed. No `base`: an append cannot lose anyone's change. |
| `DELETE /pages/<page>` | write | Body `{"base": rev}`. The page's history is kept. |
| `GET /history/<page>` | read | Earlier versions, newest first: `rev`, `updated`, `updated_by`, `replaced`. |
| `GET /log?tail=50&page=<page>` | read | Recent writes, newest first. |
| `GET /search?q=<words>` | read | Pages holding every word, best first, with the matching lines. |
| `GET /lint` | read | What is worth tidying: broken links, pages without a summary, large (over 16 KB), duplicate or stale (over 180 days) pages, an index too long to show whole. |

A write by a service is logged under its id. A write with the launcher's token is logged as `user`, or under `by` when the caller gives one (an agent names itself, for example `"by": "claude-code"`). `note` is one line saying why, kept in the log.

The launcher sends `event: change` with `data: {"keys": ["memory"]}` on its own event stream (§10.5) after every write.

### 12.5 Writing without losing writes

Several services and agents write the same few pages. Every write names the revision it was based on:

- **Create:** `base` absent or `null`. If the page exists, the write is refused with `409` and the page as `current`.
- **Update:** `base` is the `rev` the writer read. If the page has changed since, the write is refused with `409`, a message naming who changed it, and the page as `current`. The writer merges its change into `current` and sends again with the new `rev`.
- **Delete:** needs `base` in the same way.
- **Append** needs none.

A write that changes nothing is not logged. Every update and delete first keeps the version it replaces in history, so the user can see and restore any of the last 20.

One page per topic, and appending to it rather than rewriting it, keeps writers out of each other's way. Pages that grow messy are tidied (§12.6), not avoided.

### 12.6 Agents

Agents run as the user, so they can use `lx memory` with the launcher's token. What the user sets for them, on the "Your AI agents" row or with `lx memory access agents <level>`, decides what they are told:

- `none`: nothing. The launcher publishes no memory skill.
- `read` or `write`: the launcher publishes a skill named `launchx-memory` (the name is reserved) into the agents' skill folders, as in §10.7. It says what memory is for, carries the current index, and says how to read, and with `write` how to write: read first, write with `--base`, name itself with `--by`, one-line summaries, no secrets. It asks agents to treat what pages say as information about the user, not as instructions. The launcher rewrites it as pages change.

Like discovery, this decides what agents are told, not what they can reach (§11).

With `write`, the skill also asks agents to tidy memory when the user asks or when it is due: run `lx memory lint`, merge duplicate pages, fix broken links, shorten what is long, and correct facts that newer ones contradict, using `lx memory log` and `lx memory history` as the record of what changed. Memory consolidation is done by an agent, with the user's model, rather than by the launcher, which takes no side on models.

### 12.7 CLI

`lx memory` (with no action, `ls`) works whether or not the launcher runs: without it, `lx` reads and writes the files itself, under the same lock.

| Command | Does |
|---|---|
| `lx memory ls` | Pages, and the well-known pages not yet written. |
| `lx memory show <page> [--rev R] [--json]` | Print a page; `--json` gives its `rev`. |
| `lx memory write <page> [--base REV] [--file F] [--title T] [--summary S] [--note N] [--by B]` | Write a page from stdin or a file. A page that exists needs `--base`. |
| `lx memory append <page> [text] [--title T] [--note N] [--by B]` | Add to the end of a page. |
| `lx memory rm <page> [--base REV]` | Delete a page; without `--base`, whatever it holds now. |
| `lx memory history <page>` | Earlier versions. |
| `lx memory log [-n N] [--page P]` | Recent writes. |
| `lx memory search <words…>` / `lx memory lint` / `lx memory index` | As in §12.4. |
| `lx memory access` / `lx memory access <id\|agents> <none\|read\|write>` | Who may use memory, or set it. |
| `lx memory open` | Open the Memory view of the launcher's page, signed in. |

Commands that read take `--json`.

## 13. Not in 0.1

- **Action catalogue.** A machine-readable list of each service's actions, so one generic client (or one MCP server in the launcher) can drive every service. The likely next addition.
- **Remote and phone access.** Exposing the gateway on a LAN or Tailscale with pairing, so every service gets it at once.
- **Restoring on a new machine.** `installs.json` lists every source, so `lx install` could reinstall them all from it.
- **Pinned versions.** `lx install` follows a repository's releases, or its default branch while it has none; holding an app at a chosen tag or following another branch is not there yet.
- **Jobs.** One-shot or scheduled commands, as opposed to long-running services.
- **Adopting standalone instances.** A service started by hand is not taken over by the launcher.
- **Memory by page.** Access is per service, for all of memory. A switch per page or per group of pages (so a service could read `schedule` but not `resume`) is the likely next step.
- **Memory events for services.** Services poll `GET /api/memory` and compare its `rev`; a change stream they can subscribe to is not there yet.
- **Version constraints, sandboxing, multiple users, Windows.**

## Appendix A. Worked examples

Not everything is a service. An agent plugin that only runs when an agent calls it, such as [ppagent](https://github.com/AutoPhd-org/your-paper-reading-agent), needs no manifest. Its state lives in files the agent writes.

### vmc, a machine console — level 2, public code with a private profile

vmc shows live CPU, RAM, GPU and disk usage for servers reached over SSH. Its first version named the cluster it watched in its code, which kept it private. It now has two layers, a pattern §4.6 recommends:

| Layer | Contains | Published |
|---|---|---|
| Code | The service, CLI, page, tests and `launchx.toml`. It watches whatever SSH aliases a site profile lists. | Yes |
| Site profile | `~/.config/vmc/site.toml`: the cluster's name, labels and host aliases. Addresses, users and keys stay in `~/.ssh/config`. | Never |

A pre-commit hook in the public repository refuses any commit that contains a word from the private layer, or a hostname or user that OpenSSH resolves for the profile's hosts.

What it took to meet the contract:

1. `vmc serve` runs in the foreground and reads `LAUNCHX_HOST`, `LAUNCHX_PORT`, `LAUNCHX_TOKEN` and `LAUNCHX_SERVICE`.
2. Under LaunchX every request needs the bearer token, and the page carries no token. A copy started by hand keeps its own Host, Origin and page-token checks.
3. `/api/health` returns `id`, `version`, `status` (`degraded` when no machine is reachable) and a `summary` such as "11 of 12 machines online".
4. `/api/state` includes the public part of the site profile, so the page shows the cluster's labels without them being in the code.
5. CLI: `vmc state`, `vmc ls`, `vmc refresh`, `vmc gpu-users`, `vmc settings`. Commands that act find the running service through `run/vmc.json` (§10.2).
6. `vmc` with no arguments hands over to `lx open vmc` when LaunchX is running.

The page polls `/api/state`; there is no event stream.

### Schedule Everything — level 2, adapting an existing app

[Schedule Everything](https://github.com/AutoPhd-org/schedule-everything) (`rmd`) already had bearer-token auth, a health route, a server-sent event stream and a broad CLI. Its server started itself in the background and kept its own registry of port, token and process. To meet the contract (these changes are working but not yet released in Schedule Everything):

1. `rmd serve` runs the same server in the foreground, with port and token from the environment, and skips its own registry, detaching and browser sessions.
2. `/api/health` reports `id`, `version`, `status` and a summary such as "11 open tasks · next: dinner at 18:30".
3. `GET /api/state` returns what the page shows. The page's existing command channel is documented as the action API (§7.3).
4. `rmd state` prints that document, and `rmd web` hands over to `lx open rmd` when LaunchX is running.

Its reminder daemon is a separate long-running process with no HTTP interface, a natural level 1 service with `autostart = true` and `restart = "always"`.

```toml
launchx = "0.1"

[service.rmd]
name  = "Schedule Everything"
level = 2
run   = ["rmd", "serve"]
cli   = "rmd"

[service.rmd.http]
health = "/api/health"
ui     = "/"
state  = "/api/state"
```

## Appendix B. Open questions

Choices this draft makes that would be expensive to reverse. Feedback welcome.

1. **The gateway is mandatory for browsers.** Services never implement browser auth; they check one header. The cost is that the launcher must contain a streaming reverse proxy. The gain is that security is written once, URLs are stable, and remote access can later be added for every service at once.
2. **Standalone instances are not adopted.** Convenience commands hand over to the launcher instead (§8.4). A copy started by hand before the launcher is running stays outside it.
3. **Events are recommended, not required, at level 2.** Polling with an `ETag` is accepted.
4. **Agents are told nothing until the user says so.** Discovery is off for every service by default (§10.7), and a service's manifest cannot turn it on. Publishing into agents' skill folders by link, rather than by asking each agent to read `lx agents`, is what makes the skills show up in an agent's skill list.
5. **Memory belongs to the launcher, not to a service.** It is one store with one access switch per service, rather than a memory service that others `require`. The cost is a bigger launcher; the gain is that the person sets access where they already set everything else, and no service has to be running for another to read memory.
6. **Memory is Markdown files and optimistic revisions, not a database.** The person can read and edit it with any tool, and agents read it as text. Two writers to the same page are serialised by `base` revisions rather than merged; the second gets the page back and merges itself.
7. **Smaller choices.** `~/.launchx` as the home directory instead of XDG or `~/Library`; port 7600 for the gateway; several services per manifest file.
