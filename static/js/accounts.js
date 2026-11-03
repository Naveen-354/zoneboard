// Accounts page: the practice (demo) accounts with their VIRTUAL credits, and where broker accounts will appear.
// Loaded on first visit, so it adds nothing to the page-load path. DOM is built with textContent only.
import { $, el, api } from "./util.js";

const JSON_HEADERS = { "Content-Type": "application/json" };
let onChange = () => {};
let onLoad = () => {};

/** Called after an account change that can alter open positions (a reset), so the chart screen reloads them. */
export function init(opts) {
  onChange = opts.onChange || onChange;
  onLoad = opts.onLoad || onLoad;
}

/** "1234567.5" -> "1,234,567.5". The API sends decimal strings; they stay strings, so nothing is rounded by a float. */
export function money(s) {
  const m = /^(-?)(\d+)(?:\.(\d+))?$/.exec(String(s));
  if (!m) return String(s);
  return m[1] + m[2].replace(/\B(?=(\d{3})+(?!\d))/g, ",") + (m[3] ? "." + m[3] : "");
}

function say(node, text, kind) {
  node.textContent = text;
  node.className = "acct-msg" + (kind ? " " + kind : "");
}

function field(label, input) {
  const l = el("label", null, label);
  l.append(input);
  return l;
}

function numberInput(value, name) {
  const i = el("input");
  i.type = "text";
  i.inputMode = "decimal";
  i.autocomplete = "off";
  i.value = value;
  i.name = name;
  return i;
}

function walletTable(a) {
  const t = el("table", "acct-wallets");
  const head = el("tr");
  for (const h of ["Currency", "Credits", "Balance", "In positions", "Available"]) head.append(el("th", null, h));
  const thead = el("thead");
  thead.append(head);
  t.append(thead);
  const body = el("tbody");
  for (const w of a.wallets) {
    const tr = el("tr");
    const neg = w.available.startsWith("-");
    tr.append(el("td", null, w.currency), el("td", "num", money(w.credits)), el("td", "num", money(w.balance)), el("td", "num", money(w.in_positions)), el("td", "num" + (neg ? " dn" : ""), money(w.available)));
    body.append(tr);
  }
  t.append(body);
  const wrap = el("div", "table-wrap");
  wrap.append(t);
  return wrap;
}

function card(a, reload) {
  const c = el("article", "card acct");
  c.dataset.account = a.id;
  const head = el("div", "acct-head");
  head.append(el("b", null, a.label), el("span", "vbadge", "VIRTUAL"), el("small", null, `${a.id} · ${a.environment} · ${a.open_positions} open position${a.open_positions === 1 ? "" : "s"}`));
  const msg = el("p", "acct-msg");
  msg.setAttribute("role", "status");
  msg.setAttribute("aria-live", "polite");

  const actions = el("div", "acct-actions");
  const edit = el("button", "btn ghost", "Edit credits");
  edit.type = "button";
  const reset = el("button", "btn ghost", "Reset…");
  reset.type = "button";
  actions.append(edit, reset);

  const form = el("form", "acct-edit");
  form.hidden = true;
  const label = el("input");
  label.type = "text";
  label.maxLength = 60;
  label.value = a.label;
  label.name = "label";
  form.append(field("Name", label));
  const inputs = {};
  for (const w of a.wallets) {
    inputs[w.currency] = numberInput(w.credits, w.currency);
    form.append(field(`${w.currency} credits (virtual)`, inputs[w.currency]));
  }
  const save = el("button", "btn", "Save");
  save.type = "submit";
  const cancel = el("button", "btn ghost", "Cancel");
  cancel.type = "button";
  form.append(save, cancel);

  edit.addEventListener("click", () => {
    form.hidden = !form.hidden;
    if (!form.hidden) label.focus();
  });
  cancel.addEventListener("click", () => (form.hidden = true));
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = {};
    if (label.value.trim() !== a.label) body.label = label.value.trim();
    const credits = {};
    for (const w of a.wallets) if (inputs[w.currency].value.trim() !== w.credits) credits[w.currency] = inputs[w.currency].value.trim();
    if (Object.keys(credits).length) body.credits = credits;
    if (!Object.keys(body).length) return say(msg, "Nothing changed.");
    save.disabled = true;
    try {
      await api(`/api/accounts/${encodeURIComponent(a.id)}`, { method: "PATCH", headers: JSON_HEADERS, body: JSON.stringify(body) });
      await reload("Saved.");
    } catch (err) {
      say(msg, err.message, "err"); // e.g. "credits.USDT allows at most 8 decimal places"
      save.disabled = false;
    }
  });

  reset.addEventListener("click", async () => {
    const n = a.open_positions;
    const ok = confirm(
      `Reset "${a.label}"?\n\nVirtual balances go back to the credits. ${n} open practice position${n === 1 ? " is" : "s are"} removed without logging a trade. Your trade history is kept.`,
    );
    if (!ok) return;
    reset.disabled = true;
    try {
      await api(`/api/accounts/${encodeURIComponent(a.id)}/reset`, { method: "POST" });
      onChange();
      await reload("Account reset.");
    } catch (err) {
      say(msg, err.message, "err");
      reset.disabled = false;
    }
  });

  c.append(head, walletTable(a), actions, form, msg);
  return c;
}

export async function load(note) {
  const list = $("#acctList");
  const reload = (text) => load(text);
  let d;
  try {
    d = await api("/api/accounts");
  } catch (err) {
    list.replaceChildren(el("p", "acct-msg err", `Can't load accounts: ${err.message}`));
    return;
  }
  onLoad(d);
  list.replaceChildren(...d.accounts.map((a) => card(a, reload)));
  if (note) {
    const first = list.querySelector(".acct-msg");
    if (first) say(first, note, "ok");
  }
}

/** The "New demo account" form is static markup; this wires it. */
export function wire() {
  const form = $("#acctNew");
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const label = form.elements.label.value.trim();
    const msg = $("#acctNewMsg");
    if (!label) return say(msg, "Give the account a name.", "err");
    try {
      await api("/api/accounts", { method: "POST", headers: JSON_HEADERS, body: JSON.stringify({ kind: "demo", label }) });
      form.reset();
      say(msg, "Created.", "ok");
      await load();
    } catch (err) {
      say(msg, err.message, "err");
    }
  });
}
