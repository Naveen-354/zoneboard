// Generic, safe rendering of API results (objects, arrays of rows) and job polling, shared by the
// Operations and Research Lab pages. textContent only, never innerHTML.
import { el, api } from "./util.js";
import { table, row, td, badge } from "./bots/ui.js";

const label = (k) => k.replace(/_/g, " ");
const scalar = (v) => v === null || v === undefined || typeof v !== "object";

function text(v) {
  if (v === null || v === undefined) return "–";
  if (typeof v === "number") return Number.isInteger(v) ? String(v) : String(Number(v.toPrecision(8)));
  if (typeof v === "boolean") return v ? "yes" : "no";
  if (typeof v === "string") return v;
  return JSON.stringify(v);
}

function cell(v) {
  const c = td(text(v), typeof v === "number" ? "num" + (v > 0 ? "" : "") : null);
  if (typeof v === "string" && /^(ok|done|healthy|approved|validated|enabled|filled|active|scored)$/.test(v)) c.classList.add("up");
  if (typeof v === "string" && /^(unavailable|failed|rejected|degraded|stopped|disabled|cancelled)$/.test(v)) c.classList.add("dn");
  return c;
}

/** Rows of objects as a table (columns: union of scalar keys, at most `max`). */
export function rows(list, max = 10, pick) {
  if (!list?.length) return el("p", "st-about", "Nothing yet.");
  const keys = pick || [...new Set(list.flatMap((r) => Object.keys(r).filter((k) => scalar(r[k]) || (Array.isArray(r[k]) && r[k].every(scalar)))))].slice(0, max);
  return table(keys.map(label), list.slice(0, 300).map((r) => row(keys.map((k) => cell(Array.isArray(r[k]) ? r[k].join(", ") : r[k])))));
}

/** An object: scalars as a two-column table, nested objects and row lists as titled sub-sections. */
export function obj(o, depth = 0) {
  if (o == null) return el("p", "st-about", "–");
  if (Array.isArray(o)) return o.length && typeof o[0] === "object" ? rows(o) : el("p", null, o.map(text).join(", "));
  const box = el("div", "kv");
  const flat = Object.entries(o).filter(([, v]) => scalar(v) || (Array.isArray(v) && v.every(scalar)));
  if (flat.length) box.append(table(["Field", "Value"], flat.map(([k, v]) => row([label(k), cell(Array.isArray(v) ? v.join(", ") : v)]))));
  if (depth < 2)
    for (const [k, v] of Object.entries(o)) {
      if (scalar(v) || (Array.isArray(v) && v.every(scalar))) continue;
      const d = el("details");
      if (depth === 0) d.open = true;
      d.append(el("summary", null, label(k)), obj(v, depth + 1));
      box.append(d);
    }
  return box;
}

export function card(title, ...kids) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, title), ...kids);
  return c;
}

/** Poll a job until it ends, showing progress in `status`. Resolves with the job. */
export async function waitJob(id, status) {
  for (;;) {
    const j = await api(`/api/jobs/${id}`);
    if (status) status.textContent = `${j.kind} job #${id}: ${j.status}${j.progress != null && j.status === "running" ? ` (${Math.round(j.progress * 100)}%)` : ""}`;
    if (["done", "failed", "cancelled"].includes(j.status)) return j;
    await new Promise((r) => setTimeout(r, 700));
  }
}

export { badge };
