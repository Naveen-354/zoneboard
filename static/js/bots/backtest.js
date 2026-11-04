// Backtesting and optimisation. Everything shown is computed by the server from real (or clearly labelled sample)
// history: metrics, trades, the equity curve and the price chart with the strategy's entries and exits.
import { el, post, field, input, select, button, say, stat, pct, num, money, short, pnl, table, row, td, when, fmtAuto, lineChart, priceChart } from "./ui.js";

let ctx = null;
let host = null;
let source = null; // { kind: "bot", id } or { kind: "draft", draft }
let lastDraft = null; // the builder's unsaved strategy stays selectable after switching to a bot
let disposers = [];

export function init(c) {
  ctx = c;
}

/** Open the tab on a bot, or on the builder's unsaved strategy. */
export function use(s) {
  source = s;
  if (s?.kind === "draft") lastDraft = s;
  if (host) render(host);
}

function dispose() {
  for (const d of disposers) d();
  disposers = [];
}

export function render(h) {
  host = h;
  dispose();
  host.replaceChildren(form(), el("div", "bt-out"));
}

function sourceOptions() {
  const opts = ctx.bots.map((b) => ({ value: `bot:${b.id}`, label: `${b.name} · ${b.symbol}${b.symbol2 ? "/" + b.symbol2 : ""} ${b.interval}` }));
  if (lastDraft) opts.unshift({ value: "draft", label: `Unsaved strategy from Create Bot (${lastDraft.draft.name || "untitled"})` });
  return opts;
}

function currentSpec() {
  if (!source) return null;
  if (source.kind === "draft") return source.draft;
  return ctx.bots.find((b) => b.id === source.id) || null;
}

function form() {
  const c = el("section", "card st-card");
  const hd = el("div", "st-card-h");
  hd.append(el("h3", null, "Backtest"), el("span", "jsub", "Signals on candle close, fills at the next open, fees and slippage charged"));
  c.append(hd);
  const opts = sourceOptions();
  if (!opts.length) {
    c.append(el("p", "st-about", "Create a bot first (or open Create Bot and press Backtest this strategy)."));
    return c;
  }
  if (!source || (source.kind === "bot" && !ctx.bots.some((b) => b.id === source.id))) source = opts[0].value === "draft" ? lastDraft : { kind: "bot", id: Number(opts[0].value.slice(4)) };
  const pick = select(opts, source.kind === "draft" ? "draft" : `bot:${source.id}`, (v) => {
    source = v === "draft" ? lastDraft : { kind: "bot", id: Number(v.slice(4)) };
    render(host);
  });
  const s = currentSpec();
  const g = el("div", "st-grid");
  const sym = input(s.symbol, { maxlength: 20 });
  const sym2 = input(s.symbol2 || "", { maxlength: 20 });
  const ivs = (ctx.library.intervals[s.market] || []).map((v) => ({ value: v, label: v }));
  const iv = select(ivs, s.interval);
  const bars = input(1000, { type: "number", min: 100, max: 5000, step: 100 });
  const cap = input(s.allocation || "10000", { inputmode: "decimal" });
  const fee = input(s.market === "crypto" ? 0.1 : 0.05, { type: "number", min: 0, max: 5, step: "any" });
  const slip = input(s.slippage_pct ?? 0.05, { type: "number", min: 0, max: 5, step: "any" });
  g.append(field("Strategy", pick), field("Symbol", sym));
  if (s.spec?.kind === "pairs") g.append(field("Second symbol", sym2));
  g.append(field("Timeframe", iv), field("Bars of history", bars, "Up to 5000 (NSE: what Yahoo serves)"), field("Starting capital", cap), field("Fee per side, %", fee), field("Slippage, %", slip));
  const saveBox = el("input");
  saveBox.type = "checkbox";
  saveBox.checked = source.kind === "bot";
  saveBox.disabled = source.kind !== "bot";
  const sl = el("label", "set-check");
  sl.append(saveBox, document.createTextNode(" Save the report to this bot (counts toward its live-readiness checklist)"));
  const msg = el("p", "st-msg");
  msg.setAttribute("role", "status");
  const req = () => ({
    ...(source.kind === "bot" ? { bot_id: source.id } : {}),
    spec: s.spec,
    market: s.market,
    symbol: sym.value.trim().toUpperCase(),
    symbol2: s.spec?.kind === "pairs" ? sym2.value.trim().toUpperCase() : null,
    interval: iv.value,
    bars: Number(bars.value),
    capital: Number(cap.value),
    fee_pct: Number(fee.value),
    slippage_pct: Number(slip.value),
  });
  const run = button("Run backtest", "btn", async () => {
    say(msg, "Fetching history and running…");
    run.disabled = true;
    try {
      const r = await post("/api/backtest", { ...req(), save: saveBox.checked && source.kind === "bot" });
      say(msg, r.saved_id ? `Done. Saved as report #${r.saved_id}.` : "Done.", "ok");
      results(r);
    } catch (e) {
      say(msg, e.message, "err");
    } finally {
      run.disabled = false;
    }
  });
  const bar = el("div", "st-actions");
  bar.append(run, msg);
  c.append(g, sl, bar, optimizer(s, req));
  return c;
}

// ---------------------------------------------------------------- results

function results(r) {
  dispose();
  const out = host.querySelector(".bt-out");
  out.replaceChildren();
  const m = r.metrics;
  const warn = r.data.sample ? el("div", "st-live-note", "SAMPLE DATA: these prices are made up. The numbers below say nothing about real markets.") : null;
  const src = el("p", "st-about", `${r.data.source} · ${r.data.symbol}${r.data.symbol2 ? " / " + r.data.symbol2 : ""} ${r.data.interval} · ${r.data.bars} bars from ${when(r.data.from)} to ${when(r.data.to)}`);
  const stats = el("div", "stats");
  const sign = (x) => (x > 0 ? "up" : x < 0 ? "dn" : "");
  stats.append(
    stat("Net profit", pnl(el("span"), m.net_profit, true), "", `Exactly ${money(m.net_profit)}`),
    stat("Return", pct(m.return_pct), sign(m.return_pct)),
    stat("Buy and hold", pct(m.buy_hold_pct), sign(m.buy_hold_pct), "The symbol's own move over the same bars"),
    stat("Trades", String(m.trades)),
    stat("Win rate", pct(m.win_rate)),
    stat("Profit factor", m.profit_factor == null ? (m.trades && !m.losses ? "no losses" : "–") : num(m.profit_factor), "", "Gross profit / gross loss"),
    stat("Sharpe", m.sharpe == null ? "–" : num(m.sharpe), "", "Annualised from per-bar equity returns"),
    stat("Sortino", m.sortino == null ? "–" : num(m.sortino)),
    stat("Max drawdown", pct(m.max_drawdown_pct), m.max_drawdown_pct > 0 ? "dn" : ""),
    stat("Expectancy / trade", short(m.expectancy)),
    stat("Time in market", pct(m.exposure_pct)),
    stat("Fees paid", short(m.fees), "", `Exactly ${money(m.fees)}`),
  );
  const charts = el("div", "bt-charts");
  const pc = el("div", "card st-card");
  pc.append(el("h4", null, "Price, indicators and trades"));
  const pch = el("div", "chart-box tall");
  pc.append(pch);
  const ec = el("div", "card st-card");
  ec.append(el("h4", null, "Equity"));
  const ech = el("div", "chart-box");
  ec.append(ech);
  charts.append(pc, ec);
  const markers = [];
  for (const t of r.trades.filter((t) => t.leg === 0)) {
    markers.push({ time: t.entry_time, position: t.side === "long" ? "belowBar" : "aboveBar", color: t.side === "long" ? "#0b9f6e" : "#e5484d", shape: t.side === "long" ? "arrowUp" : "arrowDown", text: t.side === "long" ? "B" : "S" });
    markers.push({ time: t.exit_time, position: "inBar", color: String(t.pnl).startsWith("-") ? "#e5484d" : "#0b9f6e", shape: "circle", text: "" });
  }
  out.append(...[warn, src, stats, charts].filter(Boolean));
  disposers.push(priceChart(pch, r.candles, r.overlays, markers));
  disposers.push(lineChart(ech, r.equity, { baseline: Number(m.start_capital) }));
  out.append(winLoss(r), tradesTable(r), el("p", "st-about", r.assumptions + (r.open_lots ? ` ${r.open_lots} position(s) were still open at the end.` : "")));
}

function winLoss(r) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, "Win / loss analysis"));
  const m = r.metrics;
  const g = el("div", "stats");
  g.append(stat("Wins", String(m.wins), "up"), stat("Losses", String(m.losses), "dn"), stat("Average win", short(m.avg_win), "up"), stat("Average loss", short(m.avg_loss), "dn"), stat("Largest win", short(m.largest_win)), stat("Largest loss", short(m.largest_loss)), stat("Max losing streak", String(m.max_consecutive_losses)), stat("Avg bars held", num(m.avg_bars_held, 1)));
  c.append(g);
  // distribution of trade results, one bar per trade, scaled to the largest
  const vals = r.trades.map((t) => Number(t.pnl));
  if (vals.length) {
    const max = Math.max(...vals.map(Math.abs)) || 1;
    const hist = el("div", "pnl-bars");
    hist.setAttribute("aria-label", "trade results in order");
    for (const v of vals.slice(-200)) {
      const b = el("i", v >= 0 ? "up" : "dn");
      b.style.height = `${Math.max(2, (Math.abs(v) / max) * 100)}%`;
      b.title = fmtAuto(v);
      hist.append(b);
    }
    c.append(el("small", "st-about", vals.length > 200 ? "Last 200 trades, in order" : "Every trade, in order"), hist);
  }
  // results by exit reason
  const by = new Map();
  for (const t of r.trades) {
    const k = t.reason;
    const e = by.get(k) || { n: 0, sum: 0 };
    e.n++;
    e.sum += Number(t.pnl);
    by.set(k, e);
  }
  if (by.size)
    c.append(
      table(
        ["Exit reason", { label: "Trades", cls: "num" }, { label: "Net P&L", cls: "num" }],
        [...by].map(([k, e]) => row([k, td(String(e.n), "num"), td(fmtAuto(e.sum), "num " + (e.sum >= 0 ? "up" : "dn"))])),
      ),
    );
  return c;
}

function tradesTable(r) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, `Trades (${r.trades.length})`));
  const rows = r.trades
    .slice(-300)
    .reverse()
    .map((t) => {
      const p = td(null, "num");
      pnl(p, t.pnl);
      return row([when(t.entry_time), t.leg ? "B" : "A", t.side, td(fmtAuto(t.qty), "num"), td(fmtAuto(t.entry), "num"), when(t.exit_time), td(fmtAuto(t.exit), "num"), p, t.reason, td(String(t.bars), "num")]);
    });
  c.append(table(["Entry", "Leg", "Side", { label: "Qty", cls: "num" }, { label: "Entry", cls: "num" }, "Exit", { label: "Exit", cls: "num" }, { label: "P&L", cls: "num" }, "Reason", { label: "Bars", cls: "num" }], rows));
  if (!r.trades.length) c.append(el("p", "st-about", "No trades: the entry rules never fired on this history."));
  return c;
}

// ---------------------------------------------------------------- optimisation

/** Every number in the spec that could be varied, with a JSON pointer and a readable label. */
function numericParams(spec) {
  const out = [];
  const walk = (v, path, label) => {
    if (typeof v === "number") out.push({ path, label, value: v });
    else if (Array.isArray(v)) v.forEach((x, i) => walk(x, `${path}/${i}`, `${label}[${i + 1}]`));
    else if (v && typeof v === "object") for (const [k, x] of Object.entries(v)) walk(x, `${path}/${k}`, label ? `${label}.${k}` : k);
  };
  (spec.indicators || []).forEach((ind, i) => {
    for (const [k, v] of Object.entries(ind.params || {})) if (typeof v === "number") out.push({ path: `/indicators/${i}/params/${k}`, label: `${ind.id}.${k}`, value: v });
  });
  for (const k of ["stop_loss", "take_profit", "trailing_stop"]) if (spec[k]) walk(spec[k], `/${k}`, k);
  for (const k of ["grid", "dca", "pairs", "entry", "exit"]) if (spec[k]) walk(spec[k], `/${k}`, k);
  if (typeof spec.max_hold_bars === "number") out.push({ path: "/max_hold_bars", label: "max_hold_bars", value: spec.max_hold_bars });
  return out.filter((p) => !/\/shift$/.test(p.path));
}

function suggest(v) {
  const r = (x) => (Number.isInteger(v) ? Math.max(1, Math.round(x)) : Math.round(x * 100) / 100);
  return [...new Set([r(v * 0.6), r(v * 0.8), v, r(v * 1.25), r(v * 1.5)])].join(", ");
}

function optimizer(s, req) {
  const d = el("details", "st-opt");
  d.append(el("summary", null, "Optimise parameters"));
  const params = numericParams(s.spec || {});
  if (!params.length) {
    d.append(el("p", "st-about", "This strategy has no numeric parameters to vary."));
    return d;
  }
  d.append(el("p", "st-about", "Grid search over the values you list (at most 200 combinations). Parameters are chosen on the first part of history (in-sample) and then tested on the rest (out-of-sample). Trust the out-of-sample column: in-sample results of an optimised strategy always look better than they will be."));
  const rows = [];
  const list = el("div", "opt-list");
  for (const p of params.slice(0, 24)) {
    const cb = el("input");
    cb.type = "checkbox";
    const vals = input(suggest(p.value), { class: "opt-vals", "aria-label": `${p.label} values` });
    const l = el("label", "opt-row");
    l.append(cb, el("code", null, p.label), el("small", null, `now ${p.value}`), vals);
    list.append(l);
    rows.push({ p, cb, vals });
  }
  const obj = select(
    [
      { value: "sharpe", label: "Sharpe ratio" },
      { value: "return", label: "Return" },
      { value: "profit_factor", label: "Profit factor" },
      { value: "win_rate", label: "Win rate" },
      { value: "max_drawdown", label: "Smallest drawdown" },
    ],
    "sharpe",
  );
  const split = input(70, { type: "number", min: 30, max: 90, step: 5 });
  const g = el("div", "st-grid");
  g.append(field("Rank by (in-sample)", obj), field("In-sample share, %", split));
  const msg = el("p", "st-msg");
  const out = el("div");
  const go = button("Run optimisation", "btn ghost", async () => {
    const chosen = rows.filter((r) => r.cb.checked);
    if (!chosen.length) return say(msg, "Tick at least one parameter.", "err");
    const axes = [];
    for (const r of chosen) {
      const values = r.vals.value
        .split(/[,\s]+/)
        .filter(Boolean)
        .map(Number);
      if (!values.length || values.some((x) => !Number.isFinite(x))) return say(msg, `${r.p.label}: list numbers separated by commas.`, "err");
      axes.push({ path: r.p.path, values });
    }
    const n = axes.reduce((a, x) => a * x.values.length, 1);
    if (n > 200) return say(msg, `${n} combinations; at most 200.`, "err");
    say(msg, `Testing ${n} combinations…`);
    go.disabled = true;
    try {
      const res = await post("/api/backtest/optimize", { ...req(), params: axes, objective: obj.value, split: Number(split.value) / 100 });
      say(msg, `Done: ${res.combinations} combinations. ${res.data.sample ? "SAMPLE DATA: made-up prices." : res.data.source}`, "ok");
      out.replaceChildren(optTable(res, s));
    } catch (e) {
      say(msg, e.message, "err");
    } finally {
      go.disabled = false;
    }
  });
  const bar = el("div", "st-actions");
  bar.append(go, msg);
  d.append(list, g, bar, out);
  return d;
}

function optTable(res, s) {
  const wrap = el("div");
  wrap.append(el("p", "st-about", `In-sample ${when(res.in_sample.from)} to ${when(res.in_sample.to)} (${res.in_sample.bars} bars) · out-of-sample ${when(res.out_of_sample.from)} to ${when(res.out_of_sample.to)} (${res.out_of_sample.bars} bars). ${res.note}`));
  const rows = res.results.slice(0, 25).map((r, i) => {
    if (r.error) return row([String(i + 1), JSON.stringify(r.params), td(r.error, "dn"), "", "", "", "", ""]);
    const is = r.in_sample;
    const oos = r.out_of_sample;
    const apply = button("Use", "btn ghost sm", () => {
      const spec = JSON.parse(JSON.stringify(s.spec));
      for (const [path, v] of Object.entries(r.params)) setPointer(spec, path, v);
      ctx.applyToBuilder({ ...s, spec });
    }, "Open these parameters in Create Bot");
    return row([
      String(i + 1),
      td(Object.entries(r.params).map(([k, v]) => `${k.split("/").slice(-2).join(".")}=${v}`).join("  "), "notes"),
      td(pct(is.return_pct), "num"),
      td(is.sharpe == null ? "–" : num(is.sharpe), "num"),
      td(pct(oos.return_pct), "num " + (oos.return_pct >= 0 ? "up" : "dn")),
      td(oos.sharpe == null ? "–" : num(oos.sharpe), "num"),
      td(pct(oos.max_drawdown_pct), "num"),
      td(`${oos.trades}`, "num"),
      apply,
    ]);
  });
  wrap.append(table(["#", "Parameters", { label: "IS return", cls: "num" }, { label: "IS Sharpe", cls: "num" }, { label: "OOS return", cls: "num" }, { label: "OOS Sharpe", cls: "num" }, { label: "OOS max DD", cls: "num" }, { label: "OOS trades", cls: "num" }, ""], rows));
  return wrap;
}

function setPointer(obj, path, v) {
  const keys = path.split("/").slice(1);
  let o = obj;
  for (const k of keys.slice(0, -1)) o = o[k];
  o[keys[keys.length - 1]] = v;
}
