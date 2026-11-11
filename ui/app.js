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
    const focused = document.activeElement?.closest("[data-id]");
    const focusKey = focused ? `${focused.dataset.action}:${focused.dataset.id}` : null;
    grid.innerHTML = markup;
    grid._markup = markup;
    if (focusKey) {
      const [action, id] = focusKey.split(":");
      grid.querySelector(`[data-action="${action}"][data-id="${CSS.escape(id)}"]`)?.focus();
    }
  }
  $("#empty").hidden = services.length > 0;
  const errors = state.errors || [];
  $("#errors").hidden = errors.length === 0;
  $("#errors").innerHTML = errors.map((e) => `<p>${esc(e)}</p>`).join("");
  $("#footer").textContent = `LaunchX ${state.launcher.version} · ${state.launcher.home}`;
}

async function refresh() {
  try {
    const response = await fetch("/api/state", { cache: "no-store" });
    if (response.status === 401) { location.reload(); return; }
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    state = await response.json();
    render();
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
  $("#logs-title").textContent = `${service?.name || id} · logs`;
  $("#logs-body").textContent = "";
  $("#logs").hidden = false;
  try {
    const response = await fetch(`/api/services/${encodeURIComponent(id)}/logs?tail=400`);
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
  events.addEventListener("change", scheduleRefresh);
  events.addEventListener("log", (event) => {
    const data = JSON.parse(event.data);
    if (data.id === logsFor) appendLog(data.line);
  });
  events.addEventListener("error", () => setConnection(false));
}

$("#services").addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action]");
  if (button) act(button.dataset.action, button.dataset.id);
});
$("#logs-close").addEventListener("click", closeLogs);
document.addEventListener("keydown", (event) => { if (event.key === "Escape" && !$("#logs").hidden) closeLogs(); });
setInterval(() => { if (state) render(); }, 30000);
refresh();
connect();
