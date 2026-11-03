// Settings page: timezone, notification toggle, a read-only security summary and the audit log viewer.
// Loaded on first visit. DOM is built with textContent only.
import { $, el, api } from "./util.js";

const JSON_HEADERS = { "Content-Type": "application/json" };
const AUDIT_LIMIT = 100;

function say(id, text, kind) {
  const n = $(id);
  n.textContent = text;
  n.className = "set-msg" + (kind ? " " + kind : "");
}

async function put(body) {
  return api("/api/settings", { method: "PUT", headers: JSON_HEADERS, body: JSON.stringify(body) });
}

function timezones(current) {
  let zones = [];
  try {
    zones = Intl.supportedValuesOf("timeZone");
  } catch {
    /* older engine: just offer the current one */
  }
  if (!zones.includes(current)) zones = [current, ...zones];
  if (!zones.includes("UTC")) zones = ["UTC", ...zones];
  return zones;
}

function renderGeneral(s) {
  const sel = $("#setTz");
  sel.replaceChildren(...timezones(s.timezone).map((z) => Object.assign(el("option"), { value: z, textContent: z })));
  sel.value = s.timezone;
  $("#setNotify").checked = !!s["notifications.desktop"];
}

/** Read-only. Every line is true of this build; none of them is a control. */
function renderSecurity() {
  const tokenActive = !!document.querySelector('meta[name="zb-token"]')?.getAttribute("content");
  const rows = [
    ["API token", tokenActive ? "active: every request to this server must carry it" : "missing: reload the page"],
    ["Credential vault", "not in use: no broker is connected, so no credential is stored"],
    ["Live trading", "disarmed"],
    ["Real orders", "this build cannot place any: the live account refuses orders"],
    ["Network", "this server answers on this computer only (127.0.0.1 and ::1)"],
  ];
  const box = $("#setSecurity");
  box.replaceChildren();
  for (const [k, v] of rows) {
    const r = el("div", "sec-row");
    r.append(el("b", null, k), el("span", k === "Live trading" ? "pill-off" : null, v));
    box.append(r);
  }
}

function auditRow(e) {
  const tr = el("tr");
  const when = new Date(e.ts_utc);
  tr.append(el("td", "num", Number.isNaN(when.getTime()) ? e.ts_utc : when.toLocaleString()), el("td", null, e.actor), el("td", null, e.action), el("td", null, e.entity));
  const full = JSON.stringify(e.detail);
  const td = el("td", "notes", full.length > 160 ? full.slice(0, 157) + "…" : full);
  td.title = full;
  tr.append(td);
  return tr;
}

export async function loadAudit() {
  const body = $("#auditBody");
  try {
    const d = await api(`/api/audit?limit=${AUDIT_LIMIT}`);
    body.replaceChildren(...d.entries.map(auditRow));
    $("#auditNote").textContent = `Latest ${d.entries.length}, newest first.${d.next_before ? " Older entries exist." : ""}`;
  } catch (err) {
    body.replaceChildren();
    $("#auditNote").textContent = `Can't load the audit log: ${err.message}`;
  }
}

export async function load() {
  renderSecurity();
  try {
    renderGeneral(await api("/api/settings"));
  } catch (err) {
    say("#setMsg", `Can't load settings: ${err.message}`, "err");
  }
  await loadAudit();
}

export function wire() {
  $("#setTz").addEventListener("change", async (e) => {
    try {
      await put({ timezone: e.target.value });
      say("#setMsg", "Saved.", "ok");
      loadAudit();
    } catch (err) {
      say("#setMsg", err.message, "err");
    }
  });
  $("#setNotify").addEventListener("change", async (e) => {
    const box = e.target;
    try {
      await put({ "notifications.desktop": box.checked });
      say("#setMsg", "Saved.", "ok");
      loadAudit();
    } catch (err) {
      box.checked = !box.checked;
      say("#setMsg", err.message, "err");
    }
  });
  $("#auditRefresh").addEventListener("click", loadAudit);
}
