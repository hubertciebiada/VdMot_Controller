// VdMot Revamped dashboard. Vanilla JS, no build step; polls the /api/*
// endpoints (DESIGN.md "HTTP API"). Kept small: it is embedded gzip-compressed.
"use strict";

const $ = (id) => document.getElementById(id);
let eventSeq = 0;

async function getJson(path) {
  const r = await fetch(path, { cache: "no-store" });
  if (!r.ok) throw new Error(path + " " + r.status);
  return r.json();
}

function chip(el, text, cls) {
  el.textContent = text;
  el.className = "chip " + cls;
}

function uptime(s) {
  const d = Math.floor(s / 86400), h = Math.floor(s / 3600) % 24, m = Math.floor(s / 60) % 60;
  return d + "d " + h + ":" + String(m).padStart(2, "0");
}

async function refreshStatus() {
  const s = await getJson("/api/status");
  $("station").textContent = s.net.hostname || "VdMot";
  $("uptime").textContent = uptime(s.esp.uptime);
  $("time").textContent = s.time.valid ? s.time.local : "time not synced";
  $("esp-version").textContent = s.esp.version;
  $("stm-version").textContent = s.stm.version || "?";
  chip($("net"), s.net.state, s.net.state === "down" ? "err" : "ok");
  chip($("link"), "stm " + s.stm.link, s.stm.link === "up" ? "ok" : (s.stm.link === "down" ? "err" : "warn"));
  chip($("mqtt"), "mqtt " + s.mqtt.state, s.mqtt.state === "connected" ? "ok" : (s.mqtt.state === "disabled" ? "" : "warn"));
}

async function refreshValves() {
  const v = await getJson("/api/valves");
  const list = $("valve-list");
  list.textContent = "";
  for (const x of v.valves) {
    if (!x.active && !x.known) continue;
    const c = document.createElement("div");
    c.className = "card";
    c.textContent = x.idx + " " + (x.name || "") + " - " + x.stateKey + " " + x.pos + "% -> " +
      (x.target === null ? "?" : x.target + "%");
    list.appendChild(c);
  }
}

async function refreshEvents() {
  const e = await getJson("/api/events?since=" + eventSeq + "&limit=50");
  const list = $("event-list");
  for (const ev of e.events) {
    const li = document.createElement("li");
    li.textContent = ev.seq + " " + ev.sev + " " + ev.msg;
    list.insertBefore(li, list.firstChild);
    while (list.childNodes.length > 200) list.removeChild(list.lastChild);
  }
  eventSeq = e.next;
}

async function tick() {
  for (const f of [refreshStatus, refreshValves, refreshEvents]) {
    try { await f(); } catch (err) { console.warn(err); }
  }
}

tick();
setInterval(tick, 2000);
