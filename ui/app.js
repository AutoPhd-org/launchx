"use strict";

const $ = (selector) => document.querySelector(selector);
let state = null;
let logsFor = null;
let refreshTimer = null;
let toastTimer = null;
const busy = new Set();

function esc(value) {
  return String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function duration(since) {
  if (!since) return "—";
  const seconds = Math.max(0, Math.round((Date.now() - Date.parse(since)) / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
  return `${Math.floor(seconds / 86400)}d ${Math.floor((seconds % 86400) / 3600)}h`;
}

function toast(message) {
  clearTimeout(toastTimer);
  $("#toast").textContent = message;
  $("#toast").hidden = false;
  toastTimer = setTimeout(() => { $("#toast").hidden = true; }, 4000);
}

const ACCESS = [["none", "Off"], ["read", "Read"], ["write", "Read & write"]];
const RANK = { none: 0, read: 1, write: 2 };

function ago(time) {
  if (!time) return "";
  const seconds = Math.max(0, Math.round((Date.now() - Date.parse(time)) / 1000));
  if (seconds < 60) return "just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 86400 * 30) return `${Math.floor(seconds / 86400)}d ago`;
  return time.slice(0, 10);
}

function who(name) {
  return name === "user" ? "you" : name || "someone";
}

/** Off / Read / Read & write, for an app (its id) or for "agents". */
function segmented(target, access) {
  const pending = busy.has(`memory:${target}`);
  return `<div class="segmented" role="radiogroup" aria-label="Memory access for ${esc(target)}">${ACCESS.map(([value, label]) =>
    `<button type="button" role="radio" aria-checked="${access === value}" data-action="memory-access" data-who="${esc(target)}" data-access="${value}" ${pending ? "disabled" : ""}>${label}</button>`).join("")}</div>`;
}

function asks(memory) {
  if (!memory?.wants || memory.wants === "none" || RANK[memory.access] >= RANK[memory.wants]) return "";
  const what = memory.wants === "write" ? "read and write" : "read";
  return `<p class="asks">Asks to ${what} your memory${memory.reason ? `: ${esc(memory.reason)}` : ""}</p>`;
}

// --- Installed apps (§10.8) ------------------------------------------------------

const JOB_LOG = ".install";
const VERBS = { install: "Installing", update: "Updating", uninstall: "Uninstalling" };
let dismissedJob = null;
let planned = null;

function jobRunning() {
  return state?.job?.status === "running";
}

function shortSource(pkg) {
  return pkg.cloned ? pkg.source.replace(/^https:\/\/github\.com\//, "").replace(/\.git$/, "") : `the folder ${pkg.dir}`;
}

function packageLine(pkg) {
  const at = pkg.version || (pkg.commit ? pkg.commit.slice(0, 7) : "");
  const commit = at ? ` · ${at}` : "";
  const fresh = pkg.available
    ? `<span class="new">${esc(pkg.available)} available</span>`
    : pkg.behind > 0 ? `<span class="new">${pkg.behind} new commit${pkg.behind === 1 ? "" : "s"}</span>` : "";
  const off = jobRunning() ? "disabled" : "";
  return `<div class="package">
    <span class="from" title="Installed from ${esc(pkg.source)}">From ${esc(shortSource(pkg))}${esc(commit)}</span>${fresh}
    <button class="button small" data-action="update" data-id="${esc(pkg.name)}" ${off}>Update</button>
    <button class="button small danger" data-action="uninstall" data-id="${esc(pkg.name)}" ${off}>Uninstall</button>
  </div>`;
}

function renderJob() {
  const job = state.job;
  const box = $("#job");
  const running = jobRunning();
  $("#install-open").disabled = running;
  $("#check-updates").disabled = running;
  $("#check-updates").hidden = !(state.packages || []).some((p) => p.cloned);
  if (!job || dismissedJob === job.started_at) { box.hidden = true; return; }
  const markup = running
    ? `<span class="spinner" aria-hidden="true"></span><span class="message">${VERBS[job.action] || esc(job.action)} ${esc(job.name)}…</span><button class="link" type="button" data-job="output">Show output</button>`
    : `<span class="dot ${job.status === "done" ? "ok" : "bad"}"></span><span class="message" title="${esc(job.message)}">${esc(job.message)}</span><button class="link" type="button" data-job="output">Output</button><button class="link" type="button" data-job="dismiss">Dismiss</button>`;
  if (box._markup !== markup) { box.innerHTML = markup; box._markup = markup; }
  box.className = `job ${job.status}`;
  box.hidden = false;
}

async function startJob(url, body) {
  try {
    const response = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
    const result = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
    dismissedJob = null;
    openLogs(JOB_LOG);
    refresh();
    return true;
  } catch (error) {
    toast(error.message);
    return false;
  }
}

async function packageAction(action, name) {
  const pkg = (state.packages || []).find((p) => p.name === name);
  if (!pkg) return;
  if (action === "uninstall") {
    const lines = [`Uninstall ${name}?`, "", `Stops and removes ${pkg.services.join(", ")}.`];
    lines.push(pkg.cloned ? `Deletes the checkout ${pkg.dir}.` : `Leaves the folder ${pkg.dir} as it is.`);
    if (pkg.data.length) lines.push(`Keeps your data in ${pkg.data.join(", ")}.`);
    if (!confirm(lines.join("\n"))) return;
  }
  await startJob(`/api/packages/${encodeURIComponent(name)}/${action}`, {});
}

function resetInstall() {
  planned = null;
  $("#install-plan").innerHTML = "";
  $("#install-plan").className = "plan";
  $("#install-go").hidden = true;
  $("#install-look").hidden = false;
}

function renderPlan(plan) {
  const row = (label, value) => value ? `<dt>${label}</dt><dd>${value}</dd>` : "";
  const services = plan.services.map((s) => `<code>${esc(s.id)}</code> ${esc(s.name)}`).join("<br>");
  const installer = plan.run.length ? `<code>${esc(plan.run.join(" "))}</code>` : "None: LaunchX registers the app and runs it from the checkout";
  $("#install-plan").innerHTML = `<dl>
      ${row("App", esc(plan.name))}
      ${row("From", esc(plan.source))}
      ${row("Version", plan.version ? esc(plan.version) : "")}
      ${plan.cloned ? row("Checkout", `<code>${esc(plan.dir)}</code>`) : ""}
      ${row("Services", services)}
      ${row("Installer", installer)}
      ${row("Needs", plan.needs.map(esc).join(", "))}
      ${row("Your data", plan.data.map((d) => `<code>${esc(d)}</code>`).join(" "))}
    </dl>
    ${plan.installed ? '<p class="warning">Already installed. This runs its installer again.</p>' : ""}
    ${plan.run.length ? '<p class="warning">The installer runs as you, with access to your files. Install only from sources you trust.</p>' : ""}`;
  $("#install-go").textContent = plan.installed ? "Reinstall" : "Install";
}

async function lookUp() {
  const source = $("#install-source").value.trim();
  if (!source) return;
  if (planned && planned.source === source) { install(); return; }
  const look = $("#install-look");
  look.disabled = true;
  $("#install-plan").className = "plan";
  $("#install-plan").innerHTML = '<span class="spinner" aria-hidden="true"></span> Fetching…';
  try {
    const response = await fetch("/api/install/plan", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ source }) });
    const result = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
    planned = { source, plan: result };
    renderPlan(result);
    look.hidden = true;
    $("#install-go").hidden = false;
    $("#install-go").focus();
  } catch (error) {
    $("#install-plan").className = "plan error";
    $("#install-plan").textContent = error.message;
  } finally {
    look.disabled = false;
  }
}

async function install() {
  if (!planned) return;
  const go = $("#install-go");
  go.disabled = true;
  const ok = await startJob("/api/install", { source: planned.source });
  go.disabled = false;
  if (ok) $("#install-dialog").close();
}

async function checkUpdates() {
  const button = $("#check-updates");
  button.disabled = true;
  button.textContent = "Checking…";
  try {
    const response = await fetch("/api/packages/check", { method: "POST" });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
    const fresh = result.packages.filter((p) => p.behind > 0);
    if (result.errors?.length) toast(result.errors.join("; "));
    else toast(fresh.length ? `Updates for ${fresh.map((p) => p.name).join(", ")}` : "Everything is up to date");
  } catch (error) {
    toast(`Could not check: ${error.message}`);
  } finally {
    button.textContent = "Check for updates";
    button.disabled = false;
    refresh();
  }
}

const LABELS = { running: "Running", starting: "Starting", waiting: "Waiting", stopping: "Stopping", stopped: "Stopped", failed: "Failed" };

function healthDot(service) {
  if (service.status !== "running") return service.status === "failed" ? "bad" : "";
  return { ok: "ok", degraded: "warn", unhealthy: "bad" }[service.health] || "";
}

function card(service) {
  const active = ["running", "starting", "waiting"].includes(service.status);
  const pending = busy.has(service.id) || service.status === "stopping";
  const problem = service.error || (service.health === "unhealthy" ? service.detail : null);
  const line = problem || service.summary || service.detail || (service.status === "stopped" && service.last_exit ? `Last run ${service.last_exit}` : "");
  const icon = (service.name || service.id).trim().charAt(0).toUpperCase();
  const requires = service.requires.length ? `<span>Requires</span>${service.requires.map((r) => `<span class="chip">${esc(r)}</span>`).join("")}` : "";
  const requiredBy = service.required_by.length ? `<span>Required by</span>${service.required_by.map((r) => `<span class="chip">${esc(r)}</span>`).join("")}` : "";
  const open = service.ui && service.status === "running"
    ? `<a class="button primary" href="${esc(service.ui)}" target="_blank" rel="noopener">Open</a>` : "";
  const toggle = active || service.status === "stopping"
    ? `<button class="button" data-action="stop" data-id="${esc(service.id)}" ${pending ? "disabled" : ""}>Stop</button>`
    : `<button class="button ${service.ui ? "" : "primary"}" data-action="start" data-id="${esc(service.id)}" ${pending || service.error && service.status !== "failed" ? "disabled" : ""}>Start</button>`;
  const restart = service.status === "running"
    ? `<button class="button" data-action="restart" data-id="${esc(service.id)}" ${pending ? "disabled" : ""}>Restart</button>` : "";
  const pkg = service.package && (state.packages || []).find((p) => p.name === service.package);
  const agents = service.agents || { discoverable: false, skills: [] };
  const offers = [service.cli ? "CLI" : "", agents.skills.length ? `${agents.skills.length} skill${agents.skills.length === 1 ? "" : "s"}` : ""].filter(Boolean).join(" · ");
  const agentsHelp = agents.discoverable
    ? `Agents are told about this app${agents.skills.length ? ` and get its skills: ${agents.skills.join(", ")}` : ""}.`
    : "Hidden from agents. Switch on to list it in `lx agents` and publish its skills to your agents' skill folders.";
  return `<article class="card" data-card="${esc(service.id)}">
    <div class="card-head">
      <div class="icon" aria-hidden="true">${esc(icon)}</div>
      <div class="title"><h2 title="${esc(service.name)}">${esc(service.name)}</h2>
        <div class="sub">${esc(service.id)}<span class="level" title="Contract level ${service.level}">L${service.level}</span></div></div>
      <span class="pill ${esc(service.status)}">${LABELS[service.status] || esc(service.status)}</span>
    </div>
    ${service.description ? `<p class="description">${esc(service.description)}</p>` : ""}
    <p class="summary ${problem ? "problem" : ""}"><span class="dot ${healthDot(service)}"></span><span>${esc(line) || "&nbsp;"}</span></p>
    <dl class="meta">
      <div><dt>Version</dt><dd>${esc(service.version || "—")}</dd></div>
      <div><dt>Port</dt><dd>${esc(service.port ?? "—")}</dd></div>
      <div><dt>Up</dt><dd>${service.status === "running" ? duration(service.since) : "—"}</dd></div>
      <div><dt>Restarts</dt><dd>${service.restarts}</dd></div>
    </dl>
    ${requires || requiredBy ? `<div class="relations">${requires}${requires && requiredBy ? "<span>·</span>" : ""}${requiredBy}</div>` : ""}
    <div class="memory-access" title="What this app may do with your shared memory. LaunchX checks it on every request.">
      <span>Memory</span>${segmented(service.id, service.memory?.access || "none")}
    </div>
    ${asks(service.memory)}
    <label class="agents" title="${esc(agentsHelp)}">
      <input type="checkbox" role="switch" data-action="agents" data-id="${esc(service.id)}" ${agents.discoverable ? "checked" : ""} ${busy.has(`agents:${service.id}`) ? "disabled" : ""}>
      <span class="switch" aria-hidden="true"></span>
      <span>Discoverable by agents</span>
      ${offers ? `<span class="offers">${esc(offers)}</span>` : ""}
    </label>
    ${pkg ? packageLine(pkg) : ""}
    <div class="actions">${open}${toggle}${restart}<button class="button right" data-action="logs" data-id="${esc(service.id)}">Logs</button></div>
  </article>`;
}

function render() {
  if (!state) return;
  const services = state.services;
  const running = services.filter((s) => s.status === "running").length;
  const failed = services.filter((s) => s.status === "failed").length;
  $("#overview").textContent = services.length
    ? `${running} of ${services.length} running${failed ? ` · ${failed} failed` : ""}` : "";
  const grid = $("#services");
  const markup = services.map(card).join("");
  if (grid._markup !== markup) {
    const key = (el) => `${el.dataset.action}|${el.dataset.id || ""}|${el.dataset.who || ""}|${el.dataset.access || ""}`;
    const focused = document.activeElement?.closest("[data-action]");
    const focusKey = focused && grid.contains(focused) ? key(focused) : null;
    grid.innerHTML = markup;
    grid._markup = markup;
    if (focusKey) [...grid.querySelectorAll("[data-action]")].find((el) => key(el) === focusKey)?.focus();
  }
  const pages = state.memory?.pages ?? 0;
  $("#memory-count").textContent = pages ? String(pages) : "";
  $("#empty").hidden = services.length > 0;
  const errors = state.errors || [];
  $("#errors").hidden = errors.length === 0;
  $("#errors").innerHTML = errors.map((e) => `<p>${esc(e)}</p>`).join("");
  renderJob();
  $("#footer").textContent = `LaunchX ${state.launcher.version} · ${state.launcher.home}`;
}

async function refresh() {
  try {
    const response = await fetch("/api/state", { cache: "no-store" });
    if (response.status === 401) { location.reload(); return; }
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    state = await response.json();
    render();
    if (route().view === "memory" && !memory.page && !memory.editing) renderMemory();
  } catch (error) {
    setConnection(false);
  }
}

function scheduleRefresh() {
  clearTimeout(refreshTimer);
  refreshTimer = setTimeout(refresh, 80);
}

function setConnection(ok) {
  $("#connection").innerHTML = ok
    ? '<span class="dot ok"></span><span>Live</span>'
    : '<span class="dot bad"></span><span>Reconnecting…</span>';
}

async function act(action, id) {
  if (action === "logs") { openLogs(id); return; }
  if (action === "update" || action === "uninstall") { packageAction(action, id); return; }
  const service = state.services.find((s) => s.id === id);
  if (action === "stop") {
    const dependents = service.required_by.filter((other) => {
      const s = state.services.find((x) => x.id === other);
      return s && !["stopped", "failed"].includes(s.status);
    });
    if (dependents.length && !confirm(`Stopping ${service.name} also stops ${dependents.join(", ")}, which require it. Continue?`)) return;
  }
  busy.add(id);
  render();
  try {
    const response = await fetch(`/api/services/${encodeURIComponent(id)}/${action}`, { method: "POST" });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
    if (action === "stop" && result.stopped?.length > 1) toast(`Stopped ${result.stopped.join(", ")}`);
    if (action === "start" && result.started?.length > 1) toast(`Starting ${result.started.join(", ")}`);
  } catch (error) {
    toast(`Could not ${action} ${id}: ${error.message}`);
  } finally {
    busy.delete(id);
    refresh();
  }
}

async function setAgents(id, on) {
  busy.add(`agents:${id}`);
  render();
  try {
    const response = await fetch(`/api/services/${encodeURIComponent(id)}/agents`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ discoverable: on }),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
    toast(on ? `Agents can now discover ${id}` : `${id} is hidden from agents`);
  } catch (error) {
    toast(`Could not change ${id}: ${error.message}`);
  } finally {
    busy.delete(`agents:${id}`);
    refresh();
  }
}

// --- Shared memory --------------------------------------------------------------

const memory = {
  listing: null, // GET /api/memory
  page: null, // the page shown, or {missing: name}
  history: null, // its earlier versions, when shown
  viewing: null, // an earlier version being looked at
  editing: null, // {page, isNew, base, title, summary, body, note, dirty}
  conflict: undefined, // the page as it is now, after a refused save
  results: null, // search results
  log: [],
  lint: [],
};
let memoryTimer = null;
let lastHash = location.hash;

function route() {
  const hash = decodeURIComponent(location.hash.replace(/^#/, ""));
  const [view, ...rest] = hash.split("/");
  return view === "memory" ? { view: "memory", page: rest.join("/") || null } : { view: "apps" };
}

function memoryDirty() {
  return Boolean(memory.editing?.dirty);
}

async function applyRoute() {
  if (memoryDirty() && location.hash !== lastHash) {
    if (!confirm("Discard your unsaved changes to this page?")) { history.replaceState(null, "", lastHash || "#"); return; }
  }
  lastHash = location.hash;
  const current = route();
  $("#apps-view").hidden = current.view !== "apps";
  $("#memory-view").hidden = current.view !== "memory";
  document.querySelectorAll(".tabs a").forEach((tab) => {
    if (tab.dataset.tab === current.view) tab.setAttribute("aria-current", "page"); else tab.removeAttribute("aria-current");
  });
  document.title = current.view === "memory" ? "Memory · LaunchX" : "LaunchX";
  if (current.view !== "memory") return;
  memory.editing = null;
  memory.conflict = undefined;
  memory.history = null;
  memory.viewing = null;
  await loadMemory(current.page);
}

async function getJSON(url) {
  const response = await fetch(url, { cache: "no-store" });
  if (response.status === 401) { location.reload(); throw new Error("signed out"); }
  const result = await response.json();
  if (!response.ok) { const error = new Error(result.error || `HTTP ${response.status}`); error.status = response.status; throw error; }
  return result;
}

async function sendJSON(method, url, body) {
  const response = await fetch(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  const result = await response.json().catch(() => ({}));
  return { ok: response.ok, status: response.status, result };
}

async function loadMemory(name) {
  try {
    const [listing, page, log, lint] = await Promise.all([
      getJSON("/api/memory"),
      name ? getJSON(`/api/memory/pages/${name}`).catch((error) => (error.status === 404 ? { missing: name } : Promise.reject(error))) : null,
      name ? null : getJSON("/api/memory/log?tail=25"),
      name ? null : getJSON("/api/memory/lint"),
    ]);
    memory.listing = listing;
    memory.page = page;
    if (!name) { memory.log = log.entries; memory.lint = lint.problems; }
    if (memory.results && $("#memory-q").value.trim()) await searchMemory($("#memory-q").value);
    renderMemory();
  } catch (error) {
    $("#memory-main").innerHTML = `<p class="problem">Could not load memory: ${esc(error.message)}</p>`;
  }
}

/** After a change elsewhere: reload, unless the person is in the middle of an edit. */
function scheduleMemory() {
  clearTimeout(memoryTimer);
  memoryTimer = setTimeout(() => {
    if (memory.editing) { refreshMemoryList(); return; }
    loadMemory(route().page);
  }, 150);
}

async function refreshMemoryList() {
  try { memory.listing = await getJSON("/api/memory"); renderMemorySide(); } catch { /* keep what is shown */ }
}

function pageNames() {
  return new Set((memory.listing?.pages || []).map((p) => p.page));
}

/** Markdown, the small part pages need. Everything is escaped before it is marked up. */
function inline(text) {
  const names = pageNames();
  return text.split(/(`[^`]+`)/).map((part, i) => {
    if (i % 2) return `<code>${esc(part.slice(1, -1))}</code>`;
    return esc(part)
      .replace(/\[\[([a-z0-9][a-z0-9\/-]*)(?:\|([^\]]+))?\]\]/g, (_, name, label) =>
        `<a href="#memory/${name}" class="wiki${names.has(name) ? "" : " missing"}" title="${names.has(name) ? name : `${name} (not written yet)`}">${label || name}</a>`)
      .replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+|mailto:[^\s)]+)\)/g, (_, label, url) => `<a href="${url}" target="_blank" rel="noopener">${label}</a>`)
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/(^|[\s(])\*([^*\s][^*]*)\*/g, "$1<em>$2</em>")
      .replace(/(^|[\s(])_([^_\s][^_]*)_(?=$|[\s).,;:!?])/g, "$1<em>$2</em>");
  }).join("");
}

function markdown(source) {
  const lines = (source || "").replace(/\r/g, "").split("\n");
  let html = "";
  let paragraph = [];
  let list = null;
  const closeParagraph = () => { if (paragraph.length) { html += `<p>${inline(paragraph.join(" "))}</p>`; paragraph = []; } };
  const closeList = () => { if (list) { html += `</${list}>`; list = null; } };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    let match;
    if (/^\s*```/.test(line)) {
      closeParagraph(); closeList();
      const code = [];
      for (i++; i < lines.length && !/^\s*```/.test(lines[i]); i++) code.push(lines[i]);
      html += `<pre><code>${esc(code.join("\n"))}</code></pre>`;
    } else if (!line.trim()) {
      closeParagraph(); closeList();
    } else if ((match = line.match(/^(#{1,6})\s+(.*)$/))) {
      closeParagraph(); closeList();
      const level = Math.min(6, match[1].length + 2);
      html += `<h${level}>${inline(match[2])}</h${level}>`;
    } else if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(line)) {
      closeParagraph(); closeList();
      html += "<hr>";
    } else if ((match = line.match(/^\s*(?:[-*+]|(\d+)[.)])\s+(.*)$/))) {
      closeParagraph();
      const kind = match[1] ? "ol" : "ul";
      if (list !== kind) { closeList(); html += `<${kind}>`; list = kind; }
      const box = match[2].match(/^\[( |x|X)\]\s+(.*)$/);
      html += box ? `<li class="task"><span class="box ${box[1] === " " ? "" : "done"}" aria-hidden="true"></span>${inline(box[2])}</li>` : `<li>${inline(match[2])}</li>`;
    } else if (list && /^\s{2,}\S/.test(line)) {
      html = html.replace(/<\/li>$/, ` ${inline(line.trim())}</li>`);
    } else if ((match = line.match(/^>\s?(.*)$/))) {
      closeParagraph(); closeList();
      html += `<blockquote>${inline(match[1])}</blockquote>`;
    } else {
      closeList();
      paragraph.push(line.trim());
    }
  }
  closeParagraph(); closeList();
  return html || '<p class="muted">This page is empty.</p>';
}

function renderMemory() {
  renderMemorySide();
  renderMemoryMain();
}

function renderMemorySide() {
  const listing = memory.listing;
  if (!listing) return;
  const current = route().page;
  const side = $("#memory-pages");
  const link = (page) => `<a class="memory-link ${page.page === current ? "current" : ""}" href="#memory/${page.page}">
      <span class="name">${esc(page.title)}</span><span class="slug">${esc(page.page)}</span>
      ${page.lines?.length ? `<span class="lines">${page.lines.map((l) => esc(l)).join("<br>")}</span>` : ""}</a>`;
  let html = `<a class="memory-link overview ${current ? "" : "current"}" href="#memory"><span class="name">Overview</span><span class="slug">who can use it · recent changes</span></a>`;
  if (memory.results) {
    html += `<h3>${memory.results.length ? `Found in ${memory.results.length} page${memory.results.length === 1 ? "" : "s"}` : "No page has all of those words"}</h3>`;
    html += memory.results.map(link).join("");
  } else {
    html += `<h3>Pages${listing.count ? ` · ${listing.count}` : ""}</h3>`;
    html += listing.pages.length ? listing.pages.map(link).join("") : '<p class="muted small">No pages yet.</p>';
    if (listing.suggested.length) {
      html += "<h3>Not written yet</h3>";
      html += listing.suggested.map((page) => `<button type="button" class="memory-link suggested" data-action="memory-new" data-page="${esc(page.page)}">
        <span class="name">＋ ${esc(page.title)}</span><span class="slug">${esc(page.summary)}</span></button>`).join("");
    }
  }
  if (side._markup !== html) { side.innerHTML = html; side._markup = html; }
}

function renderMemoryMain() {
  const main = $("#memory-main");
  let html;
  if (!memory.listing) html = '<p class="muted">Loading…</p>';
  else if (memory.editing) html = editorView();
  else if (memory.page?.missing) html = missingView(memory.page.missing);
  else html = memory.page ? pageView(memory.page) : overviewView();
  if (main._markup === html) return;
  // Re-rendering must not take the keyboard focus away from where it was.
  const key = (el) => `${el.dataset?.action || ""}|${el.dataset?.who || ""}|${el.dataset?.access || ""}|${el.dataset?.rev || ""}|${el.name || ""}`;
  const active = document.activeElement;
  const focused = active && main.contains(active) ? key(active) : null;
  main.innerHTML = html;
  main._markup = html;
  if (memory.editing && !memory.editing.focused) {
    memory.editing.focused = true;
    main.querySelector(memory.editing.isNew ? "[name=page]" : "[name=body]")?.focus({ preventScroll: true });
  } else if (focused) {
    [...main.querySelectorAll("[data-action], [name]")].find((el) => key(el) === focused)?.focus({ preventScroll: true });
  }
}

function overviewView() {
  const services = state?.services || [];
  const rows = services.map((service) => `<div class="access-row">
      <div class="who"><strong>${esc(service.name)}</strong> <span class="muted">${esc(service.id)}</span>${asks(service.memory)}</div>
      ${segmented(service.id, service.memory?.access || "none")}</div>`).join("");
  const agents = `<div class="access-row agents-row">
      <div class="who"><strong>Your AI agents</strong> <span class="muted">through the <code>launchx-memory</code> skill</span>
        <p class="asks plain">They run as you, so this decides what they are told, not what they could reach.</p></div>
      ${segmented("agents", state?.memory?.agents || "none")}</div>`;
  const log = memory.log.length
    ? `<ul class="changes">${memory.log.map((entry) => `<li>
        <span class="when" title="${esc(entry.time)}">${esc(ago(entry.time))}</span>
        <span><strong>${esc(who(entry.by))}</strong> ${esc({ create: "created", update: "updated", append: "added to", delete: "deleted" }[entry.action] || entry.action)}
        ${entry.action === "delete" ? `<code>${esc(entry.page)}</code>` : `<a href="#memory/${esc(entry.page)}">${esc(entry.page)}</a>`}
        ${entry.note ? `<span class="muted">— ${esc(entry.note)}</span>` : ""}</span></li>`).join("")}</ul>`
    : '<p class="muted">No changes yet.</p>';
  const lint = memory.lint.length
    ? `<section class="panel"><h3>Worth tidying</h3><ul class="changes">${memory.lint.map((problem) => `<li><span>${problem.page ? `<a href="#memory/${esc(problem.page)}">${esc(problem.page)}</a> ` : ""}${esc(problem.detail)}</span></li>`).join("")}</ul>
        <p class="muted small">An agent with write access can tidy these: ask it to run <code>lx memory lint</code> and fix what it finds.</p></section>`
    : "";
  const count = memory.listing.count;
  return `<div class="memory-intro">
      <h2>Your memory</h2>
      <p>${count ? `${count} page${count === 1 ? "" : "s"}` : "Pages"} about you that your apps and agents share, so you do not have to tell each of them again: your profile, preferences, schedule, resume, projects. Each page is a Markdown file in <code>${esc(state?.memory?.dir || "~/.launchx/memory")}</code>; every change is logged and the version it replaced is kept.</p>
    </div>
    <section class="panel"><h3>Who can use it</h3>
      <p class="muted small">Every app starts with no access. An app asks LaunchX for each read and write, and LaunchX checks this each time.</p>
      <div class="access-table">${rows || '<p class="muted">No apps registered yet.</p>'}${agents}</div></section>
    <section class="panel"><h3>Recent changes</h3>${log}</section>
    ${lint}`;
}

function missingView(name) {
  const suggested = memory.listing.suggested.find((page) => page.page === name);
  return `<div class="memory-intro"><h2>${esc(suggested?.title || name)}</h2>
    <p class="muted">There is no page <code>${esc(name)}</code> yet${suggested ? `. It is for: ${esc(suggested.summary)}` : ""}.</p>
    <p><button class="button primary" type="button" data-action="memory-new" data-page="${esc(name)}">Write it</button></p></div>`;
}

function pageView(page) {
  const shown = memory.viewing || page;
  const backlinks = (memory.listing.pages || []).filter((other) => other.links?.includes(page.page) && other.page !== page.page);
  const banner = memory.viewing
    ? `<div class="notice">An earlier version, written ${esc(ago(memory.viewing.updated))} by ${esc(who(memory.viewing.updated_by))} (rev <code>${esc(memory.viewing.rev)}</code>).
        <span class="notice-actions"><button class="button primary" type="button" data-action="memory-restore">Restore this version</button>
        <button class="button" type="button" data-action="memory-current">Back to the current page</button></span></div>` : "";
  const history = memory.history
    ? `<section class="panel history"><h3>Earlier versions</h3>${memory.history.length
      ? `<ul class="changes">${memory.history.map((version) => `<li>
          <span class="when">${esc(ago(version.replaced))}</span>
          <span>Written ${esc(ago(version.updated))} by <strong>${esc(who(version.updated_by))}</strong> · <code>${esc(version.rev)}</code></span>
          <button class="button small" type="button" data-action="memory-version" data-rev="${esc(version.rev)}">View</button></li>`).join("")}</ul>`
      : '<p class="muted">No earlier versions.</p>'}</section>` : "";
  return `<article class="memory-page">
    <header>
      <h2>${esc(shown.title)}</h2>
      <div class="sub"><code>${esc(page.page)}</code> · updated ${esc(ago(page.updated))} by ${esc(who(page.updated_by))} · rev <code>${esc(page.rev)}</code></div>
    </header>
    ${shown.summary ? `<p class="page-summary">${esc(shown.summary)}</p>` : '<p class="page-summary empty">No summary yet. The index that apps and agents read first is made of summaries.</p>'}
    ${banner}
    <div class="markdown">${markdown(shown.body)}</div>
    ${backlinks.length ? `<p class="backlinks">Linked from ${backlinks.map((other) => `<a href="#memory/${esc(other.page)}">${esc(other.title)}</a>`).join(", ")}</p>` : ""}
    <div class="actions">
      <button class="button primary" type="button" data-action="memory-edit">Edit</button>
      <button class="button" type="button" data-action="memory-history" aria-expanded="${Boolean(memory.history)}">History</button>
      <button class="button right danger" type="button" data-action="memory-delete">Delete</button>
    </div>
    ${history}
  </article>`;
}

function editorView() {
  const draft = memory.editing;
  let conflict = "";
  if (memory.conflict !== undefined) {
    const by = memory.conflict && `${esc(who(memory.conflict.updated_by))}, ${esc(ago(memory.conflict.updated))}`;
    conflict = memory.conflict
      ? `<div class="notice problem">${draft.isNew ? `There is already a page <code>${esc(memory.conflict.page)}</code> (last written by ${by}).` : `This page was changed by ${by}, while you were editing.`} Your text is still here.
          Copy what you want to keep from their version into yours, then save mine.
          <details class="theirs"><summary>Their version</summary><pre>${esc(memory.conflict.body)}</pre></details>
          <span class="notice-actions"><button class="button" type="button" data-action="memory-theirs">Use their version</button>
          <button class="button primary" type="button" data-action="memory-mine">Save mine over it</button></span></div>`
      : `<div class="notice problem">This page was deleted while you were editing. Your text is still here.
          <span class="notice-actions"><button class="button primary" type="button" data-action="memory-mine">Save it anyway</button></span></div>`;
  }
  return `<form class="memory-editor" autocomplete="off">
    <h2>${draft.isNew ? "New page" : `Editing ${esc(draft.page)}`}</h2>
    ${draft.isNew ? `<label>Page name <input name="page" value="${esc(draft.page)}" required pattern="[a-z0-9][a-z0-9\-]*(/[a-z0-9][a-z0-9\-]*){0,3}" placeholder="profile, or projects/thesis">
      <span class="hint">Lowercase letters, digits and '-'; use '/' to group, like <code>people/ann</code>.</span></label>` : ""}
    <label>Title <input name="title" value="${esc(draft.title)}" maxlength="120"></label>
    <label>Summary <input name="summary" value="${esc(draft.summary)}" maxlength="300" placeholder="One line on what this page holds">
      <span class="hint">The index, which apps and agents read first, is made of these lines.</span></label>
    <label>Page <textarea name="body" rows="18" spellcheck="true">${esc(draft.body)}</textarea>
      <span class="hint">Markdown. Link other pages with <code>[[page]]</code>. No passwords or keys: every app you allow can read this.</span></label>
    <label>Note for the log <input name="note" value="${esc(draft.note)}" maxlength="300" placeholder="Why this change (optional)"></label>
    ${conflict}
    <div class="actions">
      <button class="button primary" type="submit" ${draft.saving ? "disabled" : ""}>${draft.saving ? "Saving…" : "Save"}</button>
      <button class="button" type="button" data-action="memory-cancel">Cancel</button>
    </div>
  </form>`;
}

function startEditing(page, isNew, preset = {}) {
  memory.conflict = undefined;
  memory.editing = isNew
    ? { page: preset.page || "", isNew: true, base: null, title: preset.title || "", summary: preset.summary || "", body: "", note: "", dirty: false }
    : { page: page.page, isNew: false, base: page.rev, title: page.title, summary: page.summary, body: page.body, note: "", dirty: false };
  renderMemoryMain();
}

function memoryInput(event) {
  if (event.target.id === "memory-q") {
    clearTimeout(memoryInput.timer);
    memoryInput.timer = setTimeout(() => searchMemory(event.target.value).then(renderMemorySide), 200);
    return;
  }
  const field = event.target.name;
  if (memory.editing && field in memory.editing) {
    memory.editing[field] = event.target.value;
    memory.editing.dirty = true;
  }
}

async function searchMemory(text) {
  const query = text.trim();
  if (!query) { memory.results = null; return; }
  try { memory.results = (await getJSON(`/api/memory/search?q=${encodeURIComponent(query)}`)).pages; } catch { memory.results = []; }
}

async function saveMemoryPage(overwrite = false) {
  const draft = memory.editing;
  if (!draft || draft.saving) return;
  const name = draft.page.trim();
  if (!/^[a-z0-9][a-z0-9-]*(\/[a-z0-9][a-z0-9-]*){0,3}$/.test(name)) { toast("A page name is lowercase letters, digits and '-', like profile or projects/thesis"); return; }
  let base = draft.base;
  if (overwrite) base = memory.conflict ? memory.conflict.rev : null;
  draft.saving = true;
  renderMemoryMain();
  const { ok, status, result } = await sendJSON("PUT", `/api/memory/pages/${name}`, { title: draft.title, summary: draft.summary, body: draft.body, note: draft.note, base });
  draft.saving = false;
  if (ok) {
    memory.editing = null;
    memory.conflict = undefined;
    toast(`Saved ${name}`);
    if (route().page === name) loadMemory(name); else location.hash = `#memory/${name}`;
    return;
  }
  if (status === 409) {
    memory.conflict = result.current ?? null;
  } else {
    toast(`Could not save: ${result.error || `HTTP ${status}`}`);
  }
  renderMemoryMain();
}

async function memoryClick(event) {
  const button = event.target.closest("[data-action]");
  if (!button) return;
  const action = button.dataset.action;
  const page = memory.page;
  if (action === "memory-access") { setMemoryAccess(button.dataset.who, button.dataset.access); return; }
  if (action === "memory-new") {
    const preset = memory.listing?.suggested.find((s) => s.page === button.dataset.page) || { page: button.dataset.page || "" };
    if (memoryDirty() && !confirm("Discard your unsaved changes to this page?")) return;
    startEditing(null, true, preset);
    return;
  }
  if (action === "memory-edit" && page) { memory.viewing = null; startEditing(page, false); return; }
  if (action === "memory-cancel") {
    if (memoryDirty() && !confirm("Discard your unsaved changes to this page?")) return;
    memory.editing = null; memory.conflict = undefined; renderMemoryMain(); return;
  }
  if (action === "memory-theirs" && memory.conflict) { startEditing({ ...memory.conflict, page: memory.conflict.page }, false); return; }
  if (action === "memory-mine") { saveMemoryPage(true); return; }
  if (action === "memory-history" && page) {
    if (memory.history) { memory.history = null; renderMemoryMain(); return; }
    try { memory.history = (await getJSON(`/api/memory/history/${page.page}`)).versions; } catch (error) { toast(error.message); return; }
    renderMemoryMain();
    return;
  }
  if (action === "memory-version" && page) {
    try { memory.viewing = await getJSON(`/api/memory/pages/${page.page}?rev=${encodeURIComponent(button.dataset.rev)}`); } catch (error) { toast(error.message); return; }
    renderMemoryMain();
    $("#memory-main").scrollIntoView({ block: "start", behavior: "smooth" });
    return;
  }
  if (action === "memory-current") { memory.viewing = null; renderMemoryMain(); return; }
  if (action === "memory-restore" && page && memory.viewing) {
    const version = memory.viewing;
    const { ok, result } = await sendJSON("PUT", `/api/memory/pages/${page.page}`, { text: version.text, base: page.rev, note: `Restored rev ${version.rev}` });
    if (!ok) { toast(`Could not restore: ${result.error}`); return; }
    toast(`Restored ${page.page} to rev ${version.rev}`);
    memory.viewing = null; memory.history = null;
    loadMemory(page.page);
    return;
  }
  if (action === "memory-delete" && page) {
    if (!confirm(`Delete "${page.title}"? Apps and agents will no longer see it. Its versions stay in history.`)) return;
    const { ok, result } = await sendJSON("DELETE", `/api/memory/pages/${page.page}`, { base: page.rev });
    if (!ok) { toast(`Could not delete: ${result.error}`); return; }
    toast(`Deleted ${page.page}`);
    location.hash = "#memory";
  }
}

async function setMemoryAccess(target, access) {
  busy.add(`memory:${target}`);
  render();
  if (route().view === "memory" && !memory.page && !memory.editing) renderMemoryMain();
  try {
    const url = target === "agents" ? "/api/memory/agents" : `/api/services/${encodeURIComponent(target)}/memory`;
    const { ok, status, result } = await sendJSON("POST", url, { access });
    if (!ok) throw new Error(result.error || `HTTP ${status}`);
    const subject = target === "agents" ? "Your agents" : target;
    toast(access === "none" ? `${subject} can no longer use your memory` : `${subject} can now ${access === "write" ? "read and write" : "read"} your memory`);
  } catch (error) {
    toast(`Could not change ${target}: ${error.message}`);
  } finally {
    busy.delete(`memory:${target}`);
    refresh();
  }
}

function appendLog(line) {
  const body = $("#logs-body");
  const span = document.createElement("span");
  if (/^\S+ \S+ \[launchx\]/.test(line)) span.className = "launcher-line";
  span.textContent = line + "\n";
  body.appendChild(span);
  while (body.childNodes.length > 2000) body.removeChild(body.firstChild);
  if ($("#logs-follow").checked) body.scrollTop = body.scrollHeight;
}

async function openLogs(id) {
  logsFor = id;
  const service = state.services.find((s) => s.id === id);
  $("#logs-title").textContent = id === JOB_LOG ? "Install output" : `${service?.name || id} · logs`;
  $("#logs-body").textContent = "";
  $("#logs").hidden = false;
  try {
    const url = id === JOB_LOG ? "/api/install/log?tail=400" : `/api/services/${encodeURIComponent(id)}/logs?tail=400`;
    const response = await fetch(url);
    const result = await response.json();
    if (logsFor !== id) return;
    if (!result.lines.length) $("#logs-body").textContent = "No output yet.\n";
    for (const line of result.lines) appendLog(line);
  } catch (error) {
    $("#logs-body").textContent = `Could not load logs: ${error.message}`;
  }
  $("#logs-close").focus();
}

function closeLogs() {
  logsFor = null;
  $("#logs").hidden = true;
}

function connect() {
  const events = new EventSource("/api/events");
  events.addEventListener("open", () => { setConnection(true); refresh(); });
  events.addEventListener("change", (event) => {
    let keys = [];
    try { keys = JSON.parse(event.data).keys || []; } catch { /* a bare change */ }
    if (keys.includes("memory")) { if (route().view === "memory") scheduleMemory(); }
    scheduleRefresh();
  });
  events.addEventListener("log", (event) => {
    const data = JSON.parse(event.data);
    if (data.id === logsFor) appendLog(data.line);
  });
  events.addEventListener("error", () => setConnection(false));
}

$("#services").addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  if (button.dataset.action === "memory-access") setMemoryAccess(button.dataset.who, button.dataset.access);
  else act(button.dataset.action, button.dataset.id);
});
$("#services").addEventListener("change", (event) => {
  const box = event.target.closest('input[data-action="agents"]');
  if (box) setAgents(box.dataset.id, box.checked);
});
$("#logs-close").addEventListener("click", closeLogs);
$("#job").addEventListener("click", (event) => {
  const button = event.target.closest("button[data-job]");
  if (!button) return;
  if (button.dataset.job === "output") openLogs(JOB_LOG);
  else { dismissedJob = state.job?.started_at; render(); }
});
$("#install-open").addEventListener("click", () => { resetInstall(); $("#install-dialog").showModal(); $("#install-source").select(); });
$("#install-cancel").addEventListener("click", () => $("#install-dialog").close());
$("#install-source").addEventListener("input", () => { if (planned) resetInstall(); });
$("#install-form").addEventListener("submit", (event) => { event.preventDefault(); lookUp(); });
$("#install-go").addEventListener("click", install);
$("#check-updates").addEventListener("click", checkUpdates);
$("#memory-view").addEventListener("click", memoryClick);
$("#memory-view").addEventListener("input", memoryInput);
$("#memory-view").addEventListener("submit", (event) => { event.preventDefault(); saveMemoryPage(); });
window.addEventListener("hashchange", applyRoute);
window.addEventListener("beforeunload", (event) => { if (memoryDirty()) event.preventDefault(); });
document.addEventListener("keydown", (event) => { if (event.key === "Escape" && !$("#logs").hidden) closeLogs(); });
setInterval(() => { if (state) render(); }, 30000);
applyRoute();
refresh();
connect();
