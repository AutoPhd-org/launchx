# LaunchX Service Contract

Version 0.1 — draft, 2026-11-10

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
state.json           assigned ports and tokens, which services the user left running
                     and which are discoverable by agents; mode 0600
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
GET  /api/services/{id}/logs?tail=200
GET  /api/agents                       what agents are told (§10.7)
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
      "agents": { "discoverable": false, "skills": [] }
    }
  ]
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
| `lx check <id>` | Verify the service against its declared level. |
| `lx state` | Print the launcher's state document. |
| `lx agents` | List the services agents can discover (§10.7). |
| `lx agents on <id>…` / `lx agents off <id>…` | Make services discoverable by agents, or hide them. |

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

## 11. Security model

| Threat | Handled by |
|---|---|
| Another local user connecting to a service's port | The service's bearer token (§6.2). |
| Another local user connecting to the gateway | The gateway's browser session (§10.4). |
| A web page sending requests to localhost | The gateway's Origin check; no CORS on services. |
| DNS rebinding | The gateway routes by Host; services require a token the attacker's page cannot have. |
| One service reaching into another | Tokens are per service; only declared dependencies receive one (§9). |
| Other devices on the network | Everything binds to loopback by default. |

Out of scope: a hostile process running as the same user. It can read the run files, as it can read everything else the user owns. This includes an AI agent the user runs: hiding a service from agents (§10.7) keeps it out of what they are told, and a well-behaved agent stays away, but only running the agent as another user, or in a sandbox, keeps it out.

## 12. Not in 0.1

- **Action catalogue.** A machine-readable list of each service's actions, so one generic client (or one MCP server in the launcher) can drive every service. The likely next addition.
- **Remote and phone access.** Exposing the gateway on a LAN or Tailscale with pairing, so every service gets it at once.
- **Install and update.** `lx install <repo>`. For now, installing is the job of the service's own installer.
- **Jobs.** One-shot or scheduled commands, as opposed to long-running services.
- **Adopting standalone instances.** A service started by hand is not taken over by the launcher.
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
5. **Smaller choices.** `~/.launchx` as the home directory instead of XDG or `~/Library`; port 7600 for the gateway; several services per manifest file.
