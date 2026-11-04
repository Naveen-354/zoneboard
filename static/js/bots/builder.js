// Create / edit a bot. The strategy spec (JSON) is the single source of truth: the visual editor reads and writes
// it, and the Advanced panel shows the same JSON for direct edits. The server validates everything on save.
import { el, api, post, put, field, input, select, seg, button, badge, say, avatar } from "./ui.js";

const COLORS = ["#4f46e5", "#0b9f6e", "#e5484d", "#f59e0b", "#0ea5e9", "#a855f7", "#14b8a6", "#334155"];
const GROUPS = { trend: "Trend following", momentum: "Momentum", mean_reversion: "Mean reversion", volume: "Volume", advanced: "Advanced" };
const OPS = [
  { value: "crosses_above", label: "crosses above" },
  { value: "crosses_below", label: "crosses below" },
  { value: "gt", label: ">" },
  { value: "gte", label: "≥" },
  { value: "lt", label: "<" },
  { value: "lte", label: "≤" },
  { value: "rising", label: "is rising over" },
  { value: "falling", label: "is falling over" },
];

let ctx = null;
let host = null;
let draft = null;
let editingId = null;

const clone = (x) => JSON.parse(JSON.stringify(x));

export function init(c) {
  ctx = c;
}

export function blank() {
  const t = ctx.library.templates.find((x) => x.id === "ema_cross");
  return {
    name: "",
    description: "",
    avatar: { glyph: "", color: COLORS[0] },
    market: "crypto",
    mode: "demo",
    account_id: "demo-1",
    symbol: ctx.symbols[0] || "BTCUSDT",
    symbol2: "",
    interval: "1h",
    spec: clone(t.spec),
    template: t.id,
    source: null,
    allocation: "1000",
    max_daily_loss: "",
    max_open_positions: 10,
    slippage_pct: 0.05,
  };
}

/** Start the builder from a draft (a template, a catalog bot, or an existing bot to edit). */
export function load(d, id = null) {
  draft = { ...blank(), ...clone(d) };
  editingId = id;
  if (host) render(host);
}

export const current = () => (draft ? clone(draft) : null);

function kindOf(spec) {
  return spec?.kind || "signal";
}

// ---------------------------------------------------------------- rendering

export function render(h) {
  host = h;
  if (!draft) draft = blank();
  host.replaceChildren(head(), identity(), market(), strategy(), risk(), capital(), advanced(), actions());
}

function card(title, sub, ...children) {
  const c = el("section", "card st-card");
  const hd = el("div", "st-card-h");
  hd.append(el("h3", null, title));
  if (sub) hd.append(el("span", "jsub", sub));
  c.append(hd, ...children);
  return c;
}

function head() {
  const d = el("div", "st-build-head");
  d.append(el("b", null, editingId ? `Editing: ${draft.name || "bot"}` : "Create a bot"));
  if (draft.source) {
    const src = ctx.catalog.find((x) => x.id === draft.source);
    if (src) d.append(badge(`Inspired by ${src.name}: a ZoneBoard template, not an integration`, "info"));
  }
  return d;
}

function identity() {
  const name = input(draft.name, { maxlength: 60, placeholder: "My EMA bot" });
  name.addEventListener("input", () => (draft.name = name.value));
  const desc = el("textarea");
  desc.maxLength = 500;
  desc.rows = 2;
  desc.placeholder = "What it does, in a sentence";
  desc.value = draft.description;
  desc.addEventListener("input", () => (draft.description = desc.value));
  const glyph = input(draft.avatar.glyph, { maxlength: 2, placeholder: "AB", class: "glyph" });
  const preview = el("div", "avt-wrap");
  const paint = () => preview.replaceChildren(avatar(draft.avatar, draft.name));
  glyph.addEventListener("input", () => {
    draft.avatar.glyph = glyph.value;
    paint();
  });
  name.addEventListener("input", paint);
  const sw = el("div", "swatches");
  for (const c of COLORS) {
    const b = el("button", "swatch" + (c === draft.avatar.color ? " on" : ""));
    b.type = "button";
    b.style.background = c;
    b.title = c;
    b.setAttribute("aria-label", `colour ${c}`);
    b.addEventListener("click", () => {
      draft.avatar.color = c;
      for (const x of sw.children) x.classList.toggle("on", x === b);
      paint();
    });
    sw.append(b);
  }
  paint();
  const av = el("div", "st-avatar");
  av.append(preview, field("Initials", glyph), field("Colour", sw));
  const g = el("div", "st-grid");
  g.append(field("Name", name), av);
  return card("1 · Identity", null, g, field("Description", desc));
}

function market() {
  const g = el("div", "st-grid");
  const iv = select(ivOptions(), draft.interval, (v) => (draft.interval = v));
  const sym = input(draft.symbol, { maxlength: 20, list: "stSymbols", spellcheck: "false" });
  sym.addEventListener("input", () => (draft.symbol = sym.value.trim().toUpperCase()));
  const dl = el("datalist");
  dl.id = "stSymbols";
  for (const s of draft.market === "crypto" ? ctx.symbols : []) {
    const o = el("option");
    o.value = s;
    dl.append(o);
  }
  const mk = seg(
    [
      { value: "crypto", label: "Crypto" },
      { value: "india", label: "NSE" },
    ],
    draft.market,
    (v) => {
      draft.market = v;
      draft.symbol = v === "crypto" ? ctx.symbols[0] || "BTCUSDT" : "RELIANCE";
      draft.symbol2 = "";
      if (!ivOptions().some((o) => o.value === draft.interval)) draft.interval = v === "crypto" ? "1h" : "1d";
      render(host);
    },
  );
  const exch = el("div", "st-exch");
  exch.append(
    el("b", null, draft.market === "crypto" ? "Binance" : "FYERS"),
    el("small", null, draft.market === "crypto" ? "Binance public market data (live)." : "NSE data via Yahoo, about 15 min delayed, until the FYERS feed is built."),
  );
  const accts = ctx.accounts.filter((a) => draft.market === "crypto" || a.kind === "demo");
  const acct = select(
    accts.map((a) => ({ value: a.id, label: a.kind === "demo" ? `${a.label} · DEMO (virtual)` : `${a.label} · ${a.environment === "live" ? "LIVE (real money)" : "TESTNET"}` })),
    draft.account_id,
    (v) => {
      draft.account_id = v;
      draft.mode = ctx.accounts.find((a) => a.id === v)?.kind === "demo" ? "demo" : "live";
      render(host);
    },
  );
  const a = ctx.accounts.find((x) => x.id === draft.account_id);
  const modeNote = !a || a.kind === "demo" ? "Demo: paper trading with virtual funds at real prices." : a.environment === "live" ? "Live: real orders on your Binance account, only after an approval of this exact bot and with live trading armed." : "Binance Spot Testnet: real orders with practice funds, after a saved backtest.";
  g.append(field("Market", mk), field("Exchange", exch), field("Symbol", sym), field("Timeframe", iv), field("Account (decides the trading mode)", acct, modeNote));
  if (kindOf(draft.spec) === "pairs") {
    const s2 = input(draft.symbol2 || "", { maxlength: 20, list: "stSymbols", placeholder: draft.market === "crypto" ? "ETHUSDT" : "ICICIBANK" });
    s2.addEventListener("input", () => (draft.symbol2 = s2.value.trim().toUpperCase()));
    g.append(field("Second symbol (pair leg B)", s2));
  }
  const parts = [g, dl];
  if (a && a.kind !== "demo") parts.push(liveNote(a));
  return card("2 · Market", null, ...parts);
}

function liveNote(a) {
  const n = el("div", "st-live-note");
  n.append(
    el("b", null, a.environment === "live" ? "This bot would trade REAL money on Binance." : "This bot sends real orders to the Binance Spot Testnet (practice funds)."),
    el("p", null, "Spot only: long-only strategies, no shorts or pairs. Every order passes the risk engine. " + (a.environment === "live" ? "It cannot start until you approve this exact configuration in the Approval Center and arm live trading; any later change needs a new approval." : "It cannot start until a backtest of this exact strategy version is saved.")),
  );
  return n;
}

function ivOptions() {
  const list = ctx.library.intervals[draft.market] || [];
  return list.map((v) => ({ value: v, label: v }));
}

// ---------------------------------------------------------------- strategy

function strategy() {
  const tpl = select(
    [{ value: "", label: "Custom (start from the current strategy)" }, ...Object.entries(GROUPS).map(([g, label]) => ({ group: label, items: ctx.library.templates.filter((t) => t.group === g).map((t) => ({ value: t.id, label: t.name })) }))],
    draft.template || "",
    (v) => {
      if (!v) {
        draft.template = null;
        return;
      }
      const t = ctx.library.templates.find((x) => x.id === v);
      draft.spec = clone(t.spec);
      draft.template = v;
      if (t.spec.kind === "grid") draft.max_open_positions = Math.max(draft.max_open_positions, (t.spec.grid.levels || 10) - 1);
      render(host);
    },
  );
  const t = ctx.library.templates.find((x) => x.id === draft.template);
  const about = el("p", "st-about", t ? t.about : "Custom strategy.");
  const kind = kindOf(draft.spec);
  const body = [field("Template", tpl), about];
  if (kind === "signal") {
    const dir = seg(
      [
        { value: "long", label: "Long only" },
        { value: "short", label: "Short only" },
        { value: "both", label: "Long and short" },
      ],
      draft.spec.direction || "long",
      (v) => {
        draft.spec.direction = v;
        changed();
        render(host);
      },
    );
    body.push(field("Direction", dir), indicatorsEditor());
    const dirn = draft.spec.direction || "long";
    for (const side of ["long", "short"]) {
      if (dirn !== "both" && dirn !== side) continue;
      body.push(condBlock(`Enter ${side}`, "entry", side), condBlock(`Exit ${side}`, "exit", side));
    }
  } else if (kind === "grid") body.push(gridEditor());
  else if (kind === "dca") body.push(dcaEditor(), indicatorsEditor(), condBlock("Start a cycle when (optional)", "entry", "long"));
  else if (kind === "pairs") body.push(pairsEditor());
  return card("3 · Strategy", `Type: ${kind}`, ...body);
}

function indicatorsEditor() {
  const box = el("div", "st-inds");
  box.append(el("h4", null, "Indicators"));
  const list = (draft.spec.indicators ||= []);
  list.forEach((ind, i) => {
    const spec = ctx.library.indicators.find((x) => x.kind === ind.type);
    const r = el("div", "st-ind");
    const id = input(ind.id, { maxlength: 24, class: "ind-id", "aria-label": "indicator id" });
    id.addEventListener("change", () => {
      const old = ind.id;
      const v = id.value.trim().replace(/[^A-Za-z0-9_]/g, "_") || old;
      ind.id = v;
      renameRefs(draft.spec, old, v);
      changed();
      render(host);
    });
    r.append(id, el("span", "ind-kind", spec ? spec.name : ind.type));
    for (const p of spec?.params || []) {
      const v = ind.params?.[p.name] ?? p.default;
      const n = input(v, { type: "number", step: p.integer ? 1 : "any", min: p.min, max: p.max, "aria-label": p.name });
      n.addEventListener("change", () => {
        (ind.params ||= {})[p.name] = Number(n.value);
        changed();
      });
      const l = el("label", "ind-p");
      l.append(el("small", null, p.name), n);
      r.append(l);
    }
    r.append(
      button("✕", "icon-btn", () => {
        list.splice(i, 1);
        changed();
        render(host);
      }, "Remove indicator"),
    );
    box.append(r);
  });
  const add = select(
    [{ value: "", label: "+ Add indicator…" }, ...Object.entries(GROUPS).map(([g, label]) => ({ group: label, items: ctx.library.indicators.filter((x) => x.group === g).map((x) => ({ value: x.kind, label: x.name })) }))],
    "",
    (v) => {
      if (!v) return;
      const spec = ctx.library.indicators.find((x) => x.kind === v);
      let id = v.replace(/[^a-z]/g, "").slice(0, 6) || "ind";
      let k = 1;
      while (list.some((x) => x.id === id)) id = `${v.slice(0, 5)}${++k}`;
      list.push({ id, type: v, params: Object.fromEntries(spec.params.map((p) => [p.name, p.default])) });
      changed();
      render(host);
    },
  );
  box.append(add);
  return box;
}

function renameRefs(node, from, to) {
  if (Array.isArray(node)) node.forEach((x) => renameRefs(x, from, to));
  else if (node && typeof node === "object") {
    if (node.ind === from) node.ind = to;
    for (const v of Object.values(node)) renameRefs(v, from, to);
  }
}

/** Every operand the visual editor offers: indicator outputs, price fields, a number. */
function operandOptions() {
  const out = [];
  for (const ind of draft.spec.indicators || []) {
    const spec = ctx.library.indicators.find((x) => x.kind === ind.type);
    for (const o of spec?.outputs || ["value"]) out.push({ value: `ind:${ind.id}:${o}`, label: spec && spec.outputs.length > 1 ? `${ind.id} · ${o}` : ind.id });
  }
  const prices = ctx.library.price_fields.map((p) => ({ value: `price:${p}`, label: `price ${p}` }));
  return [{ group: "Indicators", items: out }, { group: "Price", items: prices }, { value: "value", label: "a number" }];
}

function operandKey(o) {
  if (o == null) return "value";
  if (typeof o === "number" || o.value !== undefined) return "value";
  if (o.price) return `price:${o.price}`;
  const spec = ctx.library.indicators.find((x) => x.kind === (draft.spec.indicators || []).find((i) => i.id === o.ind)?.type);
  return `ind:${o.ind}:${o.out || spec?.outputs?.[0] || "value"}`;
}

function operandEditor(o, setO, allowShift = true) {
  const box = el("span", "opnd");
  const key = operandKey(o);
  const sel = select(operandOptions(), key, (v) => {
    if (v === "value") setO({ value: 0 });
    else if (v.startsWith("price:")) setO({ price: v.slice(6) });
    else {
      const [, id, out] = v.split(":");
      setO({ ind: id, out });
    }
    render(host);
  });
  box.append(sel);
  if (key === "value") {
    const n = input(typeof o === "number" ? o : o?.value ?? 0, { type: "number", step: "any", class: "num-in", "aria-label": "number" });
    n.addEventListener("change", () => setO({ value: Number(n.value) }));
    box.append(n);
  } else if (allowShift) {
    const sh = input(o.shift || 0, { type: "number", min: 0, max: 500, step: 1, class: "shift-in", title: "bars ago", "aria-label": "bars ago" });
    sh.addEventListener("change", () => {
      const v = Math.max(0, Math.floor(Number(sh.value) || 0));
      setO({ ...o, shift: v || undefined });
    });
    const l = el("label", "shift");
    l.append(sh, el("small", null, "bars ago"));
    box.append(l);
  }
  return box;
}

function condBlock(title, key, side) {
  const box = el("div", "st-cond");
  const hd = el("div", "st-cond-h");
  hd.append(el("h4", null, title));
  const root = (draft.spec[key] ||= {})[side];
  if (!root) {
    hd.append(
      button("+ Add rule", "btn ghost sm", () => {
        draft.spec[key][side] = { all: [newLeaf()] };
        changed();
        render(host);
      }),
    );
    box.append(hd, el("p", "st-about", key === "exit" ? "No exit rule: positions close only by stop-loss, take-profit, trailing stop or max hold." : "No rule yet."));
    return box;
  }
  hd.append(
    button("Remove", "btn ghost sm", () => {
      delete draft.spec[key][side];
      changed();
      render(host);
    }),
  );
  // the root is shown as a group; a bare rule becomes a one-rule group on first edit
  const group = root.all || root.any ? root : { all: [root] };
  if (group !== root) draft.spec[key][side] = group;
  box.append(hd, groupEditor(group, 0, () => {}));
  return box;
}

function newLeaf() {
  const first = (draft.spec.indicators || [])[0];
  return first ? { left: { ind: first.id }, op: "gt", right: { value: 0 } } : { left: { price: "close" }, op: "gt", right: { value: 0 } };
}

function groupEditor(g, depth, onRemove) {
  const box = el("div", "st-group" + (depth ? " nested" : ""));
  const mode = g.all ? "all" : "any";
  const items = g[mode];
  const top = el("div", "st-group-h");
  top.append(
    seg(
      [
        { value: "all", label: "ALL of" },
        { value: "any", label: "ANY of" },
      ],
      mode,
      (v) => {
        if (v === mode) return;
        g[v] = items;
        delete g[mode];
        changed();
        render(host);
      },
    ),
  );
  if (depth) top.append(button("✕", "icon-btn", onRemove, "Remove group"));
  box.append(top);
  items.forEach((c, i) => {
    const remove = () => {
      items.splice(i, 1);
      if (!items.length) items.push(newLeaf());
      changed();
      render(host);
    };
    if (c.all || c.any) box.append(groupEditor(c, depth + 1, remove));
    else if (c.not) {
      const n = el("div", "st-rule");
      n.append(el("span", "st-about", "NOT (edit in Advanced JSON)"), button("✕", "icon-btn", remove, "Remove"));
      box.append(n);
    } else box.append(leafEditor(c, remove));
  });
  const add = el("div", "st-group-add");
  add.append(
    button("+ Rule", "btn ghost sm", () => {
      items.push(newLeaf());
      changed();
      render(host);
    }),
  );
  if (depth < 2)
    add.append(
      button("+ Group", "btn ghost sm", () => {
        items.push({ any: [newLeaf()] });
        changed();
        render(host);
      }),
    );
  box.append(add);
  return box;
}

function leafEditor(c, remove) {
  const r = el("div", "st-rule");
  r.append(
    operandEditor(c.left, (o) => {
      c.left = o;
      changed();
    }),
  );
  r.append(
    select(OPS, c.op, (v) => {
      c.op = v;
      if ((v === "rising" || v === "falling") && !(typeof c.right === "number" || c.right?.value !== undefined)) c.right = { value: 1 };
      changed();
      render(host);
    }),
  );
  if (c.op === "rising" || c.op === "falling") {
    const n = input(c.right?.value ?? c.right ?? 1, { type: "number", min: 1, max: 500, step: 1, class: "num-in", "aria-label": "bars" });
    n.addEventListener("change", () => {
      c.right = { value: Math.max(1, Math.floor(Number(n.value) || 1)) };
      changed();
    });
    const l = el("span", "opnd");
    l.append(n, el("small", null, "bars"));
    r.append(l);
  } else
    r.append(
      operandEditor(c.right, (o) => {
        c.right = o;
        changed();
      }),
    );
  r.append(button("✕", "icon-btn", remove, "Remove rule"));
  return r;
}

function numField(obj, key, label, attrs, hint) {
  const n = input(obj[key] ?? "", { type: "number", step: "any", ...attrs });
  n.addEventListener("change", () => {
    if (n.value === "") delete obj[key];
    else obj[key] = Number(n.value);
    changed();
  });
  return field(label, n, hint);
}

function gridEditor() {
  const g = (draft.spec.grid ||= {});
  const box = el("div", "st-grid");
  box.append(
    numField(g, "levels", "Grid lines", { min: 2, max: 100, step: 1 }, "Lines evenly spaced between the bounds"),
    numField(g, "range_pct", "Range ± % around the start price", { min: 0.5, max: 90 }, "Used when no absolute bounds are set"),
    numField(g, "lower", "Lower bound (optional)", { min: 0 }),
    numField(g, "upper", "Upper bound (optional)", { min: 0 }),
    numField(g, "stop_below_pct", "Stop: % below the lowest line", { min: 0.1, max: 90 }, "Closes everything and halts the grid"),
  );
  const wrap = el("div");
  wrap.append(el("p", "st-about", "Buys one lot at each line below the last close, sells it one line up. Capital is split evenly across the cells. Long only."), box);
  return wrap;
}

function dcaEditor() {
  const d = (draft.spec.dca ||= {});
  const box = el("div", "st-grid");
  box.append(
    numField(d, "base_pct", "Base order, % of capital", { min: 0.1, max: 100 }),
    numField(d, "safety_pct", "First safety order, % of capital", { min: 0, max: 100 }),
    numField(d, "safety_orders", "Safety orders", { min: 0, max: 25, step: 1 }),
    numField(d, "step_pct", "First step below base, %", { min: 0.1, max: 50 }),
    numField(d, "step_scale", "Step scale", { min: 0.5, max: 3 }, "Each step is this many times wider"),
    numField(d, "volume_scale", "Volume scale", { min: 0.5, max: 3 }, "Each safety order is this many times larger"),
    numField(d, "take_profit_pct", "Take profit over average, %", { min: 0.1, max: 100 }),
    numField(d, "stop_loss_pct", "Stop-loss below average, % (optional)", { min: 0.1, max: 95 }),
  );
  const rs = el("input");
  rs.type = "checkbox";
  rs.checked = d.restart !== false;
  rs.addEventListener("change", () => {
    d.restart = rs.checked;
    changed();
  });
  const l = el("label", "set-check");
  l.append(rs, document.createTextNode(" Start a new cycle after each take-profit or stop"));
  const wrap = el("div");
  wrap.append(box, l);
  return wrap;
}

function pairsEditor() {
  const p = (draft.spec.pairs ||= {});
  const box = el("div", "st-grid");
  const hedge = select(
    [
      { value: "ratio", label: "Price ratio: ln A − ln B (pairs trading)" },
      { value: "ols", label: "Rolling OLS hedge ratio (statistical arbitrage)" },
    ],
    p.hedge || "ratio",
    (v) => {
      p.hedge = v;
      changed();
    },
  );
  box.append(
    field("Spread", hedge),
    numField(p, "lookback", "Lookback bars", { min: 10, max: 1000, step: 1 }),
    numField(p, "entry_z", "Enter at |z| ≥", { min: 0.5, max: 10 }),
    numField(p, "exit_z", "Exit at |z| ≤", { min: 0, max: 10 }),
    numField(p, "stop_z", "Stop at |z| ≥ (optional)", { min: 0.5, max: 20 }),
  );
  const wrap = el("div");
  wrap.append(el("p", "st-about", "When the spread is rich it sells A and buys B (and the reverse when cheap), half the capital per leg, both legs at once."), box);
  return wrap;
}

// ---------------------------------------------------------------- risk and capital

function levelEditor(key, label) {
  const cur = draft.spec[key];
  const type = cur ? cur.type : "";
  const box = el("div", "st-level");
  box.append(
    select(
      [
        { value: "", label: "None" },
        { value: "pct", label: "% of price" },
        { value: "atr", label: "× ATR" },
      ],
      type,
      (v) => {
        if (!v) draft.spec[key] = null;
        else draft.spec[key] = { type: v, value: cur?.value ?? (v === "atr" ? 2 : 2), ...(v === "atr" ? { period: cur?.period ?? 14 } : {}) };
        changed();
        render(host);
      },
    ),
  );
  if (cur) {
    const n = input(cur.value, { type: "number", step: "any", min: 0.01, class: "num-in", "aria-label": `${label} value` });
    n.addEventListener("change", () => {
      cur.value = Number(n.value);
      changed();
    });
    box.append(n);
    if (cur.type === "atr") {
      const p = input(cur.period ?? 14, { type: "number", min: 1, max: 200, step: 1, class: "num-in", title: "ATR period", "aria-label": "ATR period" });
      p.addEventListener("change", () => {
        cur.period = Number(p.value);
        changed();
      });
      const l = el("span", "opnd");
      l.append(el("small", null, "ATR"), p);
      box.append(l);
    }
  }
  return field(label, box);
}

function risk() {
  const kind = kindOf(draft.spec);
  const g = el("div", "st-grid");
  if (kind === "signal") {
    g.append(levelEditor("stop_loss", "Stop-loss"), levelEditor("take_profit", "Take-profit"), levelEditor("trailing_stop", "Trailing stop"));
  }
  if (kind === "signal" || kind === "pairs") {
    g.append(numField(draft.spec, "max_hold_bars", "Max bars in a trade (0 = no limit)", { min: 0, max: 10000, step: 1 }));
  }
  if (kind === "signal" || kind === "pairs") {
    const sz = (draft.spec.sizing ||= { type: "pct_capital", value: 100 });
    const types = [
      { value: "pct_capital", label: "% of bot capital" },
      { value: "fixed_notional", label: "Fixed amount (quote currency)" },
      { value: "fixed_qty", label: "Fixed quantity" },
      { value: "risk_pct", label: "Risk % of capital to the stop" },
    ];
    const t = select(kind === "pairs" ? types.slice(0, 2) : types, sz.type, (v) => {
      sz.type = v;
      changed();
    });
    const n = input(sz.value, { type: "number", step: "any", min: 0, class: "num-in", "aria-label": "size" });
    n.addEventListener("change", () => {
      sz.value = Number(n.value);
      changed();
    });
    const b = el("div", "st-level");
    b.append(t, n);
    g.append(field("Position size", b, "Never more than the bot's unused capital"));
  }
  if (!g.children.length) g.append(el("p", "st-about", kind === "grid" ? "Grid sizing is the capital split across the grid cells; the grid's own stop is above." : "DCA sizing and its stop are set in the strategy above."));
  return card("4 · Risk per trade", null, g);
}

function capital() {
  const g = el("div", "st-grid");
  const money = (key, label, hint) => {
    const i = input(draft[key] ?? "", { inputmode: "decimal", placeholder: key === "max_daily_loss" ? "no limit" : "" });
    i.addEventListener("input", () => (draft[key] = i.value.trim()));
    return field(label, i, hint);
  };
  const acct = ctx.accounts.find((a) => a.id === draft.account_id);
  const cur = draft.market === "crypto" ? "USDT" : "INR";
  const w = acct?.wallets?.find((x) => x.currency === cur);
  g.append(money("allocation", `Capital allocated (${cur})`, w ? `Account has ${w.available} ${cur} of virtual credit available` : null), money("max_daily_loss", `Max daily loss (${cur})`, "New orders stop for the day once reached"));
  const mo = input(draft.max_open_positions, { type: "number", min: 1, max: 100, step: 1 });
  mo.addEventListener("change", () => (draft.max_open_positions = Number(mo.value)));
  const sl = input(draft.slippage_pct, { type: "number", min: 0, max: 2, step: "any" });
  sl.addEventListener("change", () => (draft.slippage_pct = Number(sl.value)));
  g.append(field("Max open positions", mo), field("Slippage per market fill, %", sl, "Charged against you in backtests and demo fills"));
  const note = el("p", "st-about", "Account-wide limits (max order size, exposure, daily loss) also apply to every bot order: set them under Bot Performance → Risk.");
  return card("5 · Capital and limits", null, g, note);
}

function advanced() {
  const d = el("details", "card st-card st-adv");
  const s = el("summary", null, "Advanced: strategy JSON");
  const ta = el("textarea", "json");
  ta.spellcheck = false;
  ta.rows = 14;
  ta.value = JSON.stringify(draft.spec, null, 2);
  ta.id = "stJson";
  const msg = el("p", "st-msg");
  const apply = button("Apply JSON", "btn ghost sm", () => {
    try {
      const v = JSON.parse(ta.value);
      if (!v || typeof v !== "object" || Array.isArray(v)) throw new Error("the strategy must be a JSON object");
      draft.spec = v;
      draft.template = null;
      render(host);
      say(document.querySelector("#stJsonMsg") || msg, "Applied. The server validates it when you backtest or save.", "ok");
    } catch (e) {
      say(msg, `Not valid JSON: ${e.message}`, "err");
    }
  });
  msg.id = "stJsonMsg";
  d.append(s, el("p", "st-about", "The same strategy the visual editor builds. Anything the editor cannot show (NOT, deeper nesting) can be written here."), ta, apply, msg);
  return d;
}

// The template id stays as a record of where the strategy started, even after edits.
function changed() {
  const ta = document.querySelector("#stJson");
  if (ta) ta.value = JSON.stringify(draft.spec, null, 2);
}

function actions() {
  const bar = el("div", "st-actions");
  const msg = el("p", "st-msg");
  msg.setAttribute("role", "status");
  const save = button(editingId ? "Save changes" : draft.mode === "live" ? "Create bot (broker account)" : "Create bot (Demo)", "btn", async () => {
    say(msg, "Saving…");
    save.disabled = true;
    try {
      const body = { ...draft, symbol2: kindOf(draft.spec) === "pairs" ? draft.symbol2 : null, max_daily_loss: draft.max_daily_loss || null };
      const bot = editingId ? await put(`/api/bots/${editingId}`, body) : await post("/api/bots", body);
      say(msg, editingId ? "Saved." : `Created "${bot.name}". It is stopped: start it from My Bots.`, "ok");
      editingId = bot.id;
      ctx.onSaved(bot);
    } catch (e) {
      say(msg, e.message, "err");
    } finally {
      save.disabled = false;
    }
  });

  const bt = button("Backtest this strategy", "btn ghost", () => ctx.goBacktest(current()));
  const reset = button("New blank bot", "btn ghost", () => load(blank()));
  bar.append(save, bt, reset, msg);
  return bar;
}

export async function refreshAccounts() {
  try {
    ctx.accounts = (await api("/api/accounts")).accounts.filter((a) => !a.archived);
  } catch {
    /* keep the old list */
  }
}
