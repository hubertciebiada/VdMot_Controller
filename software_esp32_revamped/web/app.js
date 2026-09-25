// VdMot Revamped dashboard. Vanilla JS, no build step, no external resources;
// talks to the /api/* endpoints (DESIGN.md "HTTP API"). It is embedded
// gzip-compressed, so it stays small and dependency-free. Every value from
// the device is rendered with textContent (names are user supplied).
"use strict";
(() => {

// ------------------------------------------------------------------ helpers

const $ = (id) => document.getElementById(id);
const VALVES = 12, TEMP_SLOTS = 34, VOLT_SLOTS = 8;
const MAX_EVENT_ROWS = 500;

function h(tag, props, ...kids) {
  const e = document.createElement(tag);
  if (props) {
    for (const k of Object.keys(props)) {
      const v = props[k];
      if (v === null || v === undefined || v === false) continue;
      if (k === "class") e.className = v;
      else if (k === "text") e.textContent = v;
      else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
      else e.setAttribute(k, v === true ? "" : String(v));
    }
  }
  for (const c of kids.flat()) {
    if (c === null || c === undefined || c === false) continue;
    e.append(c instanceof Node ? c : String(c));
  }
  return e;
}

const SVGNS = "http://www.w3.org/2000/svg";
function s(tag, attrs, text) {
  const e = document.createElementNS(SVGNS, tag);
  for (const k of Object.keys(attrs || {})) e.setAttribute(k, attrs[k]);
  if (text !== undefined) e.textContent = text;
  return e;
}

const isNum = (x) => typeof x === "number" && Number.isFinite(x);
const isInt = (x) => Number.isInteger(x);
function fmt(x, digits, unit) {
  if (!isNum(x)) return "–";
  return (digits ? x.toFixed(digits) : String(Math.round(x))) + (unit ? "\u00a0" + unit : "");
}
function fmtBytes(n) {
  if (!isNum(n)) return "–";
  if (n < 1024) return n + " B";
  if (n < 1048576) return (n / 1024).toFixed(1) + " KiB";
  return (n / 1048576).toFixed(2) + " MiB";
}
function fmtUptime(sec) {
  if (!isNum(sec) || sec < 0) return "–";
  sec = Math.floor(sec);
  const d = Math.floor(sec / 86400), hh = Math.floor(sec / 3600) % 24;
  const mm = String(Math.floor(sec / 60) % 60).padStart(2, "0");
  if (d > 0) return d + "d " + hh + ":" + mm;
  return hh + ":" + mm + ":" + String(sec % 60).padStart(2, "0");
}
function fmtAge(sec) {
  if (!isNum(sec)) return "–";
  if (sec < 120) return sec + " s";
  if (sec < 7200) return Math.round(sec / 60) + " min";
  return Math.round(sec / 3600) + " h";
}
function fmtEpoch(t) {
  if (!isNum(t) || t <= 0) return "–";
  return new Date(t * 1000).toLocaleString();
}
function setText(el, t) { if (el.textContent !== t) el.textContent = t; }
function setChip(el, text, cls) {
  setText(el, text);
  const c = "chip" + (cls ? " " + cls : "");
  if (el.className !== c) el.className = c;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function lsGet(k) { try { return localStorage.getItem(k); } catch { return null; } }
function lsSet(k, v) { try { localStorage.setItem(k, v); } catch { /* private mode */ } }

// ------------------------------------------------------------------ API

class ApiError extends Error {
  constructor(status, data, retryAfter) {
    super(apiMessage(status, data, retryAfter));
    this.status = status;
    this.data = data || null;
    this.retryAfter = retryAfter || 0;
  }
}

let fsFree = null;  // free LittleFS bytes from the last /api/files
const MSG_503 = { queue_full: "The STM command queue is full; try again in a moment",
  busy: "The device is busy with other requests; nothing was changed, try again", retry: "The device state changed; try again" };
function apiMessage(status, d, retryAfter) {
  const code = d && typeof d.error === "string" ? d.error : "";
  const detail = d && typeof d.detail === "string" ? d.detail : "";
  if (status === 0) return code === "timeout" ? "The device did not answer in time" : "The device is not reachable";
  if (status === 401) return "Login required: reload the page and sign in";
  if (status === 403 && code === "host_not_allowed") return "This device does not accept requests for this host name: " + detail;
  if (status === 403 && code === "origin_not_allowed") return "Request from another web page refused: " + detail;
  if (status === 403 && code === "header_required") return "The request lacks the X-VdMot header: reload the page";
  if (status === 403 && code === "auth_required") return "Set a web user and password (Settings, Web access) to export passwords";
  if (status === 410) return "Removed: use " + detail;
  if (status === 415) return "The device expects JSON: " + detail;
  if (status === 429) return "Too many failed logins from this address; try again in " + (retryAfter || 60) + " s";
  if (status === 409 && code === "stm_unsupported") return detail;
  if (status === 501) return "Not supported by this firmware";
  if (status === 503) return MSG_503[code] || "The device is busy; try again";
  if (status === 507) return detail === "too_many_images" ? "Only 3 STM images can be stored; delete one first" :
    "Not enough space on the device" + (isNum(fsFree) ? " (" + fmtBytes(fsFree) + " free)" : "");
  if (status === 413) return "Too large for the device";
  let m = code ? code.replace(/_/g, " ") : "HTTP " + status;
  if (detail) m += ": " + detail;
  return m;
}

async function api(method, path, body, timeoutMs) {
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs || 10000);
  const init = { method, cache: "no-store", signal: ctl.signal, headers: { "X-VdMot": "1" } };
  if (body !== undefined) {
    init.headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  let r, text;
  try {
    r = await fetch(path, init);
    text = await r.text();
  } catch (e) {
    throw new ApiError(0, { error: e && e.name === "AbortError" ? "timeout" : "network" });
  } finally {
    clearTimeout(timer);
  }
  let data = null;
  if (text) { try { data = JSON.parse(text); } catch { data = null; } }
  if (!r.ok) throw new ApiError(r.status, data, Number(r.headers.get("Retry-After")) || 0);
  return data;
}

// Multipart upload with progress (fetch has no upload progress).
function upload(url, field, file, fileName, onProgress, headers) {
  return new Promise((resolve, reject) => {
    const x = new XMLHttpRequest();
    x.open("POST", url);
    x.timeout = 600000;
    x.setRequestHeader("X-VdMot", "1");
    for (const k of Object.keys(headers || {})) x.setRequestHeader(k, headers[k]);
    x.upload.onprogress = (e) => { if (e.lengthComputable && e.total > 0) onProgress(e.loaded / e.total); };
    x.onload = () => {
      let d = null;
      try { d = x.responseText ? JSON.parse(x.responseText) : null; } catch { d = null; }
      if (x.status >= 200 && x.status < 300) resolve(d);
      else reject(new ApiError(x.status, d, Number(x.getResponseHeader("Retry-After")) || 0));
    };
    x.onerror = () => reject(new ApiError(0, { error: "network" }));
    x.ontimeout = () => reject(new ApiError(0, { error: "timeout" }));
    const fd = new FormData();
    fd.append(field, file, fileName);
    x.send(fd);
  });
}

// ------------------------------------------------------------------ UI bits

function toast(msg, isErr) {
  const t = h("div", { class: "toast" + (isErr ? " err" : "") , text: msg });
  const box = $("toasts");
  box.append(t);
  while (box.childNodes.length > 4) box.firstChild.remove();
  setTimeout(() => t.remove(), isErr ? 8000 : 4000);
}
const fail = (e) => toast(e instanceof Error ? e.message : String(e), true);

const hasDialog = typeof HTMLDialogElement === "function" &&
  typeof HTMLDialogElement.prototype.showModal === "function";

// Resolves true when the user confirmed. opt: {title, text, ok, danger,
// typed (text the user must type), extra (nodes shown under the text)}.
function confirmDlg(opt) {
  const dlg = $("dlg-confirm");
  if (!hasDialog) {
    if (opt.typed) return Promise.resolve(window.prompt(opt.text + "\n\nType " + opt.typed) === opt.typed);
    return Promise.resolve(window.confirm(opt.text));
  }
  $("dc-title").textContent = opt.title;
  $("dc-text").textContent = opt.text;
  const extra = $("dc-extra");
  extra.textContent = "";
  if (opt.extra) extra.append(...opt.extra);
  const ok = $("dc-ok");
  ok.textContent = opt.ok || "OK";
  ok.className = opt.danger ? "danger primary" : "primary";
  const typed = $("dc-typed"), inp = $("dc-input");
  typed.hidden = !opt.typed;
  inp.value = "";
  ok.disabled = !!opt.typed;
  if (opt.typed) {
    $("dc-input-label").textContent = "Type " + opt.typed + " to confirm";
    inp.oninput = () => { ok.disabled = inp.value !== opt.typed; };
  }
  return new Promise((resolve) => {
    dlg.returnValue = "";
    dlg.addEventListener("close", () => {
      resolve(dlg.returnValue === "ok" && (!opt.typed || inp.value === opt.typed));
    }, { once: true });
    dlg.showModal();
  });
}

for (const b of document.querySelectorAll("dialog [data-close]")) {
  b.addEventListener("click", () => b.closest("dialog").close("cancel"));
}

// Runs an action with its button disabled, reports errors as toasts.
async function busy(btn, fn) {
  if (btn) btn.disabled = true;
  try { return await fn(); } catch (e) { fail(e); return undefined; } finally { if (btn) btn.disabled = false; }
}

// ------------------------------------------------------------------ theme

const THEMES = ["auto", "light", "dark"];
function applyTheme(t) {
  if (!THEMES.includes(t)) t = "auto";
  if (t === "auto") document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", t);
  $("theme").textContent = "Theme: " + t;
  return t;
}
let theme = applyTheme(lsGet("vdm.theme") || "auto");
$("theme").addEventListener("click", () => {
  theme = applyTheme(THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length]);
  lsSet("vdm.theme", theme);
});

// ------------------------------------------------------------------ polling

const pollers = [];
function poller(fn, ms, when) {
  const p = { fn, ms, when, busy: false, next: 0, fails: 0 };
  pollers.push(p);
  return p;
}
async function runPoller(p) {
  if (p.busy) return;
  p.busy = true;
  try {
    await p.fn();
    p.fails = 0;
  } catch (e) {
    p.fails++;
    if (!(e instanceof ApiError && e.status === 0)) console.warn(e);
  } finally {
    p.busy = false;
    p.next = Date.now() + p.ms * Math.min(1 + p.fails, 5);
  }
}
function kick(p) { p.next = 0; if (!document.hidden && (!p.when || p.when())) runPoller(p); }
setInterval(() => {
  if (document.hidden) return;
  const now = Date.now();
  for (const p of pollers) if (now >= p.next && (!p.when || p.when())) runPoller(p);
}, 250);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) for (const p of pollers) kick(p);
});

// ------------------------------------------------------------------ views

const VIEWS = ["valves", "sensors", "events", "settings", "maintenance"];
let view = "";
const visible = (v) => () => view === v;
function showView() {
  let v = location.hash.replace(/^#/, "");
  if (!VIEWS.includes(v)) v = "valves";
  if (v === view) return;
  view = v;
  for (const x of VIEWS) $("v-" + x).hidden = x !== v;
  for (const a of document.querySelectorAll("nav.tabs a")) {
    if (a.getAttribute("href") === "#" + v) a.setAttribute("aria-current", "page");
    else a.removeAttribute("aria-current");
  }
  if (v === "settings" && !cfgLoaded) loadSettings();
  if (v === "maintenance" && S) renderInfo(S);
  if (v === "events") { ev.seen = Math.max(ev.seen, lastEventSeq); $("ev-dot").hidden = true; }
  for (const p of pollers) if (p.when && p.when()) kick(p);
}
window.addEventListener("hashchange", showView);

// ------------------------------------------------------------------ status

let S = null;            // last /api/status
let lastEventSeq = 0;
let restartWait = null;  // {label, base uptime, boots, t0} while a restart is expected

// Stacked notices under the banner: id -> element, rebuilt when the text changes.
function notice(id, show, cls, text, buttons) {
  let el = $("n-" + id);
  if (!show) { if (el) el.remove(); return; }
  const key = cls + "|" + text;
  if (el && el.dataset.k === key) return;
  const nel = h("div", { id: "n-" + id, class: "notice " + cls, role: "status" }, h("p", { text }),
    buttons && buttons.length ? h("div", { class: "row" }, buttons) : null);
  nel.dataset.k = key;
  if (el) el.replaceWith(nel); else $("notices").append(nel);
}
function ssGet(k) { try { return sessionStorage.getItem(k); } catch { return null; } }
function ssSet(k, v) { try { sessionStorage.setItem(k, v); } catch { /* private mode */ } }
const btn = (text, onclick, cls) => h("button", { type: "button", class: cls || "", onclick }, text);
let importReport = null;  // /api/import-report, loaded once while status.importReport
const DROPPED_TXT = { pi: "PI control", window: "window contacts", messenger: "messenger", ds18Timeout: "sensor timeout",
  legacyFailsafe: "MQTT timeout position" };
function importText(r) {
  if (!r || typeof r !== "object") return "The configuration was imported from the legacy firmware.";
  const parts = ["The configuration was imported from the legacy firmware: " + fmt(r.imported) + " settings taken, " +
    fmt(r.rejected) + " rejected" + (r.firstRejected ? " (first: " + r.firstRejected + ")" : "") + ", " + fmt(r.ignored) + " ignored."];
  if (Array.isArray(r.dropped) && r.dropped.length) parts.push("Not available here: " + r.dropped.map((x) => DROPPED_TXT[x] || x).join(", ") + ".");
  if (r.legacyFailsafe && typeof r.legacyFailsafe === "object") parts.push("The legacy MQTT timeout (" + fmt(r.legacyFailsafe.timeoutMin) +
    " min, " + fmt(r.legacyFailsafe.pct) + " %) was replaced by the failsafe settings (Settings, Failsafe).");
  if (typeof r.rootTopic === "string" && r.rootTopic) parts.push("MQTT root topic kept: " + r.rootTopic + ".");
  if (Array.isArray(r.renamed) && r.renamed.length) parts.push("Renamed for MQTT: " + r.renamed.map((x) => x.kind + " " + x.n + " " + x.name +
    (x.topic ? " (topic " + x.topic + ")" : "")).join(", ") + ".");
  return parts.join(" ");
}
function renderNotices(st) {
  const net = st.net || {}, stm = st.stm || {}, mq = st.mqtt || {}, c = st.config || {};
  notice("auth", st.auth === false && !(Number(lsGet("vdm.authNoticeUntil")) > Date.now()), "info",
    "Login is off: anyone on your network can change settings, reset the STM or flash firmware. Set a user and a password under Settings -> Web access.",
    [btn("Settings", () => { location.hash = "#settings"; }),
      btn("Hide for 30 days", () => { lsSet("vdm.authNoticeUntil", String(Date.now() + 30 * 86400000)); renderNotices(S); })]);
  if (st.importReport === true && importReport === null) {
    importReport = false;
    api("GET", "/api/import-report").then((r) => { importReport = r; renderNotices(S); }).catch(() => { importReport = null; });
  }
  notice("import", st.importReport === true, "info", importText(importReport),
    [btn("Dismiss", (e) => busy(e.currentTarget, async () => { await api("DELETE", "/api/import-report"); importReport = null; kick(pStatus); }))]);
  notice("backup", c.source === "backup" && ssGet("vdm.n3") !== "1", "warn",
    "The stored configuration could not be read; the device restored its backup copy.",
    [btn("Dismiss", () => { ssSet("vdm.n3", "1"); renderNotices(S); })]);
  notice("newer", c.newerSchema === true, "warn",
    "The configuration was written by a newer firmware. Saving here drops the settings this firmware does not know.");
  const trial = net.trial && typeof net.trial === "object" ? net.trial : null;
  notice("trial", !!trial, "warn", "New network settings are on trial: keep them? The previous settings return in " +
    (trial ? fmt(trial.remainS) : "?") + " s.",
  [btn("Keep these settings", (e) => busy(e.currentTarget, async () => { await api("POST", "/api/system/network/confirm"); toast("Network settings kept"); kick(pStatus); }), "primary"),
    btn("Revert now", (e) => busy(e.currentTarget, async () => { await api("POST", "/api/system/network/revert"); expectRestart("Network revert"); }))]);
  const lease = stm.lease && typeof stm.lease === "object" ? stm.lease : null;
  notice("failsafe", !!lease && (lease.state === "expired" || (lease.mode === "esp" && lease.failsafeMask > 0)), "warn",
    "Failsafe active: the regulator is silent; valves are at their failsafe positions. Targets set here are stored and applied once the regulator is back. To open a valve now use Assembly (100 % until the next target) or change its failsafe %.");
  notice("safe", !!(stm.status && stm.status.safeMode === true), "err",
    "The STM is in safe mode after repeated watchdog resets: valves stay where they are. Check the wiring, then leave safe mode.",
    [btn("Leave safe mode", (e) => simpleAction(e.currentTarget, { title: "Leave safe mode?", text: "The STM resumes normal valve control.", ok: "Leave safe mode" },
      "POST", "/api/stm/safe-mode/leave", undefined, "Leaving safe mode", () => kick(pStatus)))]);
  notice("ha", mq.haStatus === "offline" && mq.state === "connected", "warn",
    "Home Assistant reports offline while commands arrive: enable retained birth/last will in HA.");
}

// E19: a new ESP firmware comes with its own dashboard. Reload once per
// version, or ask when the settings form has unsaved changes.
let espBuild = null;
function checkVersion(esp) {
  const ver = String(esp.version) + "|" + String(esp.build);
  if (espBuild === null) { espBuild = ver; return; }
  if (ver === espBuild) return;
  if (!dirtyCount() && ssGet("vdm.reloadedFor") !== ver) { ssSet("vdm.reloadedFor", ver); location.reload(); return; }
  notice("version", true, "info", "The ESP now runs " + esp.version + ". Reload the page to use the matching dashboard.",
    [btn("Reload", () => { ssSet("vdm.reloadedFor", ver); location.reload(); }, "primary")]);
}

function banner(text, isErr) {
  const b = $("banner");
  b.hidden = !text;
  b.textContent = text || "";
  b.className = "banner" + (isErr ? " err" : "");
}

const LINK_CLS = { up: "ok", degraded: "warn", booting: "warn", unknown: "", down: "err", suspended: "info" };
const MQTT_CLS = { connected: "ok", connecting: "warn", error: "err", disabled: "" };

async function pollStatus() {
  let st;
  try {
    st = await api("GET", "/api/status", undefined, 6000);
  } catch (e) {
    if (e.status === 0) {
      setChip($("st-net"), "offline", "err");
      if (!restartWait) banner("The device is not reachable. Retrying…", true);
    } else if (e.status === 401) {
      banner("Login required: reload the page and sign in.", true);
    }
    throw e;
  }
  if (!st || typeof st !== "object" || !st.esp) throw new Error("bad status");
  checkVersion(st.esp);
  const protoChanged = !S || !S.stm || S.stm.proto !== (st.stm && st.stm.proto);
  S = st;
  renderStatus(st);
  if (protoChanged && V.length) renderValves();
  if (restartWait && restarted(st)) {
    toast(restartWait.label + ": the ESP is back (" + st.esp.version + ")");
    restartWait = null;
  }
  renderNotices(st);
  if (!restartWait) {
    const msgs = [];
    if (st.config && st.config.source === "defaults_after_error") {
      msgs.push("The configuration could not be read and no backup exists: the device runs with defaults. Import a saved configuration under Maintenance.");
    }
    if (st.stm && st.stm.support === "too_old") {
      msgs.push("STM firmware " + (st.stm.version || "") + " is older than 1.4.0: update the STM.");
    } else if (st.stm && st.stm.compatible === false) {
      msgs.push("The STM firmware " + (st.stm.version || "") + " is older than the supported minimum " +
        (st.stm.minVersion || "") + ". Update it under Maintenance.");
    }
    if (st.stm && st.stm.link === "down") msgs.push("The STM does not answer.");
    banner(msgs.join(" "), (st.stm && st.stm.link === "down") || (st.config && st.config.source === "defaults_after_error"));
  }
}

function renderStatus(st) {
  const net = st.net || {}, stm = st.stm || {}, mq = st.mqtt || {}, esp = st.esp || {}, tm = st.time || {};
  const station = st.station || net.hostname || "VdMot";
  setText($("station"), station);
  document.title = station + " · VdMot Revamped";
  let netText = net.state === "down" || !net.state ? "network down" : net.state;
  if (net.ip && net.state !== "down") netText += " " + net.ip;
  if (net.state === "wifi" && isNum(net.rssi)) netText += " (" + net.rssi + " dBm)";
  setChip($("st-net"), netText, net.state === "down" ? "err" : "ok");
  setChip($("st-link"), "STM " + (stm.link === "suspended" ? "flashing" : stm.link || "?"), LINK_CLS[stm.link] || "");
  setChip($("st-mqtt"), "MQTT " + (mq.state || "?"), MQTT_CLS[mq.state] || "");
  const cal = st.calibration && st.calibration.active;
  $("st-cal-li").hidden = !cal;
  setText($("st-time"), tm.valid && tm.local ? tm.local.replace("T", " ") : "time not synced");
  setText($("st-uptime"), fmtUptime(esp.uptime));
  const espV = esp.version || "?";
  const stmV = (stm.version || "unknown") + (isNum(stm.proto) && stm.proto > 0 ? " (proto " + stm.proto + ")" : "");
  setText($("st-esp"), espV);
  setText($("st-stm"), stmV);
  setText($("ft-esp"), espV + (isNum(esp.build) ? " built " + fmtEpoch(esp.build) : ""));
  setText($("ft-stm"), stmV);
  if (isNum(st.lastEventSeq)) {
    if (ev.seen === 0 && lastEventSeq === 0) ev.seen = st.lastEventSeq;  // no dot for history
    lastEventSeq = st.lastEventSeq;
    $("ev-dot").hidden = view === "events" || lastEventSeq <= ev.seen;
  }
  if (view === "maintenance") renderInfo(st);
  renderSummary();
}

// Waits for the ESP to come back after a restart it announced. The old
// firmware's uptime keeps growing with wall time; a fresh boot reports less.
function expectRestart(label) {
  const base = S && isNum(S.esp.uptime) ? S.esp.uptime : 0;
  restartWait = { label, base, boots: S ? S.esp.boots : undefined, t0: Date.now() };
  banner(label + ": waiting for the ESP to restart…", false);
  const w = restartWait;
  setTimeout(() => {
    if (restartWait !== w) return;
    restartWait = null;
    banner("The ESP did not come back within 3 minutes. Check the device.", true);
  }, 180000);
}
function restarted(st) {
  const w = restartWait, up = st.esp.uptime;
  if (w.boots !== undefined && st.esp.boots !== undefined && st.esp.boots !== w.boots) return true;
  return isNum(up) && up + 2 < w.base + (Date.now() - w.t0) / 1000;
}

// ------------------------------------------------------------------ valves

let V = [];              // last /api/valves entries (index = idx - 1)
const cards = [];
const STATE_CLS = { idle: "ok", opening: "info", closing: "info", connected: "info", fullopen: "info",
  failed: "err", blocked: "err", invalid: "err", novalve: "warn", unknown: "warn", nodata: "" };
const STATE_TXT = { nodata: "no data", fullopen: "full open", novalve: "no valve" };
const HEALTH = { blocked: ["blocked", "err"], failed: ["failed", "err"], noValve: ["no valve", "warn"],
  calibRetries: ["calibration retries", "warn"], earlyStop: ["early stop", "warn"],
  cmdRejected: ["commands rejected", "warn"], stale: ["no fresh data", "warn"],
  targetUnconfirmed: ["target not confirmed", "warn"], tempFailed: ["sensor failed", "warn"],
  calEarlyStop: ["early end stop since calibration", "warn"], calLastFailed: ["last calibration failed", "err"],
  strokeShort: ["stroke close to minCounts: calibration may fail", "warn"] };
const STOP_TXT = { none: "–", target: "target reached", endstop: "end stop", early_endstop: "early end stop",
  timeout: "timeout", undercurrent: "no motor current", safety_overcurrent: "over-current limit", aborted: "aborted" };
const SYNC_TXT = { pending: "target pending", await_ack: "sending target", await_verify: "verifying target",
  failed: "target not confirmed" };

function valveLabel(i) {
  const v = V[i - 1];
  return "valve " + i + (v && v.name ? " (" + v.name + ")" : "");
}

function kvItem(label) {
  const dd = h("dd", { text: "–" });
  return [h("div", null, h("dt", { text: label }), dd), dd];
}

function buildCard(i) {
  const c = { i };
  c.name = h("span");
  c.state = h("span", { class: "chip" });
  c.cal = h("span", { class: "chip info", hidden: true, text: "calibrating" });
  c.sync = h("span", { class: "chip warn", hidden: true });
  c.pos = h("span", { class: "big num" });
  c.tgt = h("span", { class: "muted" });
  c.bar = h("i");
  c.mark = h("b");
  const items = [["Mean current", "mean"], ["Open/close", "occ"], ["Dead zone", "dc"], ["Retries", "cr"],
    ["Moves", "moves"], ["Early stops", "early"], ["Rejected", "rej"], ["Data age", "age"]];
  const dl = h("dl", { class: "kv" });
  for (const [label, key] of items) { const [div, dd] = kvItem(label); dl.append(div); c[key] = dd; }
  c.last = h("p", { class: "lastmove" });
  c.temps = h("p", { class: "temps" });
  c.flags = h("div", { class: "flags" });
  const inId = "tg-" + i;
  c.fs = h("span", { class: "chip warn", hidden: true });
  c.input = h("input", { id: inId, type: "text", inputmode: "decimal", min: 0, max: 100, step: "any",
    "aria-label": "Target for valve " + i + " in percent" });
  c.input.addEventListener("input", () => { c.dirty = true; });
  c.input.addEventListener("keydown", (e) => { if (e.key === "Enter") setTarget(c); });
  c.setBtn = h("button", { type: "button", class: "primary", onclick: () => setTarget(c) }, "Set");
  c.btns = [
    c.setBtn,
    h("button", { type: "button", onclick: (e) => valveAction(e.currentTarget, i, "calibrate") }, "Calibrate"),
    h("button", { type: "button", onclick: (e) => valveAction(e.currentTarget, i, "assembly") }, "Assembly"),
    c.moveBtn = h("button", { type: "button", onclick: () => openMove(i) }, "Service move…"),
    c.profBtn = h("button", { type: "button", onclick: () => openProfile(i) }, "Profile"),
    c.stopBtn = h("button", { type: "button", onclick: (e) => simpleAction(e.currentTarget, null, "POST", "/api/valves/" + i + "/stop",
      undefined, valveLabel(i) + ": stop requested", () => kick(pValves)) }, "Stop"),
  ];
  c.el = h("article", { class: "card", "aria-labelledby": "vt-" + i },
    h("div", { class: "vhead" },
      h("h3", { id: "vt-" + i }, h("span", { class: "idx", text: "#" + i }), c.name),
      c.state, c.cal, c.sync, c.fs),
    h("div", { class: "pos" }, c.pos, c.tgt),
    h("div", { class: "meter", role: "presentation" }, c.bar, c.mark),
    dl, c.last, c.temps, c.flags,
    h("div", { class: "actions" }, h("span", { class: "row" }, c.input, h("span", { "aria-hidden": "true" }, "%")),
      c.btns));
  return c;
}

function updateCard(c, v) {
  const key = typeof v.stateKey === "string" ? v.stateKey : "invalid";
  setText(c.name, v.name || "Valve " + c.i);
  setChip(c.state, v.active ? STATE_TXT[key] || key : "inactive", v.active ? STATE_CLS[key] || "" : "");
  const ext = v.ext && typeof v.ext === "object" ? v.ext : null;
  // ext.calState is the phase only (0 idle, 1 requested, 2 running); the flags are separate.
  c.cal.hidden = !(v.calibrating || (ext && ext.calState > 0));
  if (!c.cal.hidden) setText(c.cal, v.calibrating || (ext && ext.calState >= 2) ? "calibrating" : "calibration queued");
  const syncTxt = v.active ? SYNC_TXT[v.sync] : undefined;
  c.sync.hidden = !syncTxt;
  if (syncTxt) setChip(c.sync, syncTxt, v.sync === "failed" ? "err" : "warn");
  const pos = isNum(v.pos) ? Math.max(0, Math.min(100, v.pos)) : null;
  setText(c.pos, pos === null ? "–" : pos + " %");
  setText(c.tgt, isNum(v.target) ? "target " + v.target + " %" + (v.targetSource && v.targetSource !== "none" ?
    " (" + v.targetSource + ")" : "") : "no target");
  c.bar.style.width = (pos || 0) + "%";
  c.mark.hidden = !isNum(v.target);
  if (isNum(v.target)) c.mark.style.left = Math.max(0, Math.min(100, v.target)) + "%";
  setText(c.mean, fmt(v.meanCur, 0, "mA"));
  setText(c.occ, fmt(v.oc) + " / " + fmt(v.cc));
  setText(c.dc, fmt(v.dc));
  setText(c.cr, fmt(v.cr));
  setText(c.moves, fmt(v.moves));
  setText(c.early, ext ? fmt(ext.earlyStops) : "–");
  setText(c.rej, ext ? fmt(ext.cmdRejected) : "–");
  setText(c.age, fmtAge(v.age));
  const lm = ext && ext.lastMove;
  if (lm && typeof lm === "object") {
    setText(c.last, "Last move: " + (lm.dir === "close" ? "close" : "open") + " " + fmt(lm.cnt) + " of " +
      fmt(lm.req) + " counts, " + (STOP_TXT[lm.stop] || lm.stop || "?") + ", peak " + fmt(lm.peak, 1, "mA") +
      ", " + fmt(isNum(lm.ms) ? lm.ms / 1000 : null, 1, "s"));
    c.last.className = "lastmove" + (["early_endstop", "timeout", "safety_overcurrent", "undercurrent"]
      .includes(lm.stop) ? " err-msg" : "");
  } else {
    setText(c.last, ext ? "Last move: none since the STM started" : "Last move details need STM firmware 2.x");
    c.last.className = "lastmove muted";
  }
  const sensors = Array.isArray(v.sensors) ? v.sensors : [];
  setText(c.temps, sensors.map((x) => "T" + (x.sensor === 2 ? 2 : 1) + " " + (x.name || "slot " + x.slot) + " " +
    (isNum(x.temp) ? x.temp.toFixed(1) + " °C" : "no reading")).join(", "));
  c.temps.hidden = !sensors.length;
  const flags = Array.isArray(v.health) ? v.health.filter((f) => HEALTH[f]) : [];
  if (ext && ext.calEarlyStop === true) flags.push("calEarlyStop");
  if (ext && ext.calLastFailed === true) flags.push("calLastFailed");
  const fk = flags.join();
  if (c.fk !== fk) {
    c.fk = fk;
    c.flags.textContent = "";
    for (const f of flags) c.flags.append(h("span", { class: "chip " + HEALTH[f][1], text: HEALTH[f][0] }));
  }
  c.flags.hidden = !flags.length;
  const fs = v.failsafe && typeof v.failsafe === "object" ? v.failsafe : null;
  c.fs.hidden = !fs || (fs.state !== "lease" && fs.state !== "blocked");
  if (!c.fs.hidden) setChip(c.fs, "failsafe " + (fs.state === "blocked" ? "(blocked) " : "") + (isNum(fs.pct) ? fs.pct + " %" : "hold"),
    fs.state === "blocked" ? "err" : "warn");
  c.el.classList.toggle("bad", !!v.active && (key === "blocked" || key === "failed"));
  c.el.classList.toggle("off", !v.active);
  if (!c.dirty && document.activeElement !== c.input) c.input.value = isNum(v.target) ? v.target : "";
  const proto2 = S && S.stm && S.stm.proto >= 2;
  for (const b of c.btns) b.disabled = !v.active;
  c.input.disabled = !v.active;
  c.moveBtn.disabled = !v.active || !proto2;
  c.moveBtn.title = proto2 ? "" : "Needs STM firmware 2.x";
  c.profBtn.disabled = !proto2;
  c.profBtn.title = proto2 ? "" : "Needs STM firmware 2.x";
  const proto3 = S && S.stm && S.stm.proto >= 3;
  c.stopBtn.hidden = !proto3;
  c.stopBtn.disabled = !v.active;
}

async function pollValves() {
  const d = await api("GET", "/api/valves");
  if (!d || !Array.isArray(d.valves)) throw new Error("bad valves");
  V = d.valves.slice(0, VALVES);
  renderValves();
}

function renderValves() {
  const list = $("valve-list");
  const showAll = $("show-inactive").checked;
  V.forEach((v, n) => {
    if (!v || typeof v !== "object") return;
    const i = n + 1;
    if (!cards[i]) { cards[i] = buildCard(i); list.append(cards[i].el); }
    updateCard(cards[i], v);
    cards[i].el.hidden = !v.active && !showAll;
  });
  if (!V.some((v) => v && v.active) && !showAll && !$("no-active")) {
    list.append(h("p", { id: "no-active", class: "muted" }, "No active valves. Enable valves under Settings, or tick Show inactive."));
  } else if ((V.some((v) => v && v.active) || showAll) && $("no-active")) {
    $("no-active").remove();
  }
  updateValveFilterNames();
  renderSummary();
}
$("show-inactive").addEventListener("change", () => { lsSet("vdm.inactive", $("show-inactive").checked ? "1" : ""); renderValves(); });
$("show-inactive").checked = lsGet("vdm.inactive") === "1";

function renderSummary() {
  const box = $("valve-summary");
  const act = V.filter((v) => v && v.active);
  const parts = [act.length + " active"];
  const bad = act.filter((v) => v.stateKey === "blocked" || v.stateKey === "failed").length;
  if (bad) parts.push(bad + " blocked/failed");
  const warn = act.filter((v) => Array.isArray(v.health) && v.health.length).length - bad;
  if (warn > 0) parts.push(warn + " with warnings");
  if (S && S.calibration) {
    if (S.calibration.active) parts.push("calibration running");
    const n = S.calibration.nextSlot;
    if (isInt(n) && n > 19700101) {
      parts.push("next scheduled calibration " + String(n).replace(/^(\d{4})(\d{2})(\d{2})$/, "$1-$2-$3"));
    }
  }
  setText(box, parts.join(" · "));
}

// Decimals with '.' or ',' are rounded by the device (43.5 -> 44).
async function setTarget(c) {
  const raw = c.input.value.trim().replace(",", ".");
  const t = Number(raw);
  if (!/^\d{1,3}(\.\d+)?$/.test(raw) || t > 100) {
    c.input.setAttribute("aria-invalid", "true");
    toast("Target must be a number from 0 to 100", true);
    c.input.focus();
    return;
  }
  c.input.removeAttribute("aria-invalid");
  await busy(c.setBtn, async () => {
    const r = await api("POST", "/api/valves/" + c.i + "/target", { target: t });
    const sent = r && isNum(r.target) ? r.target : Math.round(t);
    c.dirty = false;
    c.input.value = sent;
    const lease = S && S.stm && S.stm.lease;
    const pending = lease && (lease.state === "expired" || (lease.mode === "esp" && lease.failsafeMask > 0));
    toast(valveLabel(c.i) + ": target " + sent + " % " + (pending ? "stored, pending until the regulator is back" : "sent"));
    kick(pValves);
  });
}

const ACTION_TXT = {
  calibrate: ["Calibrate", "The valve closes and opens fully to learn its end positions. This takes a few minutes; the target is restored afterwards."],
  assembly: ["Assembly position", "The valve opens fully so the motor head can be mounted or removed. Set a target afterwards to resume normal operation."],
};
async function valveAction(btn, i, what) {
  const [title, text] = ACTION_TXT[what];
  if (!await confirmDlg({ title: title + " " + valveLabel(i) + "?", text, ok: title })) return;
  await busy(btn, async () => {
    await api("POST", "/api/valves/" + i + "/" + what);
    toast(valveLabel(i) + ": " + title.toLowerCase() + " requested");
    kick(pValves);
  });
}

// service move dialog
let moveValve = 0;
function openMove(i) {
  if (!hasDialog) { toast("This browser does not support dialogs", true); return; }
  moveValve = i;
  $("dm-title").textContent = "Service move: " + valveLabel(i);
  $("dm-ack").checked = false;
  $("dm-go").disabled = true;
  for (const id of ["dm-counts", "dm-ma"]) { $(id).removeAttribute("aria-invalid"); $(id + "-e").textContent = ""; }
  $("dlg-move").showModal();
}
$("dm-ack").addEventListener("change", () => { $("dm-go").disabled = !$("dm-ack").checked; });
function intIn(id, min, max) {
  const el = $(id), raw = el.value.trim(), n = Number(raw);
  const ok = /^\d{1,5}$/.test(raw) && n >= min && n <= max;
  el.setAttribute("aria-invalid", ok ? "false" : "true");
  $(id + "-e").textContent = ok ? "" : "Whole number " + min + "–" + max;
  return ok ? n : null;
}
$("dm-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const dlg = $("dlg-move");
  if (e.submitter && e.submitter.value !== "ok") { dlg.close(); return; }
  const counts = intIn("dm-counts", 1, 10000), maxmA = intIn("dm-ma", 5, 60);
  if (counts === null || maxmA === null || !$("dm-ack").checked) return;
  const dir = dlg.querySelector("input[name=dir]:checked").value === "close" ? "close" : "open";
  const i = moveValve;
  await busy($("dm-go"), async () => {
    await api("POST", "/api/valves/" + i + "/service-move", { dir, counts, maxmA });
    dlg.close();
    toast(valveLabel(i) + ": service move " + dir + " " + counts + " counts started");
    kick(pValves);
  });
});

// profile dialog + chart
let profValve = 0;
async function openProfile(i) {
  if (!hasDialog) { toast("This browser does not support dialogs", true); return; }
  profValve = i;
  $("dp-title").textContent = "Current profile: " + valveLabel(i);
  $("dp-refresh").disabled = !(S && S.stm && S.stm.proto >= 2);
  $("dlg-profile").showModal();
  await loadProfile(i);
}
async function loadProfile(i) {
  const body = $("dp-body");
  body.textContent = "Loading…";
  try {
    const p = await api("GET", "/api/valves/" + i + "/profile");
    if (i !== profValve) return;
    body.textContent = "";
    const samples = p && Array.isArray(p.samples) ? p.samples.filter((x) => Array.isArray(x) && isNum(x[0]) && isNum(x[1])) : [];
    if (!samples.length) { body.textContent = "The profile of the last move is empty."; return; }
    const v = V[i - 1];
    body.append(chart(samples.map((x) => [x[0], x[1] / 10]), v && isNum(v.meanCur) ? v.meanCur : null));
  } catch (e) {
    if (i !== profValve) return;
    body.textContent = e.status === 404 ? "No profile recorded yet. It is kept for the last move after the STM started (STM 2.x)." : e.message;
  }
}
$("dp-refresh").addEventListener("click", (e) => busy(e.currentTarget, async () => {
  const i = profValve;
  await api("POST", "/api/valves/" + i + "/profile");
  await sleep(1500);
  if (i === profValve && $("dlg-profile").open) await loadProfile(i);
}));

function niceStep(max, n) {
  const raw = max / n, p = Math.pow(10, Math.floor(Math.log10(raw)));
  const m = raw / p;
  return (m <= 1 ? 1 : m <= 2 ? 2 : m <= 5 ? 5 : 10) * p;
}

// Single-series line chart: counts (x) vs motor current in mA (y).
function chart(pts, mean) {
  const W = 560, H = 240, L = 40, R = 12, T = 12, B = 30;
  const xMax = Math.max(1, ...pts.map((p) => p[0]));
  const peak = pts.reduce((a, p) => (p[1] > a[1] ? p : a), pts[0]);
  const yStep = niceStep(Math.max(peak[1], mean || 0, 1), 4);
  const yMax = Math.ceil(Math.max(peak[1], mean || 0, 1) / yStep) * yStep;
  const xStep = niceStep(xMax, 4);
  const X = (x) => L + (x / xMax) * (W - L - R), Y = (y) => T + (1 - y / yMax) * (H - T - B);
  const svg = s("svg", { viewBox: "0 0 " + W + " " + H, class: "chart", role: "img", tabindex: "0",
    "aria-label": "Motor current over " + pts.length + " samples; peak " + peak[1].toFixed(1) + " mA at count " +
      peak[0] + ". Use arrow keys to read samples." });
  for (let y = 0; y <= yMax + 1e-9; y += yStep) {
    svg.append(s("line", { class: "gl", x1: L, x2: W - R, y1: Y(y), y2: Y(y) }));
    svg.append(s("text", { x: L - 6, y: Y(y) + 4, "text-anchor": "end" }, String(+y.toFixed(1))));
  }
  for (let x = 0; x <= xMax + 1e-9; x += xStep) {
    svg.append(s("text", { x: X(x), y: H - 12, "text-anchor": "middle" }, String(Math.round(x))));
  }
  svg.append(s("text", { x: W - R, y: H - 1, "text-anchor": "end" }, "counts"));
  svg.append(s("text", { x: 2, y: 10 }, "mA"));
  if (isNum(mean) && mean > 0) {
    svg.append(s("line", { class: "ref", x1: L, x2: W - R, y1: Y(mean), y2: Y(mean) }));
    svg.append(s("text", { x: W - R, y: Y(mean) - 4, "text-anchor": "end" }, "mean " + mean + " mA"));
  }
  svg.append(s("path", { class: "ln", d: pts.map((p, k) => (k ? "L" : "M") + X(p[0]).toFixed(1) + " " + Y(p[1]).toFixed(1)).join("") }));
  const hair = s("line", { class: "hair", y1: T, y2: H - B, visibility: "hidden" });
  const dot = s("circle", { class: "dot", r: 5, visibility: "hidden" });
  svg.append(hair, dot);
  const tip = h("p", { class: "tip", "aria-live": "polite" }, "Peak " + peak[1].toFixed(1) + " mA at count " + peak[0]);
  let sel = -1;
  const pick = (k) => {
    sel = Math.max(0, Math.min(pts.length - 1, k));
    const p = pts[sel];
    for (const a of [["x1", X(p[0])], ["x2", X(p[0])]]) hair.setAttribute(a[0], a[1]);
    dot.setAttribute("cx", X(p[0]));
    dot.setAttribute("cy", Y(p[1]));
    hair.setAttribute("visibility", "visible");
    dot.setAttribute("visibility", "visible");
    tip.textContent = "Sample " + (sel + 1) + ": count " + p[0] + ", " + p[1].toFixed(1) + " mA";
  };
  svg.addEventListener("pointermove", (e) => {
    const r = svg.getBoundingClientRect();
    if (!r.width) return;
    const x = ((e.clientX - r.left) / r.width * W - L) / (W - L - R) * xMax;
    let best = 0;
    pts.forEach((p, k) => { if (Math.abs(p[0] - x) < Math.abs(pts[best][0] - x)) best = k; });
    pick(best);
  });
  svg.addEventListener("keydown", (e) => {
    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      e.preventDefault();
      pick(sel < 0 ? 0 : sel + (e.key === "ArrowRight" ? 1 : -1));
    }
  });
  const rows = pts.map((p, k) => h("tr", null, h("td", { class: "r", text: k + 1 }), h("td", { class: "r", text: p[0] }),
    h("td", { class: "r", text: p[1].toFixed(1) })));
  const table = h("details", null, h("summary", { text: "Samples as a table" }),
    h("div", { class: "tbl-wrap" }, h("table", null,
      h("thead", null, h("tr", null, h("th", { class: "r", text: "#" }), h("th", { class: "r", text: "Count" }),
        h("th", { class: "r", text: "mA" }))), h("tbody", null, rows))));
  return h("div", null, svg, tip, table);
}

// ------------------------------------------------------------------ sensors

async function pollSensors() {
  const d = await api("GET", "/api/sensors");
  if (!d) throw new Error("bad sensors");
  const tb = $("temps-body"), vb = $("volts-body");
  const temps = Array.isArray(d.temps) ? d.temps : [], volts = Array.isArray(d.volts) ? d.volts : [];
  const state = (x) => {
    if (!x.onBus) return h("span", { class: "chip " + (x.active ? "err" : ""), text: "not on bus" });
    if (x.slot === null || x.slot === undefined) return h("span", { class: "chip warn", text: "not configured" });
    return h("span", { class: "chip " + (x.active ? "ok" : ""), text: x.active ? "active" : "inactive" });
  };
  const vname = (n) => (isInt(n) && n >= 1 && n <= VALVES ? "#" + n + (V[n - 1] && V[n - 1].name ? " " + V[n - 1].name : "") : "–");
  tb.replaceChildren(...temps.map((x) => h("tr", null,
    h("td", { text: isInt(x.slot) ? x.slot : "–" }), h("td", { text: x.name || "" }),
    h("td", { class: "r", text: isNum(x.temp) ? x.temp.toFixed(1) + " °C" : "–" }),
    h("td", { text: vname(x.valve) }), h("td", null, state(x)),
    h("td", { class: "r", text: fmt(x.raw) }), h("td", { class: "r", text: fmtAge(x.age) }),
    h("td", { class: "mono", text: x.id || "" }))));
  if (!temps.length) tb.append(h("tr", null, h("td", { colspan: 8, class: "muted", text: "No temperature sensors found." })));
  vb.replaceChildren(...volts.map((x) => h("tr", null,
    h("td", { text: isInt(x.slot) ? x.slot : "–" }), h("td", { text: x.name || "" }),
    h("td", { class: "r", text: isNum(x.value) ? x.value.toFixed(3) + (x.unit ? " " + x.unit : "") : "–" }),
    h("td", null, state(x)), h("td", { class: "r", text: fmt(x.raw) }), h("td", { class: "r", text: fmtAge(x.age) }),
    h("td", { class: "mono", text: x.id || "" }))));
  if (!volts.length) vb.append(h("tr", null, h("td", { colspan: 7, class: "muted", text: "No voltage sensors found." })));
  busIds = temps.filter((x) => x.onBus && x.id).map((x) => x.id);
  busVoltIds = volts.filter((x) => x.onBus && x.id).map((x) => x.id);
  fillDatalists();
}
$("scan-bus").addEventListener("click", async (e) => {
  const btn = e.currentTarget;
  if (!await confirmDlg({ title: "Scan the 1-Wire bus?", text: "The STM searches the bus for sensors and matches them to the valves. Readings pause for a few seconds.", ok: "Scan" })) return;
  await busy(btn, async () => {
    await api("POST", "/api/sensors/scan");
    toast("Scan started");
    await sleep(4000);
    kick(pSensors);
  });
});

// ------------------------------------------------------------------ events

const ev = { key: "", since: 0, seen: 0, rows: 0 };
const SEV_CLS = { debug: "", info: "info", warning: "warn", error: "err", critical: "err" };

function updateValveFilterNames() {
  const sel = $("ev-valve");
  if (sel.options.length === 1) {
    for (let i = 1; i <= VALVES; i++) sel.append(h("option", { value: i, text: "#" + i }));
  }
  for (let i = 1; i <= VALVES; i++) {
    const v = V[i - 1];
    setText(sel.options[i], "#" + i + (v && v.name ? " " + v.name : ""));
  }
}
updateValveFilterNames();

function eventRow(e) {
  const when = isNum(e.t) && e.t > 0 ? new Date(e.t * 1000) : null;
  const t = h("time", { text: when ? when.toLocaleString() : "+" + fmt(e.up) + " s after boot" });
  if (when) t.setAttribute("datetime", when.toISOString());
  const sev = typeof e.sev === "string" ? e.sev : "info";
  return h("li", null, t, h("span", { class: "chip " + (SEV_CLS[sev] || ""), text: sev }),
    h("span", { class: "code", text: (e.name || "") + (isInt(e.code) ? " " + e.code : "") }),
    h("span", { class: "msg", text: e.msg || e.text || "" }));
}

async function pollEvents() {
  if (!$("ev-live").checked && ev.key) return;
  const sev = $("ev-sev").value, valve = $("ev-valve").value;
  const key = sev + "|" + valve;
  const list = $("ev-list");
  if (key !== ev.key) { ev.key = key; ev.since = 0; ev.rows = 0; list.textContent = ""; }
  let dropped = 0;
  for (let round = 0; round < 12; round++) {
    const q = "/api/events?limit=50&since=" + ev.since + "&minSeverity=" + encodeURIComponent(sev) +
      (valve ? "&valve=" + encodeURIComponent(valve) : "");
    const d = await api("GET", q);
    if (key !== ev.key) return;  // filter changed meanwhile
    if (!d || !Array.isArray(d.events)) throw new Error("bad events");
    if (isInt(d.last) && d.last < ev.since) {  // ESP rebooted with a fresh log
      ev.since = 0; ev.rows = 0; list.textContent = "";
      continue;
    }
    const evs = d.events.filter((x) => x && isInt(x.seq) && x.seq > ev.since).sort((a, b) => a.seq - b.seq);
    for (const e of evs) { list.insertBefore(eventRow(e), list.firstChild); ev.rows++; }
    if (isInt(d.dropped)) dropped = d.dropped;
    const next = isInt(d.next) ? d.next : evs.length ? evs[evs.length - 1].seq : ev.since;
    const moved = next > ev.since;
    ev.since = Math.max(ev.since, next);
    ev.seen = Math.max(ev.seen, ev.since);
    if (evs.length < 50 || !moved) break;
  }
  while (ev.rows > MAX_EVENT_ROWS && list.lastChild) { list.lastChild.remove(); ev.rows--; }
  const empty = $("ev-empty");
  if (!ev.rows && !empty) list.append(h("li", { id: "ev-empty" }, h("span", { class: "empty", text: "No events match the filter." })));
  else if (ev.rows && empty) empty.remove();
  setText($("ev-info"), (dropped ? dropped + " older events were overwritten on the device. " : "") +
    (ev.rows >= MAX_EVENT_ROWS ? "Showing the newest " + MAX_EVENT_ROWS + " events; download the log for more." : ""));
}
for (const id of ["ev-sev", "ev-valve"]) $(id).addEventListener("change", () => { ev.key = ""; kick(pEvents); });
$("ev-live").addEventListener("change", () => kick(pEvents));

// ------------------------------------------------------------------ settings

// Field definition: [key, label, type, a, b, rule, hint]
//   str: a..b UTF-8 bytes, rule; int: a..b; num: a..b (float); sel: a = options;
//   secret: a..b bytes; ip/mask/bool/id/days: no extra.
const RULE_MSG = { safe: "no + # / \" \\ or control characters",
  print: "no control characters", nospace: "printable ASCII without spaces", nocolon: "no ':' or control characters",
  host: "host name (letters, digits, '-', '.') or IPv4 address",
  hosts: "up to 4 host names or IPv4 addresses, comma-separated", seg: "letters, digits, '_' and '-'",
  path: "levels of letters, digits, '_', '-' separated by single '/'", client: "letters, digits, '_', '-'" };
const GROUPS = [
  ["Station and network", "Network changes restart the ESP and must be confirmed from the new address within 2 minutes.", [
    ["station", "Station name", "str", 1, 20, "safe", "Host name, MQTT root and Home Assistant device name"],
    ["net.iface", "Interface", "sel", [[0, "Automatic"], [1, "Ethernet"], [2, "WiFi"]]],
    ["net.dhcp", "Use DHCP", "bool"],
    ["net.ip", "Static IP address", "ip"],
    ["net.mask", "Subnet mask", "mask"],
    ["net.gateway", "Gateway", "ip"],
    ["net.dns", "DNS server", "ip", undefined, undefined, "", "0.0.0.0 = use the gateway"],
    ["net.ssid", "WiFi SSID", "str", 0, 32, "print"],
    ["net.wifiPassword", "WiFi password", "secret", 0, 63, "", "Empty for an open network"],
    ["net.reconnectTimeoutMin", "Restart after network loss (min)", "int", 0, 240, "", "0 = never"]]],
  ["Time", "", [
    ["time.ntpServer", "NTP server", "str", 0, 64, "host", "Empty disables time sync"],
    ["time.tzName", "Time zone name", "str", 0, 49, "print"],
    ["time.tzPosix", "POSIX time zone", "str", 1, 49, "nospace", "e.g. CET-1CEST,M3.5.0,M10.5.0/3"]]],
  ["Web access", "Login is required when both user and password are set.", [
    ["web.user", "User", "str", 0, 64, "nocolon"],
    ["web.password", "Password", "secret", 0, 64],
    ["web.protectRead", "Also require login to view status", "bool"],
    ["web.allowedHosts", "Additional host names", "str", 0, 80, "hosts",
      "The API answers only for the device IP, its host name and <host name>.local; add other names you use, comma-separated"]]],
  ["MQTT", "", [
    ["mqtt.mode", "Mode", "sel", [[0, "Off"], [1, "MQTT"], [2, "MQTT + Home Assistant discovery"]]],
    ["mqtt.host", "Broker host or IP", "str", 0, 64, "host"],
    ["mqtt.port", "Port", "int", 1, 65535],
    ["mqtt.user", "User", "str", 0, 64, "print"],
    ["mqtt.password", "Password", "secret", 0, 64, "", "Without a password the broker is used anonymously"],
    ["mqtt.rootTopic", "MQTT root topic", "str", 0, 20, "safe", "Empty = station name"],
    ["mqtt.clientId", "Client id", "str", 0, 23, "client", "Empty = automatic (<host>-<mac>); some brokers accept at most 23 characters"],
    ["mqtt.discoveryPrefix", "HA discovery prefix", "str", 0, 32, "path", "Usually homeassistant"],
    ["mqtt.keepAliveS", "Keep-alive (s)", "int", 5, 300],
    ["mqtt.publishIntervalS", "Publish interval (s)", "int", 2, 3600],
    ["mqtt.minDelayS", "Minimum delay between publishes (s)", "int", 0, 3600],
    ["mqtt.separate", "Separate topic per value", "bool"],
    ["mqtt.allTemps", "Publish all temperature sensors", "bool"],
    ["mqtt.pathAsRoot", "Topic path as root", "bool"],
    ["mqtt.upTime", "Publish uptime", "bool"],
    ["mqtt.onChange", "Publish on change", "bool"],
    ["mqtt.retained", "Retained messages", "bool"],
    ["mqtt.plainText", "Plain-text payloads", "bool"],
    ["mqtt.diag", "Legacy diagnostics", "bool"],
    ["mqtt.germanDecimal", "Decimal comma", "bool"],
    ["mqtt.newDiag", "Extended diagnostics (diag/…)", "bool"],
    ["mqtt.events", "Publish warnings and calibration results", "bool"],
    ["mqtt.haDiscoveryOnConnect", "Send HA discovery on connect", "bool"]]],
  ["Failsafe", "Used when the regulator (Home Assistant / MQTT) goes silent. Per-valve positions are in the valves table.", [
    ["failsafe.timeoutMin", "Failsafe after regulator silence (min)", "int", 0, 1440, "",
      "0 = off, 5-1440. When Home Assistant/MQTT is silent this long, every valve goes to its failsafe position"]]],
  ["Calibration schedule", "All valves are calibrated one after another at the chosen local time.", [
    ["calib.dayMask", "Days", "days"],
    ["calib.hour", "Hour (0–23)", "int", 0, 23],
    ["calib.minute", "Minute (0–59)", "int", 0, 59]]],
  ["Logging", "", [
    ["syslog.level", "Syslog", "sel", [[0, "Off"], [1, "Warnings and errors"], [2, "Info and above"], [3, "Everything (debug)"]]],
    ["syslog.server", "Syslog server IP", "ip"],
    ["syslog.port", "Syslog port", "int", 1, 65535],
    ["persistLog", "Keep the event log in flash", "bool"]]],
];
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

let cfg = null, cfgLoaded = false;
let CF = new Map();       // config fields: key -> field
let MF = new Map();       // STM motor fields
let motor = null;
let mapSel = [];          // [valve] -> {s1, s2, orig1, orig2}
let busIds = [], busVoltIds = [];

function getPath(obj, key) {
  let o = obj;
  for (const part of key.split(".")) {
    if (o === null || typeof o !== "object") return undefined;
    o = Array.isArray(o) ? o[Number(part) - 1] : o[part];
  }
  return o;
}

function ipv4(str) {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(str);
  if (!m) return null;
  const b = m.slice(1).map(Number);
  return b.every((x) => x <= 255) ? b : null;
}
function maskOk(b) {
  const n = ((b[0] << 24) | (b[1] << 16) | (b[2] << 8) | b[3]) >>> 0, inv = ~n >>> 0;
  return ((inv & (inv + 1)) >>> 0) === 0;
}
// Lengths are checked in UTF-8 bytes, like the firmware (legacy char buffers).
function utf8Len(v) { return new TextEncoder().encode(v).length; }
function ruleOk(rule, v) {
  if (/[\x00-\x1f\x7f-\x9f]/.test(v)) return false;
  switch (rule) {
    case "safe": return !/[+#/"\\]/.test(v);
    case "nospace": return /^[\x21-\x7e]*$/.test(v);
    case "nocolon": return !v.includes(":");
    case "seg": return /^[A-Za-z0-9_-]*$/.test(v);
    case "client": return /^[A-Za-z0-9_-]*$/.test(v);
    case "path": return v === "" || /^[A-Za-z0-9_-]+(\/[A-Za-z0-9_-]+)*$/.test(v);
    case "hosts": return v.trim() === "" ? v === "" : v.split(",").length <= 4 && v.split(",").every((x) => ruleOk("host", x.trim()) && x.trim() !== "");
    case "host":
      if (v === "") return true;
      if (/^[\d.]+$/.test(v)) return ipv4(v) !== null;
      return /^[A-Za-z0-9.-]+$/.test(v) && !/^[-.]|[-.]$/.test(v);
    default: return true;
  }
}

// Reads a field's input: {v, err}. v is the typed value to send.
function readField(f) {
  const el = f.el;
  switch (f.type) {
    case "bool": return { v: el.checked };
    case "sel": return { v: Number(el.value) };
    case "days": {
      let m = 0;
      f.boxes.forEach((b, i) => { if (b.checked) m |= 1 << i; });
      return { v: m };
    }
    case "int": {
      const raw = el.value.trim(), n = Number(raw);
      if (!/^-?\d{1,6}$/.test(raw)) return { err: "Enter a whole number" + (f.a !== undefined ? " " + f.a + "–" + f.b : "") };
      if (n < f.a || n > f.b) return { err: "Allowed " + f.a + "–" + f.b };
      return { v: n };
    }
    case "pct": {  // 0..100 or hold (255)
      if (f.hold && f.hold.checked) return { v: 255 };
      const raw = el.value.trim(), n = Number(raw);
      if (!/^\d{1,3}$/.test(raw) || n > 100) return { err: "0-100, or Hold" };
      return { v: n };
    }
    case "learn": {
      const raw = el.value.trim(), n = Number(raw);
      if (!/^\d{1,5}$/.test(raw) || (n !== 0 && (n < 50 || n > 65534))) return { err: "0 (off) or 50–65534" };
      return { v: n };
    }
    case "num": {
      const raw = el.value.trim().replace(",", "."), n = Number(raw);
      if (!/^-?\d+(\.\d+)?$/.test(raw) || !Number.isFinite(n)) return { err: "Enter a number" };
      if (n < f.a || n > f.b) return { err: "Allowed " + f.a + " to " + f.b };
      if (f.nonzero && n === 0) return { err: "Must not be 0" };
      return { v: n };
    }
    case "ip": case "mask": {
      const raw = el.value.trim(), b = ipv4(raw);
      if (!b) return { err: "IPv4 address like 192.168.1.10" };
      if (f.type === "mask" && !maskOk(b)) return { err: "Not a valid subnet mask" };
      return { v: b.join(".") };
    }
    case "id": {
      const raw = el.value.trim().toLowerCase();
      if (raw !== "" && !/^[0-9a-f]{2}(-[0-9a-f]{2}){7}$/.test(raw)) return { err: "Format 28-xx-xx-xx-xx-xx-xx-xx" };
      return { v: raw };
    }
    case "secret": {
      const v = el.value;
      if (utf8Len(v) > f.b) return { err: "At most " + f.b + " bytes" };
      if (!ruleOk("print", v)) return { err: RULE_MSG.print };
      return { v };
    }
    default: {  // str
      const v = el.value, n = utf8Len(v);
      if (n < f.a || n > f.b) return { err: (f.a ? f.a + "–" + f.b : "At most " + f.b) + " bytes (UTF-8)" };
      if (!ruleOk(f.rule, v)) return { err: RULE_MSG[f.rule] || "Invalid" };
      return { v };
    }
  }
}

function same(f, a, b) {
  if (f.type === "num") return isNum(a) && isNum(b) && Math.abs(a - b) < 1e-6;
  if (f.type === "id") return String(a || "").toLowerCase() === String(b || "").toLowerCase();
  return a === b;
}

function changed(f) {
  if (f.type === "secret") return f.el.value !== "" || (f.clear && f.clear.checked);
  const r = readField(f);
  return r.err !== undefined || !same(f, r.v, f.orig);
}

function showErr(f, msg) {
  const el = f.type === "days" ? f.boxes[0] : f.el;
  el.setAttribute("aria-invalid", msg ? "true" : "false");
  f.errEl.textContent = msg || "";
  if (f.compact) el.title = msg || "";
}

function makeField(map, def, compact) {
  const [key, label, type, a, b, rule, hint] = def;
  const f = { key, label, type, a, b, rule, compact, orig: undefined };
  const id = "f-" + key.replace(/\./g, "-");
  // In tables the message is announced and shown as a tooltip, not inserted,
  // so rows never move under the pointer.
  f.errEl = h("span", { class: compact ? "sr" : "err-msg", id: id + "-e" });
  let wrap;
  if (type === "bool") {
    f.el = h("input", { type: "checkbox", id, "aria-describedby": id + "-e" });
    wrap = compact ? h("div", null, f.el, f.errEl) : h("div", { class: "field check" }, f.el, h("label", { for: id, text: label }), f.errEl);
    if (compact) f.el.setAttribute("aria-label", label);
  } else if (type === "days") {
    f.boxes = DAYS.map((d, i) => h("input", { type: "checkbox", id: id + "-" + i }));
    f.el = f.boxes[0];
    wrap = h("fieldset", { class: "days field", "aria-describedby": id + "-e" }, h("legend", { text: label }),
      f.boxes.map((bx, i) => h("label", { class: "inline" }, bx, DAYS[i])), f.errEl);
  } else {
    if (type === "sel") {
      f.el = h("select", { id }, a.map(([v, t]) => h("option", { value: v, text: t })));
    } else {
      const attrs = { id, autocomplete: "off", spellcheck: "false", autocapitalize: "off", "aria-describedby": id + "-e" };
      if (type === "int" || type === "learn") { attrs.inputmode = "numeric"; }
      if (type === "num") { attrs.inputmode = "decimal"; }
      if (type === "secret") { attrs.type = "password"; attrs.autocomplete = "new-password"; }
      if (type === "id") { attrs.class = "id"; attrs.placeholder = "28-xx-xx-xx-xx-xx-xx-xx"; attrs.list = key.startsWith("volts") ? "bus-volt-ids" : "bus-ids"; }
      if (type === "str" || type === "secret") attrs.maxlength = b;
      f.el = h("input", attrs);
    }
    if (type === "pct") {
      f.el.inputMode = "numeric";
      f.el.classList.add("pct");
      f.hold = h("input", { type: "checkbox", "aria-label": label + ": hold the position" });
      f.hold.addEventListener("change", () => { f.el.disabled = f.hold.checked; });
    }
    if (compact) {
      f.el.setAttribute("aria-label", label);
      wrap = h("div", f.hold ? { class: "row" } : null, f.el, f.hold ? h("label", { class: "inline small" }, f.hold, "Hold") : null, f.errEl);
    } else {
      const kids = [h("label", { for: id, text: label }), f.el];
      if (type === "secret") {
        f.clear = h("input", { type: "checkbox", id: id + "-clr" });
        f.clearWrap = h("label", { class: "inline small", for: id + "-clr" }, f.clear, "Remove stored password");
        kids.push(f.clearWrap);
      }
      if (hint) kids.push(h("span", { class: "hint", text: hint }));
      kids.push(f.errEl);
      wrap = h("div", { class: "field" }, kids);
    }
  }
  map.set(key, f);
  return wrap;
}

function setFieldValue(f, v) {
  f.orig = v;
  switch (f.type) {
    case "pct":
      f.hold.checked = v === 255;
      f.el.value = isInt(v) && v <= 100 ? String(v) : "";
      f.el.disabled = v === 255;
      break;
    case "bool": f.el.checked = !!v; break;
    case "days": f.boxes.forEach((b, i) => { b.checked = isInt(v) && ((v >> i) & 1) === 1; }); break;
    case "secret":
      f.el.value = "";
      f.el.placeholder = v ? "stored (leave empty to keep)" : "not set";
      f.clear.checked = false;
      f.clearWrap.hidden = !v;
      break;
    default: f.el.value = v === undefined || v === null ? "" : String(v);
  }
  showErr(f, "");
}

function buildSettings() {
  const form = $("cfg-form");
  form.textContent = "";
  CF = new Map();
  form.append(h("datalist", { id: "bus-ids" }), h("datalist", { id: "bus-volt-ids" }));
  GROUPS.forEach(([title, note, defs], gi) => {
    form.append(h("details", { class: "grp", open: gi === 0 },
      h("summary", { text: title }),
      h("div", { class: "body" }, note ? h("p", { class: "hint", text: note }) : null,
        h("div", { class: "grid" }, defs.map((d) => makeField(CF, d, false))))));
  });
  // valves: name, active, sensor mapping
  mapSel = [];
  const vrows = [];
  for (let i = 1; i <= VALVES; i++) {
    const s1 = h("select", { "aria-label": "Valve " + i + " sensor 1" }), s2 = h("select", { "aria-label": "Valve " + i + " sensor 2" });
    mapSel[i] = { s1, s2, orig1: 0, orig2: 0 };
    vrows.push(h("tr", null, h("td", { text: i }),
      h("td", null, makeField(CF, ["valves." + i + ".name", "Valve " + i + " name", "str", 0, 10, "safe"], true)),
      h("td", null, makeField(CF, ["valves." + i + ".active", "Valve " + i + " active", "bool"], true)),
      h("td", null, s1), h("td", null, s2),
      h("td", null, makeField(CF, ["valves." + i + ".failsafePct", "Valve " + i + " failsafe %", "pct"], true)),
      h("td", null, makeField(CF, ["valves." + i + ".topic", "Valve " + i + " MQTT topic", "str", 0, 10, "seg"], true))));
  }
  form.append(tableGroup("Valves", "Names are used in MQTT topics and must be unique. Sensor changes are sent to the STM separately after the configuration is saved. Failsafe %: position when the regulator is silent (Hold keeps the position). MQTT topic: empty = from the name.",
    ["#", "Name", "Active", "Sensor 1", "Sensor 2", "Failsafe %", "MQTT topic"], vrows));
  const trows = [];
  for (let i = 1; i <= TEMP_SLOTS; i++) {
    const p = "temps." + i + ".";
    trows.push(h("tr", null, h("td", { text: i }),
      h("td", null, makeField(CF, [p + "name", "Temperature slot " + i + " name", "str", 0, 10, "safe"], true)),
      h("td", null, makeField(CF, [p + "active", "Temperature slot " + i + " active", "bool"], true)),
      h("td", null, makeField(CF, [p + "offset", "Temperature slot " + i + " offset in °C", "num", -10, 10], true)),
      h("td", null, makeField(CF, [p + "id", "Temperature slot " + i + " 1-Wire id", "id"], true)),
      h("td", null, makeField(CF, [p + "topic", "Temperature slot " + i + " MQTT topic", "str", 0, 10, "seg"], true))));
  }
  form.append(tableGroup("Temperature sensors", "Offset in °C (−10 to 10, 0.1 steps). Ids of sensors found on the bus are suggested. MQTT topic: empty = from the name.",
    ["Slot", "Name", "Active", "Offset", "1-Wire id", "MQTT topic"], trows));
  const wrows = [];
  for (let i = 1; i <= VOLT_SLOTS; i++) {
    const p = "volts." + i + ".";
    const nf = makeField(CF, [p + "factor", "Voltage slot " + i + " factor", "num", -1000, 1000], true);
    CF.get(p + "factor").nonzero = true;
    wrows.push(h("tr", null, h("td", { text: i }),
      h("td", null, makeField(CF, [p + "name", "Voltage slot " + i + " name", "str", 0, 10, "safe"], true)),
      h("td", null, makeField(CF, [p + "active", "Voltage slot " + i + " active", "bool"], true)),
      h("td", null, makeField(CF, [p + "offset", "Voltage slot " + i + " offset", "num", -1000, 1000], true)),
      h("td", null, nf),
      h("td", null, makeField(CF, [p + "unit", "Voltage slot " + i + " unit", "str", 0, 8, "safe"], true)),
      h("td", null, makeField(CF, [p + "id", "Voltage slot " + i + " 1-Wire id", "id"], true)),
      h("td", null, makeField(CF, [p + "topic", "Voltage slot " + i + " MQTT topic", "str", 0, 10, "seg"], true))));
  }
  form.append(tableGroup("Voltage sensors", "value = (raw / 100 + offset) × factor (raw in 10 mV). MQTT topic: empty = from the name.",
    ["Slot", "Name", "Active", "Offset", "Factor", "Unit", "1-Wire id", "MQTT topic"], wrows));
  form.append(h("div", { class: "savebar" },
    h("button", { type: "submit", class: "primary", id: "cfg-save" }, "Save"),
    h("button", { type: "button", id: "cfg-discard", onclick: () => { if (cfg) fillSettings(cfg); } }, "Discard changes"),
    h("span", { id: "cfg-state", class: "small muted", "aria-live": "polite" })));
  for (const f of CF.values()) {
    if (f.key.startsWith("temps.") && f.key.endsWith(".id")) f.el.addEventListener("change", fillSlotOptions);
    if (f.key.startsWith("temps.") && f.key.endsWith(".name")) f.el.addEventListener("change", fillSlotOptions);
  }
  fillDatalists();
}

function tableGroup(title, note, heads, rows) {
  return h("details", { class: "grp" }, h("summary", { text: title }),
    h("div", { class: "body" }, h("p", { class: "hint", text: note }),
      h("div", { class: "tbl-wrap" }, h("table", null,
        h("thead", null, h("tr", null, heads.map((x) => h("th", { text: x })))), h("tbody", null, rows)))));
}

function fillDatalists() {
  const fill = (id, ids) => {
    const dl = $(id);
    if (dl) dl.replaceChildren(...ids.map((x) => h("option", { value: x })));
  };
  fill("bus-ids", busIds);
  fill("bus-volt-ids", busVoltIds);
}

// Sensor-mapping choices: temperature slots that hold an id.
function fillSlotOptions() {
  const opts = [[0, "none"]];
  for (let i = 1; i <= TEMP_SLOTS; i++) {
    const idf = CF.get("temps." + i + ".id"), nf = CF.get("temps." + i + ".name");
    const id = readField(idf).v;
    if (id) opts.push([i, i + ": " + (nf.el.value || id)]);
  }
  for (let i = 1; i <= VALVES; i++) {
    for (const k of ["s1", "s2"]) {
      const sel = mapSel[i][k], cur = sel.value === "" ? mapSel[i][k === "s1" ? "orig1" : "orig2"] : Number(sel.value);
      sel.replaceChildren(...opts.map(([v, t]) => h("option", { value: v, text: t })));
      if (!opts.some((o) => o[0] === cur)) sel.append(h("option", { value: cur, text: cur + ": (no id)" }));
      sel.value = String(cur);
    }
  }
}

// Fields that only matter when another setting enables them.
const DEPENDS = [
  [["net.ip", "net.mask", "net.gateway", "net.dns"], () => !CF.get("net.dhcp").el.checked],
  [["syslog.server", "syslog.port"], () => CF.get("syslog.level").el.value !== "0"],
  [["mqtt.host", "mqtt.port", "mqtt.user", "mqtt.password", "mqtt.keepAliveS", "mqtt.publishIntervalS", "mqtt.minDelayS"],
    () => CF.get("mqtt.mode").el.value !== "0"],
];
function updateDepends() {
  for (const [keys, on] of DEPENDS) {
    const enabled = on();
    for (const k of keys) CF.get(k).el.disabled = !enabled;
  }
}

function fillSettings(c) {
  cfg = c;
  for (const f of CF.values()) {
    const v = f.type === "secret" ? getPath(c, f.key + "Set") === true : getPath(c, f.key);
    setFieldValue(f, v);
  }
  for (let i = 1; i <= VALVES; i++) {
    const sensors = V[i - 1] && Array.isArray(V[i - 1].sensors) ? V[i - 1].sensors : [];
    const at = (n) => sensors.find((x) => x && x.sensor === n && isInt(x.slot));
    mapSel[i].orig1 = at(1) ? at(1).slot : 0;
    mapSel[i].orig2 = at(2) ? at(2).slot : 0;
    mapSel[i].s1.value = ""; mapSel[i].s2.value = "";
  }
  fillSlotOptions();
  updateDepends();
  updateDirty();
}

async function loadSettings() {
  if (!CF.size) buildSettings();
  setText($("cfg-state"), "Loading…");
  try {
    if (!V.length) await pollValves().catch(() => {});
    const c = await api("GET", "/api/config");
    if (!c || typeof c !== "object") throw new Error("Invalid configuration document");
    fillSettings(c);
    cfgLoaded = true;
  } catch (e) {
    setText($("cfg-state"), "Could not load: " + e.message);
  }
  loadMotor();
}
$("cfg-reload").addEventListener("click", async () => {
  if (dirtyCount() && !await confirmDlg({ title: "Discard changes?", text: "Reloading drops the unsaved changes.", ok: "Reload" })) return;
  loadSettings();
});

function mappingChanges() {
  const out = [];
  for (let i = 1; i <= VALVES; i++) {
    const m = mapSel[i];
    if (!m) continue;
    const a = Number(m.s1.value), b = Number(m.s2.value);
    if (a !== m.orig1 || b !== m.orig2) out.push([i, a, b]);
  }
  return out;
}
function dirtyCount() {
  if (!cfg) return 0;
  let n = 0;
  for (const f of CF.values()) if (changed(f)) n++;
  return n + mappingChanges().length;
}
function updateDirty() {
  const n = dirtyCount();
  setText($("cfg-state"), n ? n + " unsaved change" + (n > 1 ? "s" : "") : "No changes");
  $("cfg-save").disabled = !n;
  $("cfg-discard").disabled = !n;
}
$("cfg-form").addEventListener("input", updateDirty);
// A field's message appears when it loses focus. If that happens because the
// pointer is pressing another control, wait for the release: moving content
// between mousedown and mouseup would swallow the click.
let pointerDown = false;
const pendingErr = new Set();
function flushErrs() {
  let all = null;
  for (const f of pendingErr) {
    let msg = readField(f).err;
    // keep a cross-field message from the last save attempt until it is fixed
    if (!msg && f.errEl.textContent && CF.get(f.key) === f) msg = (all = all || validateSettings()).get(f.key);
    showErr(f, msg);
  }
  pendingErr.clear();
}
document.addEventListener("pointerdown", () => { pointerDown = true; }, true);
document.addEventListener("pointerup", () => setTimeout(() => { pointerDown = false; flushErrs(); }, 0), true);
document.addEventListener("pointercancel", () => { pointerDown = false; flushErrs(); }, true);
$("cfg-form").addEventListener("change", (e) => {
  updateDepends();
  updateDirty();
  for (const f of CF.values()) {
    if (e.target === f.el || (f.boxes && f.boxes.includes(e.target))) pendingErr.add(f);
  }
  if (!pointerDown) flushErrs();
});
window.addEventListener("beforeunload", (e) => { if (dirtyCount()) { e.preventDefault(); e.returnValue = ""; } });

const segment = (n) => n.replace(/ /g, "_");
// buildHaId (lib/core common.cpp): [A-Za-z0-9_-] kept, Latin letters folded to ASCII, the rest '_'.
function haId(v) {
  return v.normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/ß/g, "ss").replace(/[æÆ]/g, (x) => x === "æ" ? "ae" : "AE")
    .replace(/[œŒ]/g, (x) => x === "œ" ? "oe" : "OE").replace(/[łŁ]/g, (x) => x === "ł" ? "l" : "L").replace(/[^A-Za-z0-9_-]/g, "_");
}

// Mirrors validateConfig (lib/core config.cpp): per-field ranges plus the
// cross-field rules. Returns Map key -> message.
function validateSettings() {
  const errs = new Map();
  const val = (k) => {
    const f = CF.get(k), r = readField(f);
    if (r.err !== undefined) { if (!errs.has(k)) errs.set(k, r.err); return f.orig; }
    return r.v;
  };
  const secretSet = (k) => {
    const f = CF.get(k);
    return f.el.value !== "" || (!!f.orig && !f.clear.checked);
  };
  for (const f of CF.values()) { const r = readField(f); if (r.err !== undefined) errs.set(f.key, r.err); }
  const need = (k, msg) => { if (!errs.has(k)) errs.set(k, msg); };
  if (!val("net.dhcp")) {
    for (const k of ["net.ip", "net.mask", "net.gateway"]) if (val(k) === "0.0.0.0") need(k, "Required without DHCP");
  }
  const ssid = val("net.ssid");
  const wp = CF.get("net.wifiPassword");
  if (ssid) {
    // Empty (not set) = open network.
    if (wp.el.value !== "" && utf8Len(wp.el.value) < 8) need("net.wifiPassword", "8–63 bytes, or empty for an open network");
  }
  if (val("net.iface") === 2 && !ssid) need("net.ssid", "Required for WiFi");
  if (val("syslog.level") > 0 && val("syslog.server") === "0.0.0.0") need("syslog.server", "Required when syslog is on");
  const user = val("web.user"), pwSet = secretSet("web.password");
  if (user && !pwSet) need("web.password", "Set a password too, or clear the user");
  if (!user && pwSet) need("web.user", "Set a user too, or remove the password");
  const mode = val("mqtt.mode");
  if (mode > 0 && !val("mqtt.host")) need("mqtt.host", "Required when MQTT is on");
  if (val("mqtt.minDelayS") > val("mqtt.publishIntervalS")) need("mqtt.minDelayS", "Must not exceed the publish interval");
  if (mode === 2 && !val("mqtt.separate")) need("mqtt.mode", "Home Assistant needs 'Separate topic per value'");
  const fsMin = val("failsafe.timeoutMin");
  if (fsMin > 0 && fsMin < 5) need("failsafe.timeoutMin", "0 (off) or 5-1440");
  // V3: Home Assistant ids of the items must differ.
  for (const [grp, count] of [["valves", VALVES], ["temps", TEMP_SLOTS], ["volts", VOLT_SLOTS]]) {
    const ids = new Map();
    for (let i = 1; i <= count; i++) {
      if (grp !== "valves" && (!val(grp + "." + i + ".active") || !val(grp + "." + i + ".id"))) continue;
      const topic = val(grp + "." + i + ".topic"), name = val(grp + "." + i + ".name");
      const seg = topic || (name ? segment(name) : String(i));
      const id = haId(seg);
      if (ids.has(id)) need(grp + "." + i + "." + (topic ? "topic" : "name"), "Same Home Assistant id as " + grp.replace(/s$/, "") + " " + ids.get(id));
      else ids.set(id, i);
    }
  }
  const names = [];
  for (let i = 1; i <= VALVES; i++) names.push(val("valves." + i + ".name") || "");
  names.forEach((n, i) => {
    if (!n) return;
    for (let j = 0; j < i; j++) if (segment(n) === segment(names[j])) need("valves." + (i + 1) + ".name", "Same name as valve " + (j + 1));
    if (/^([1-9]|1[0-2])$/.test(n) && !names[Number(n) - 1]) need("valves." + (i + 1) + ".name", "Clashes with the topic of unnamed valve " + n);
  });
  for (const [grp, count] of [["temps", TEMP_SLOTS], ["volts", VOLT_SLOTS]]) {
    const seen = new Map();
    for (let i = 1; i <= count; i++) {
      const id = val(grp + "." + i + ".id");
      if (!id) {
        if (val(grp + "." + i + ".active")) need(grp + "." + i + ".active", "Needs a 1-Wire id");
        continue;
      }
      if (/^0{2}(-0{2}){7}$/.test(id)) { need(grp + "." + i + ".id", "All-zero id"); continue; }
      if (seen.has(id)) need(grp + "." + i + ".id", "Same id as slot " + seen.get(id));
      else seen.set(id, i);
    }
  }
  return errs;
}

function revealField(f) {
  let p = f.el.parentElement;
  while (p) { if (p.tagName === "DETAILS") p.open = true; p = p.parentElement; }
  f.el.focus();
}

$("cfg-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!cfg) return;
  const errs = validateSettings();
  for (const f of CF.values()) showErr(f, errs.get(f.key));
  if (errs.size) {
    const first = [...CF.values()].find((f) => errs.has(f.key));
    revealField(first);
    toast(first.label + ": " + errs.get(first.key) + (errs.size > 1 ? " (and " + (errs.size - 1) + " more)" : ""), true);
    return;
  }
  const patch = {};
  let clear = false;
  for (const f of CF.values()) {
    if (!changed(f)) continue;
    if (f.type === "secret") {
      if (f.el.value === "") clear = true;
      patch[f.key] = f.el.value;
    } else {
      patch[f.key] = readField(f).v;
    }
  }
  if (clear) patch.clearSecrets = true;
  if (utf8Len(JSON.stringify(patch)) > 8000) {
    toast("Too many changes for one request (the device accepts 8 KB); save part of them first", true);
    return;
  }
  const maps = mappingChanges();
  await busy($("cfg-save"), async () => {
    if (Object.keys(patch).length) {
      const rejected = (err) => {
        const path = err.data && typeof err.data.detail === "string" ? err.data.detail : "";
        const f = CF.get(path);
        if (err.status === 400 && f) { showErr(f, "Rejected by the device"); revealField(f); }
        throw err;
      };
      const dry = await api("POST", "/api/config?dryRun=1", patch).catch(rejected);
      if (dry && dry.restartRequired && !await confirmDlg({ title: "Restart the ESP?", text: "These changes are applied by restarting the ESP. The STM and the valves keep running." +
        (dry.netTrial ? " The new network settings run on trial: open the dashboard at the new address and confirm them within 2 minutes, or the previous settings return." : ""),
      ok: "Save and restart" })) return;
      const res = await api("POST", "/api/config", patch).catch(rejected);
      const restart = !!(res && res.restartRequired);
      fillSettings(res && typeof res === "object" && res.station !== undefined ? res : await api("GET", "/api/config"));
      toast("Configuration saved" + (restart ? "; the ESP restarts" : "") + (res && res.netTrial ? ": confirm the network settings from the new address" : ""));
      if (restart) expectRestart("Settings");
    }
    for (const [i, a, b] of maps) {
      await api("POST", "/api/valves/" + i + "/sensors", { slot1: a, slot2: b });
      const m = mapSel[i];
      m.orig1 = a; m.orig2 = b;
      m.s1.value = String(a); m.s2.value = String(b);
      toast("Valve " + i + ": sensors sent to the STM");
    }
    updateDirty();
    kick(pValves);
  });
});

// STM motor parameters (separate endpoint; values live on the STM)
const MOTOR_DEFS = [
  ["motor.lowC", "End-stop factor, low (×0.1 of mean current)", "int", 10, 40],
  ["motor.highC", "End-stop factor, high (×0.1 of mean current)", "int", 10, 40],
  ["motor.startOnPower", "Position after STM start (%)", "int", 0, 100],
  ["motor.noOfMinCount", "Minimum counts of a full stroke", "int", 0, 60000],
  ["motor.maxCalReps", "Calibration retries before blocking", "int", 0, 2],
  ["learnMovements", "Recalibrate after movements", "learn", 0, 65534, "", "0 = off, or 50–65534"],
  ["breakaway.enable", "Breakaway: raise the current limit on each retry", "bool"],
  ["breakaway.stepPct", "Breakaway step per retry (%)", "int", 0, 100],
  ["breakaway.maxmA", "Breakaway current cap (mA)", "int", 20, 60],
];
function buildMotor() {
  const form = $("motor-form");
  MF = new Map();
  form.replaceChildren(h("details", { class: "grp", open: true }, h("summary", { text: "STM motor parameters" }),
    h("div", { class: "body" },
      h("p", { class: "hint", id: "motor-note" }),
      h("div", { class: "grid" }, MOTOR_DEFS.map((d) => makeField(MF, d, false))),
      h("div", { class: "row", style: "margin-top:12px" },
        h("button", { type: "submit", class: "primary", id: "motor-save" }, "Send to STM"),
        h("span", { id: "motor-state", class: "small muted", "aria-live": "polite" })))));
}
async function loadMotor() {
  if (!MF.size) buildMotor();
  try {
    motor = await api("GET", "/api/stm/motor");
  } catch (e) {
    motor = null;
    setText($("motor-state"), "Could not load: " + e.message);
  }
  const known = !!(motor && motor.known && motor.motor);
  const brk = known && motor.breakaway && typeof motor.breakaway === "object";
  setText($("motor-note"), known ? "These values are stored in the STM EEPROM." + (brk ? "" : " Breakaway needs STM firmware 2.x.") :
    "The STM parameters have not been read yet.");
  for (const f of MF.values()) {
    const v = motor ? getPath(motor, f.key) : undefined;
    setFieldValue(f, v);
    f.el.disabled = !known || (f.key.startsWith("breakaway.") && !brk);
  }
  $("motor-save").disabled = !known;
}
$("motor-form").addEventListener("change", (e) => {
  for (const f of MF.values()) if (e.target === f.el) pendingErr.add(f);
  if (!pointerDown) flushErrs();
});
$("motor-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (!motor || !motor.known) return;
  let bad = null;
  for (const f of MF.values()) {
    if (f.el.disabled) continue;
    const r = readField(f);
    showErr(f, r.err);
    if (r.err && !bad) bad = f;
  }
  if (bad) { revealField(bad); return; }
  const part = (prefix) => {
    const fs = [...MF.values()].filter((f) => f.key.startsWith(prefix) && !f.el.disabled);
    if (!fs.some(changed)) return undefined;
    const o = {};
    for (const f of fs) o[f.key.slice(prefix.length)] = readField(f).v;
    return o;
  };
  const body = {};
  const m = part("motor."), b = part("breakaway.");
  if (m) body.motor = m;
  if (b) body.breakaway = b;
  const lf = MF.get("learnMovements");
  if (changed(lf)) body.learnMovements = readField(lf).v;
  if (!Object.keys(body).length) { toast("No changes"); return; }
  await busy($("motor-save"), async () => {
    await api("POST", "/api/stm/motor", body);
    toast("Motor parameters sent to the STM");
    await sleep(2000);
    await loadMotor();
  });
});

// ------------------------------------------------------------------ maintenance

function infoList(dl, rows) {
  dl.replaceChildren(...rows.filter((r) => r).flatMap(([k, v]) => [h("dt", { text: k }), h("dd", { text: v })]));
}
function renderInfo(st) {
  const esp = st.esp || {}, heap = esp.heap || {}, fl = esp.flash || {}, net = st.net || {}, mq = st.mqtt || {};
  infoList($("info-esp"), [
    ["Firmware", esp.version || "?"],
    isNum(esp.build) ? ["Built", fmtEpoch(esp.build)] : null,
    ["Uptime", fmtUptime(esp.uptime) + " (boot " + fmt(esp.boots) + ", " + (esp.resetReason || "?") + ")"],
    ["Heap", fmtBytes(heap.free) + " free, min " + fmtBytes(heap.min) + ", largest " + fmtBytes(heap.largest)],
    ["App image", fmtBytes(fl.used) + " of " + fmtBytes(fl.size)],
    ["Network", (net.state || "?") + (net.ip ? " " + net.ip + " / " + (net.mask || "?") : "")],
    net.gw ? ["Gateway / DNS", net.gw + " / " + (net.dns || "–")] : null,
    ["MAC", net.mac || "–"],
    ["MQTT", (mq.state || "?") + ", " + fmt(mq.reconnects) + " reconnects, " + fmt(mq.publishFailures) + " failed publishes"],
    ["Time", st.time && st.time.valid ? (st.time.local || "").replace("T", " ") + (st.time.lastSync ? ", synced " + fmtEpoch(st.time.lastSync) : "") : "not synced"],
    ["Login", st.auth ? "enabled" : "disabled"],
  ]);
  const stm = st.stm || {}, ls = stm.stats || {}, gs = stm.status, rx = stm.espRx || {};
  infoList($("info-stm"), [
    ["Firmware", (stm.version || "unknown") + (stm.compatible === false ? " (unsupported, minimum " + (stm.minVersion || "?") + ")" : "")],
    ["Protocol", isNum(stm.proto) ? "v" + stm.proto : "unknown"],
    ["Chip", (stm.chip || "unknown") + (stm.hwId ? " (" + stm.hwId + ")" : "")],
    ["Link", (stm.link || "?") + ", " + fmt(ls.sent) + " sent, " + fmt(ls.timeouts) + " timeouts, " + fmt(ls.parseErrors) + " parse errors"],
    ["Resets", fmt(ls.policyResets) + " by link policy, " + fmt(ls.userResets) + " by user"],
    gs && typeof gs === "object" ? ["STM health", "up " + fmtUptime(gs.uptime) + ", " + fmt(gs.resets) + " resets, rx overflow " + fmt(gs.rxOverflow) + ", parse errors " + fmt(gs.parseErr)] : null,
    ["ESP receive", fmt(rx.overflow) + " overflows, " + fmt(rx.malformed) + " malformed lines"],
    st.calibration && isNum(st.calibration.lastScheduled) ? ["Last scheduled calibration", fmtEpoch(st.calibration.lastScheduled)] : null,
  ]);
}

// MD5 (RFC 1321) over a byte array; the ESP checks the image against it.
function md5(bytes) {
  const K = new Int32Array(64);
  for (let i = 0; i < 64; i++) K[i] = Math.floor(Math.abs(Math.sin(i + 1)) * 4294967296);
  const SH = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
  const n = bytes.length, total = (((n + 8) >>> 6) + 1) << 6;
  const buf = new Uint8Array(total);
  buf.set(bytes);
  buf[n] = 0x80;
  const dv = new DataView(buf.buffer);
  dv.setUint32(total - 8, (n * 8) >>> 0, true);
  dv.setUint32(total - 4, Math.floor(n / 0x20000000), true);
  let a0 = 0x67452301, b0 = 0xefcdab89 | 0, c0 = 0x98badcfe | 0, d0 = 0x10325476;
  const M = new Int32Array(16);
  for (let off = 0; off < total; off += 64) {
    for (let j = 0; j < 16; j++) M[j] = dv.getInt32(off + j * 4, true);
    let a = a0, b = b0, c = c0, d = d0;
    for (let i = 0; i < 64; i++) {
      const r = i >> 4;
      let f, g;
      if (r === 0) { f = (b & c) | (~b & d); g = i; }
      else if (r === 1) { f = (d & b) | (~d & c); g = (5 * i + 1) & 15; }
      else if (r === 2) { f = b ^ c ^ d; g = (3 * i + 5) & 15; }
      else { f = c ^ (b | ~d); g = (7 * i) & 15; }
      f = (f + a + K[i] + M[g]) | 0;
      a = d; d = c; c = b;
      const sh = SH[(r << 2) | (i & 3)];
      b = (b + ((f << sh) | (f >>> (32 - sh)))) | 0;
    }
    a0 = (a0 + a) | 0; b0 = (b0 + b) | 0; c0 = (c0 + c) | 0; d0 = (d0 + d) | 0;
  }
  let hex = "";
  for (const w of [a0, b0, c0, d0]) for (let k = 0; k < 4; k++) hex += ((w >>> (8 * k)) & 255).toString(16).padStart(2, "0");
  return hex;
}

function readFile(file) {
  if (file.arrayBuffer) return file.arrayBuffer().then((b) => new Uint8Array(b));
  return new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(new Uint8Array(r.result));
    r.onerror = () => rej(new Error("Could not read the file"));
    r.readAsArrayBuffer(file);
  });
}
function findAscii(bytes, text) {
  const t = Array.from(text, (ch) => ch.charCodeAt(0));
  outer: for (let i = 0; i + t.length <= bytes.length; i++) {
    if (bytes[i] !== t[0]) continue;
    for (let k = 1; k < t.length; k++) if (bytes[i + k] !== t[k]) continue outer;
    return true;
  }
  return false;
}
const uploading = () => ota.busy || stmUp.busy;

// ESP OTA
const ESP_APP_MAX = 0x140000;  // app0/app1 partition size of the (unchangeable) legacy table
const ota = { file: null, md5: "", busy: false };
$("ota-file").addEventListener("change", async () => {
  const f = $("ota-file").files[0], info = $("ota-info");
  ota.file = null;
  $("ota-go").disabled = true;
  info.className = "small";
  if (!f) { info.textContent = ""; return; }
  const limit = S && S.esp && S.esp.flash && isNum(S.esp.flash.size) && S.esp.flash.size > 0 ? S.esp.flash.size : ESP_APP_MAX;
  const bad = (m) => { info.textContent = m; info.className = "small err-msg"; };
  if (f.size < 1024) return bad("The file is too small for ESP32 firmware.");
  if (f.size > limit) return bad("The file (" + fmtBytes(f.size) + ") does not fit the app partition (" + fmtBytes(limit) + ").");
  info.textContent = "Checking…";
  let bytes;
  try { bytes = await readFile(f); } catch (e) { return bad(e.message); }
  if (bytes[0] !== 0xE9) return bad("Not an ESP32 application image (magic byte 0xE9 missing). STM firmware goes into the STM panel.");
  if (bytes.length >= 14 && (bytes[12] | (bytes[13] << 8)) !== 0) return bad("The image is built for a different ESP chip (chip id " + (bytes[12] | (bytes[13] << 8)) + ").");
  ota.md5 = md5(bytes);
  ota.file = f;
  info.textContent = f.name + " · " + fmtBytes(f.size) + " · MD5 " + ota.md5 + (findAscii(bytes, "-revamped") ? "" : " · note: no '-revamped' version string found");
  $("ota-go").disabled = false;
});
$("ota-go").addEventListener("click", async (e) => {
  const btn = e.currentTarget;
  if (!ota.file || uploading()) return;
  if (!await confirmDlg({ title: "Update the ESP firmware?", text: "Upload " + ota.file.name + " (" + fmtBytes(ota.file.size) + ") and restart the ESP. The valves keep their positions.", ok: "Upload and restart" })) return;
  const prog = $("ota-prog");
  ota.busy = true;
  prog.hidden = false;
  prog.value = 0;
  await busy(btn, async () => {
    try {
      await upload("/api/ota/esp?md5=" + ota.md5, "firmware", ota.file, ota.file.name, (x) => { prog.value = Math.round(x * 100); },
        { "X-Update-MD5": ota.md5 });
      $("ota-info").textContent = "Upload complete. The ESP restarts…";
      expectRestart("Firmware update");
    } finally {
      ota.busy = false;
      prog.hidden = true;
    }
  });
});

// STM images and flashing
const STM_IMAGE_MAX = 512 * 1024;
const stmUp = { file: null, name: "", busy: false, handshake: true };
function imageName(n) {
  let name = n.replace(/[^A-Za-z0-9._-]/g, "_").replace(/^\.+/, "");
  if (!/\.bin$/i.test(name)) name += ".bin";
  if (name.length > 31) name = name.slice(0, 27) + ".bin";
  if (name.length <= 4 || /^last_good\.bin$/i.test(name)) name = "stm.bin";
  return name;
}
$("stm-file").addEventListener("change", async () => {
  const f = $("stm-file").files[0], info = $("stm-file-info");
  stmUp.file = null;
  $("stm-up-go").disabled = true;
  info.className = "small";
  if (!f) { info.textContent = ""; return; }
  const bad = (m) => { info.textContent = m; info.className = "small err-msg"; };
  if (f.size < 64) return bad("The file is too small for STM32 firmware.");
  if (f.size > STM_IMAGE_MAX) return bad("The file (" + fmtBytes(f.size) + ") is larger than 512 KiB.");
  let bytes;
  try { bytes = await readFile(f); } catch (e) { return bad(e.message); }
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const sp = dv.getUint32(0, true), rv = dv.getUint32(4, true);
  if (sp < 0x20000000 || sp > 0x20020000 || (sp & 3) !== 0) return bad("Not an STM32 image: the initial stack pointer 0x" + sp.toString(16) + " is not in RAM.");
  if ((rv & 1) !== 1 || rv < 0x08000000 || rv >= 0x08000000 + f.size) return bad("Not an STM32 image: the reset vector 0x" + rv.toString(16) + " is outside the image.");
  stmUp.handshake = findAscii(bytes, "DEADBEEF") && findAscii(bytes, "BEEFIT");
  stmUp.file = f;
  stmUp.name = imageName(f.name);
  info.textContent = stmUp.name + " · " + fmtBytes(f.size) + (stmUp.handshake ? "" :
    " · Warning: no update handshake in this image; the next update will need the BOOT0 jumper.");
  if (!stmUp.handshake) info.className = "small err-msg";
  $("stm-up-go").disabled = false;
});
$("stm-up-go").addEventListener("click", async (e) => {
  if (!stmUp.file || uploading()) return;
  const prog = $("stm-up-prog");
  stmUp.busy = true;
  prog.hidden = false;
  prog.value = 0;
  await busy(e.currentTarget, async () => {
    try {
      await upload("/api/stm/images", "image", stmUp.file, stmUp.name, (x) => { prog.value = Math.round(x * 100); });
      toast("Image " + stmUp.name + " stored");
      $("stm-file").value = "";
      $("stm-file-info").textContent = "";
      stmUp.file = null;
      await loadImages();
    } finally {
      stmUp.busy = false;
      prog.hidden = true;
    }
  });
  $("stm-up-go").disabled = !stmUp.file;
});

let flashActive = false;
const CHECK_TXT = { none: "ok", image_empty: "empty file", image_too_large: "too large", image_bad_vectors: "not an STM32 image",
  image_no_handshake: "no update handshake", image_chip_mismatch: "wrong chip", image_read: "read error" };
let imagesRetry = null;
async function loadImages() {
  const tb = $("stm-images");
  let list;
  try {
    list = await api("GET", "/api/stm/images");
  } catch (e) {
    tb.replaceChildren(h("tr", null, h("td", { colspan: 5, class: "muted", text: "Could not list images: " + e.message })));
    return;
  }
  const imgs = Array.isArray(list) ? list.filter((x) => x && typeof x.name === "string") : [];
  // The device checks a fresh upload in the background ("check" null meanwhile).
  const pending = imgs.some((x) => x.check === null || x.check === undefined);
  clearTimeout(imagesRetry);
  imagesRetry = pending ? setTimeout(loadImages, 2000) : null;
  tb.replaceChildren(...imgs.map((x) => {
    const check = typeof x.check === "string" ? x.check : null;
    const flashable = check === "none" || check === "image_no_handshake";
    const lastGood = x.name === "last_good";
    const state = check === null ? h("span", { class: "chip", text: "checking…" }) :
      h("span", { class: "chip " + (check === "none" ? "ok" : flashable ? "warn" : "err"), text: CHECK_TXT[check] || check.replace(/_/g, " ") });
    return h("tr", null,
      h("td", null, h("span", { class: "mono", text: x.name }), lastGood ? h("span", { class: "small muted", text: " (backup of the last good firmware)" }) : null),
      h("td", { class: "r", text: fmtBytes(x.size) }),
      h("td", null, x.version || "–", " ", state, x.hw ? h("span", { class: "chip", text: x.hw }) : null),
      h("td", { class: "mono", text: isNum(x.crc32) ? "0x" + x.crc32.toString(16).padStart(8, "0") : x.crc32 || "–" }),
      h("td", null, h("span", { class: "row" },
        h("button", { type: "button", class: "primary", disabled: flashActive || !flashable, "aria-label": "Flash " + x.name,
          title: flashable ? "" : check === null ? "The image is still being checked" : "The image cannot be flashed",
          onclick: (e) => flashImage(e.currentTarget, x) }, "Flash"),
        h("button", { type: "button", class: "danger", disabled: flashActive, "aria-label": "Delete " + x.name, onclick: (e) => deleteImage(e.currentTarget, x.name) }, "Delete"))));
  }));
  if (!imgs.length) tb.append(h("tr", null, h("td", { colspan: 5, class: "muted", text: "No images stored." })));
}
async function deleteImage(btn, name) {
  if (!await confirmDlg({ title: "Delete " + name + "?", text: "The image file is removed from the ESP.", ok: "Delete", danger: true })) return;
  await busy(btn, async () => { await api("DELETE", "/api/stm/images/" + encodeURIComponent(name)); await loadImages(); });
}
async function flashImage(btn, img) {
  const noHandshake = img.check === "image_no_handshake";
  const mode = h("select", { id: "fl-mode" }, h("option", { value: "normal", text: "Normal (running STM firmware)" }),
    h("option", { value: "blank", text: "Blank chip / BOOT0 jumper set" }));
  const force = h("input", { type: "checkbox", id: "fl-force" });
  const board = h("select", { id: "fl-board" }, h("option", { value: "", text: "choose" }), h("option", { value: "C1", text: "C1" }),
    h("option", { value: "C2", text: "C2 (also C3/C4)" }));
  const boardField = h("div", { class: "field", hidden: true }, h("label", { for: "fl-board", text: "Board revision" + (img.hw ? " (image: " + img.hw + ")" : "") }), board);
  mode.addEventListener("change", () => { boardField.hidden = mode.value !== "blank"; });
  const extra = [h("div", { class: "field" }, h("label", { for: "fl-mode", text: "Mode" }), mode), boardField];
  if (noHandshake) {
    extra.push(h("p", { class: "small err-msg", text: "This image has no update handshake: after flashing it, the next STM update needs the BOOT0 jumper." }),
      h("label", { class: "inline small" }, force, "Flash it anyway"));
  }
  if (!await confirmDlg({ title: "Flash " + img.name + "?", text: "The STM is reset into its bootloader and reprogrammed" +
    (img.version ? " with " + img.version : "") + ". Valves do not move while flashing (about a minute). Do not power off the device.",
  ok: "Flash", danger: true, extra })) return;
  if (noHandshake && !force.checked) { toast("Not flashed: tick 'Flash it anyway' to use an image without handshake", true); return; }
  await busy(btn, async () => {
    const body = { image: img.name, mode: mode.value === "blank" ? "blank" : "normal", force: noHandshake && force.checked };
    if (mode.value === "blank" && board.value) body.board = board.value;
    await api("POST", "/api/stm/flash", body);
    toast("Flashing started");
    flashActive = true;
    loadImages();
    kick(pFlash);
  });
}
const PHASE_TXT = { idle: "idle", validating: "checking the image", resetting: "resetting the STM", handshake: "entering the bootloader",
  sync: "connecting to the bootloader", getid: "reading the chip id", erasing: "erasing", writing: "writing", verifying: "verifying",
  starting: "starting the new firmware", waiting_app: "waiting for the new firmware", done: "done", failed: "failed" };
let lastPhase = "";
async function pollFlash() {
  const d = await api("GET", "/api/stm/flash");
  if (!d || typeof d.phase !== "string") throw new Error("bad flash status");
  const box = $("flash-status");
  const active = !["idle", "done", "failed"].includes(d.phase);
  if (active !== flashActive) { flashActive = active; loadImages(); }
  if (lastPhase && lastPhase !== d.phase) {
    if (d.phase === "done") toast("STM flashed" + (d.appVersion ? ": " + d.appVersion : ""));
    if (d.phase === "failed") toast("STM flashing failed" + (d.error && d.error.code ? ": " + d.error.code.replace(/_/g, " ") : ""), true);
  }
  lastPhase = d.phase;
  if (d.phase === "idle") { box.replaceChildren(); return; }
  const pct = isNum(d.percent) ? Math.max(0, Math.min(100, d.percent)) : 0;
  const rows = [
    h("p", { class: "small" }, h("b", { text: "Flash: " + (PHASE_TXT[d.phase] || d.phase) }),
      d.image && d.image.name ? " · " + d.image.name : "", d.chipName ? " · " + d.chipName : "",
      isNum(d.attempt) && d.attempt > 1 ? " · attempt " + d.attempt : "", isNum(d.baud) && d.baud ? " · " + d.baud + " Bd" : ""),
  ];
  if (active) rows.push(h("progress", { max: 100, value: pct, "aria-label": "Flash progress" }),
    h("p", { class: "small muted", text: pct + " %" + (isNum(d.bytesTotal) && d.bytesTotal ? " · " + fmtBytes(d.bytesDone) + " of " + fmtBytes(d.bytesTotal) : "") }),
    h("button", { type: "button", class: "danger", onclick: (e) => busy(e.currentTarget, async () => {
      if (!await confirmDlg({ title: "Abort flashing?", text: "The STM may be left without firmware; flash again (blank mode if needed) before using the valves.", ok: "Abort", danger: true })) return;
      await api("POST", "/api/stm/flash/abort");
    }) }, "Abort"));
  if (d.phase === "failed" && d.error) rows.push(h("p", { class: "small err-msg", text: "Error: " + String(d.error.code || "?").replace(/_/g, " ") +
    (d.error.phase ? " while " + (PHASE_TXT[d.error.phase] || d.error.phase) : "") + (d.error.addr ? " at " + d.error.addr : "") }));
  if (d.phase === "done") rows.push(h("p", { class: "small", text: d.manualReset ?
    "Flashed and verified. Remove the BOOT0 jumper, then reset the STM (Reset STM)." :
    "The STM runs " + (d.appVersion || "the new firmware") + "." }));
  if (d.pending) rows.push(h("p", { class: "small muted", text: "Waiting for the STM to finish writing its EEPROM…" }));
  box.replaceChildren(...rows);
}

async function simpleAction(btn, opt, method, path, body, okMsg, after) {
  if (opt && !await confirmDlg(opt)) return;
  await busy(btn, async () => { const d = await api(method, path, body); toast(okMsg); if (after) after(d); });
}
$("btn-reboot").addEventListener("click", (e) => simpleAction(e.currentTarget,
  { title: "Restart the ESP?", text: "The dashboard and MQTT are unavailable for about 20 seconds. The STM and the valves keep running.", ok: "Restart" },
  "POST", "/api/system/reboot", undefined, "Restart requested", () => expectRestart("Restart")));
$("btn-stm-reset").addEventListener("click", (e) => simpleAction(e.currentTarget,
  { title: "Reset the STM?", text: "The STM restarts and every valve recalibrates (several minutes, valves move to their end positions).", ok: "Reset STM", danger: true },
  "POST", "/api/stm/reset", { confirm: true }, "STM reset requested", () => kick(pStatus)));
$("btn-cal-all").addEventListener("click", (e) => simpleAction(e.currentTarget,
  { title: "Calibrate all valves?", text: "The STM calibrates the active valves one after another; this takes a while.", ok: "Calibrate all" },
  "POST", "/api/valves/calibrate", undefined, "Calibration of all valves requested", () => kick(pValves)));
$("btn-asm-all").addEventListener("click", (e) => simpleAction(e.currentTarget,
  { title: "Open all valves fully?", text: "Every valve moves to the assembly position (fully open) so heads can be mounted. Set targets afterwards.", ok: "Open all", danger: true },
  "POST", "/api/valves/assembly", undefined, "Assembly position requested", () => kick(pValves)));
$("btn-detect").addEventListener("click", (e) => simpleAction(e.currentTarget,
  { title: "Detect valves?", text: "The STM probes every output to see which valves are connected.", ok: "Detect" },
  "POST", "/api/valves/detect", undefined, "Valve detection started", () => kick(pValves)));
$("btn-mqtt-rc").addEventListener("click", (e) => simpleAction(e.currentTarget, null, "POST", "/api/mqtt/reconnect", undefined, "MQTT reconnect requested"));
for (const b of document.querySelectorAll("[data-disc]")) {
  const action = b.getAttribute("data-disc");
  b.addEventListener("click", (e) => simpleAction(e.currentTarget, action === "delete" ?
    { title: "Delete Home Assistant discovery?", text: "Home Assistant removes the entities of this station until discovery is sent again.", ok: "Delete", danger: true } : null,
  "POST", "/api/mqtt/discovery", { action }, "Discovery " + (action === "delete" ? "deletion" : "update") + " requested"));
}
$("btn-factory").addEventListener("click", (e) => simpleAction(e.currentTarget,
  { title: "Factory reset?", text: "All settings of this firmware are erased and the ESP restarts with defaults (DHCP on Ethernet). The STM keeps its parameters.", ok: "Erase and restart", danger: true, typed: "factory-reset" },
  "POST", "/api/system/factory-reset", { confirm: "factory-reset" }, "Factory reset requested", () => expectRestart("Factory reset")));
// Flattens a config document into dotted key paths (arrays 1-based, like
// setConfigValue). Secret flags ("<key>Set") and "schema" are not settable.
function flatten(obj, prefix, out, depth) {
  if (depth > 4) throw new Error("Nested too deeply");
  const keys = Array.isArray(obj) ? obj.map((_, i) => i) : Object.keys(obj);
  for (const k of keys) {
    const v = obj[k], path = prefix + (Array.isArray(obj) ? k + 1 : k);
    if (v !== null && typeof v === "object") flatten(v, path + ".", out, depth + 1);
    else if (!/Set$/.test(String(k)) && path !== "schema") out.set(path, v);
  }
  return out;
}
// Import sends only what differs from the current configuration, as one
// request of at most 8000 bytes; keys this firmware does not know are skipped
// and listed. Passwords the file does not carry can be typed in.
const SECRETS = ["web.password", "mqtt.password", "net.wifiPassword"];
$("cfg-import").addEventListener("change", async () => {
  const inp = $("cfg-import"), f = inp.files[0];
  inp.value = "";
  if (!f) return;
  if (f.size > 65536) { toast("The file is too large for a configuration export", true); return; }
  let doc;
  try { doc = JSON.parse(new TextDecoder().decode(await readFile(f))); } catch { toast("Not a JSON file", true); return; }
  if (!doc || typeof doc !== "object" || Array.isArray(doc) || !isInt(doc.schema)) { toast("Not a VdMot Revamped configuration export", true); return; }
  await busy(null, async () => {
    const cur = await api("GET", "/api/config");
    if (!cur || doc.schema < 1) throw new Error("The export has configuration schema " + doc.schema);
    const now = flatten(cur, "", new Map(), 0), patch = {}, skipped = [];
    const file = flatten(doc, "", new Map(), 0);
    for (const [k, v] of file) {
      if (SECRETS.includes(k)) { if (v !== "") patch[k] = v; continue; }
      if (!now.has(k)) { skipped.push(k); continue; }
      if (now.get(k) === v) continue;
      patch[k] = v;
    }
    // secrets flagged in the file but missing in it and on the device
    const pw = [];
    for (const k of SECRETS) {
      if (getPath(doc, k + "Set") === true && !(k in patch) && getPath(cur, k + "Set") !== true) {
        const el = h("input", { type: "password", autocomplete: "new-password", id: "imp-" + k.replace(/\./g, "-") });
        pw.push({ k, el, field: h("div", { class: "field" }, h("label", { for: el.id, text: k }), el,
          h("span", { class: "hint", text: k === "web.password" ? "Required for the imported web login" :
            k === "mqtt.password" ? "Without it the broker login fails" : "Without it the WiFi login fails" })) });
      }
    }
    const webPw = pw.find((x) => x.k === "web.password");
    const skipWeb = h("input", { type: "checkbox", id: "imp-skipweb" });
    const n = Object.keys(patch).length;
    if (!n && !pw.length) { toast("The file matches the current configuration"); return; }
    const extra = [];
    if (doc.schema > cur.schema) extra.push(h("p", { class: "small err-msg", text: "Written by a newer firmware; unknown settings are skipped." }));
    if (skipped.length) extra.push(h("p", { class: "small muted", text: "Skipped (unknown here): " + skipped.slice(0, 12).join(", ") + (skipped.length > 12 ? " …" : "") }));
    for (const x of pw) extra.push(x.field);
    if (webPw) extra.push(h("label", { class: "inline small" }, skipWeb, "Skip the web login settings"));
    const dlgOk = () => { const ok = $("dc-ok"); ok.disabled = !!webPw && !webPw.el.value && !skipWeb.checked; };
    if (webPw) { webPw.el.addEventListener("input", dlgOk); skipWeb.addEventListener("change", dlgOk); }
    let dry = null;
    try { dry = await api("POST", "/api/config?dryRun=1", patch); } catch (e) { if (e.status !== 400 || !webPw) throw e; }
    if (webPw) setTimeout(dlgOk, 0);  // after confirmDlg reset the button
    if (!await confirmDlg({ title: "Import configuration?", text: n + " setting" + (n === 1 ? "" : "s") + " from " + f.name +
      " differ from the current ones and will be replaced. Stored passwords are kept." +
      (dry && dry.restartRequired ? " The ESP restarts to apply them." : "") +
      (dry && dry.netTrial ? " The new network settings must be confirmed from the new address within 2 minutes." : ""), ok: "Import", extra })) return;
    for (const x of pw) if (x.el.value) patch[x.k] = x.el.value;
    if (webPw && skipWeb.checked) { delete patch["web.user"]; delete patch["web.password"]; }
    if (utf8Len(JSON.stringify(patch)) > 8000) throw new Error("Too many differences for one request; edit them under Settings");
    const res = await api("POST", "/api/config", patch);
    const restart = !!(res && res.restartRequired);
    toast("Configuration imported (" + Object.keys(patch).length + " settings)" + (restart ? "; the ESP restarts" : ""));
    cfgLoaded = false;
    if (view === "settings") loadSettings();
    if (restart) expectRestart("Configuration import");
  });
});

// Export with passwords (needs the web login)
$("cfg-export-secrets").addEventListener("click", (e) => simpleAction(e.currentTarget,
  { title: "Export with passwords?", text: "The file contains the web, MQTT and WiFi passwords in clear text. Keep it safe.", ok: "Export" },
  "GET", "/api/config/export?secrets=1", undefined, "Export downloaded", null, (d) => {
    const a = h("a", { href: URL.createObjectURL(new Blob([JSON.stringify(d, null, 1)], { type: "application/json" })), download: "vdmot-config-secrets.json" });
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  }));

// Files on the ESP (LittleFS)
const KIND_TXT = { stm_image: "STM image", upload_part: "aborted upload", log: "event log", internal: "internal",
  legacy_ha_list: "legacy HA list", legacy_image: "legacy STM image", other: "other" };
async function loadFiles() {
  const tb = $("files-body");
  let d;
  try { d = await api("GET", "/api/files"); } catch (e) {
    tb.replaceChildren(h("tr", null, h("td", { colspan: 4, class: "muted", text: "Could not list files: " + e.message })));
    return;
  }
  if (!d || !Array.isArray(d.files)) return;
  fsFree = isNum(d.total) && isNum(d.used) ? Math.max(0, d.total - d.used) : null;
  setText($("files-info"), fmtBytes(d.used) + " of " + fmtBytes(d.total) + " used" + (d.truncated ? " · list truncated" : ""));
  tb.replaceChildren(...d.files.map((x) => h("tr", null,
    h("td", { class: "mono", text: x.path }), h("td", { class: "r", text: fmtBytes(x.size) }),
    h("td", { text: KIND_TXT[x.kind] || x.kind }),
    h("td", null, x.deletable ? h("button", { type: "button", class: "danger", "aria-label": "Delete " + x.path,
      onclick: (e) => simpleAction(e.currentTarget, { title: "Delete " + x.path + "?", text: "The file is removed from the ESP.", ok: "Delete", danger: true },
        "DELETE", "/api/files?path=" + encodeURIComponent(x.path), undefined, x.path + " deleted", loadFiles) }, "Delete") : null))));
  if (!d.files.length) tb.append(h("tr", null, h("td", { colspan: 4, class: "muted", text: "No files." })));
}
$("files-reload").addEventListener("click", (e) => busy(e.currentTarget, loadFiles));

// ------------------------------------------------------------------ start

const pStatus = poller(pollStatus, 5000);
const pValves = poller(pollValves, 3000, () => view === "valves");
const pSensors = poller(pollSensors, 10000, visible("sensors"));
const pEvents = poller(pollEvents, 3000, visible("events"));
const pFlash = poller(pollFlash, 1000, () => flashActive || view === "maintenance");
poller(loadImages, 60000, visible("maintenance"));
poller(loadFiles, 60000, visible("maintenance"));
showView();
})();
