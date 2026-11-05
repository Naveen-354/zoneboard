// Research Lab: Monte Carlo, walk-forward, robustness, A/B experiments, regimes, the bounded researcher,
// news and calendar, the options lab and the trade explainer. Every result says what produced it.
import { $, el, api } from "./util.js";
import { button, field, input, select, seg, say, post } from "./bots/ui.js";
import { rows, obj, card, waitJob } from "./render.js";

const TABS = [
  { value: "montecarlo", label: "Monte Carlo" },
  { value: "walkforward", label: "Walk-forward" },
  { value: "robustness", label: "Robustness" },
  { value: "ab", label: "A/B test" },
  { value: "regimes", label: "Regimes" },
  { value: "researcher", label: "Researcher" },
  { value: "news", label: "News & calendar" },
  { value: "options", label: "Options lab" },
  { value: "explain", label: "Trade explainer" },
];
let tab = "montecarlo";
let wired = false;
let templates = null;

const host = () => $("#labBody");
const msg = () => el("p", "st-msg");

export function wire() {
  if (wired) return;
  wired = true;
  $("#labTabs").replaceWith(Object.assign(seg(TABS, tab, (v) => ((tab = v), load()), "seg st-tabs"), { id: "labTabs" }));
}

export async function load() {
  wire();
  host().replaceChildren(el("p", "st-about", "Loading…"));
  try {
    templates ??= (await api("/api/strategies/library")).templates;
    await PAGES[tab]();
  } catch (e) {
    host().replaceChildren(el("p", "st-msg err", e.message));
  }
}

/** Symbol, interval, bars and a template: the window every research job runs on. */
function windowForm(withTemplate = true) {
  const sym = input("BTCUSDT");
  const iv = select(["5m", "15m", "1h", "4h", "1d"].map((v) => ({ value: v })), "1h");
  const bars = input("1000", { type: "number", min: 100, max: 5000 });
  const tpl = select(templates.map((t) => ({ value: t.id, label: t.name })), "ema_cross");
  const f = el("div", "st-form");
  f.append(field("Symbol", sym), field("Interval", iv), field("Bars", bars));
  if (withTemplate) f.append(field("Strategy template", tpl));
  const body = () => ({ symbol: sym.value.trim().toUpperCase(), interval: iv.value, bars: Number(bars.value), ...(withTemplate ? { spec: templates.find((t) => t.id === tpl.value).spec } : {}) });
  return { f, body, tpl };
}

/** Start a job, poll it, render its result (or its error). */
function runner(path, body, render = obj) {
  const status = msg();
  const out = el("div");
  const go = button("Run", "btn", async () => {
    go.disabled = true;
    out.replaceChildren();
    try {
      const j = await post(path, body());
      const done = await waitJob(j.job_id, status);
      if (done.status === "done") out.replaceChildren(render(done.result));
      else say(status, `${done.kind} job ${done.status}: ${done.error || ""}`, "err");
    } catch (e) {
      say(status, e.message, "err");
    } finally {
      go.disabled = false;
    }
  });
  return { go, status, out };
}

const PAGES = {
  async montecarlo() {
    const bt = input("", { type: "number", placeholder: "saved backtest id" });
    const bot = input("", { type: "number", placeholder: "or a bot id (its recorded trades)" });
    const [sims, seed, cost, ruin] = [input("2000", { type: "number" }), input("42", { type: "number" }), input("0", { type: "number", step: "any" }), input("50", { type: "number" })];
    const method = select([{ value: "bootstrap" }, { value: "block", label: "block bootstrap" }, { value: "shuffle", label: "shuffle (order only)" }], "bootstrap");
    const r = runner("/api/research/montecarlo", () => ({ ...(bt.value ? { backtest_id: Number(bt.value) } : { bot_id: Number(bot.value) }), simulations: Number(sims.value), seed: Number(seed.value), method: method.value, extra_cost: Number(cost.value), ruin_pct: Number(ruin.value) }));
    const f = el("div", "st-form");
    f.append(field("Backtest", bt, "save one in Bot Studio > Backtest"), field("Bot", bot), field("Method", method), field("Simulations", sims), field("Seed", seed, "same seed, same result"), field("Extra cost per trade", cost), field("Ruin at % loss", ruin), r.go, r.status);
    host().replaceChildren(card("Monte Carlo on a saved backtest's or a bot's trades", f, r.out));
  },

  async walkforward() {
    const w = windowForm();
    const [train, test, step] = [input("400", { type: "number" }), input("100", { type: "number" }), input("100", { type: "number" })];
    const mode = select([{ value: "rolling" }, { value: "anchored" }], "rolling");
    const obj_ = select([{ value: "return" }, { value: "sharpe" }, { value: "profit_factor" }], "return");
    const params = input("/indicators/0/params/period=5,9,14", { size: 40 });
    const parse = () =>
      params.value
        .split(";")
        .map((p) => p.trim())
        .filter(Boolean)
        .map((p) => {
          const [path, vals] = p.split("=");
          return { path: path.trim(), values: vals.split(",").map((x) => Number(x.trim())) };
        });
    const r = runner("/api/research/walkforward", () => ({ ...w.body(), mode: mode.value, objective: obj_.value, train_bars: Number(train.value), test_bars: Number(test.value), step_bars: Number(step.value), params: parse() }));
    w.f.append(field("Mode", mode), field("Train bars", train), field("Test bars", test), field("Step", step), field("Objective", obj_), field("Parameters (path=v1,v2; …)", params), r.go, r.status);
    host().replaceChildren(card("Walk-forward optimisation", w.f, r.out));
  },

  async robustness() {
    const w = windowForm();
    const r = runner("/api/research/robustness", w.body);
    w.f.append(r.go, r.status);
    const prev = await api("/api/research/robustness");
    host().replaceChildren(card("Strategy robustness score", el("p", "st-about", "Nine weighted dimensions, each with its evidence. Not a forecast, and never an approval for live trading."), w.f, r.out), card("Previous assessments", rows(prev.assessments, 6)));
  },

  async ab() {
    const w = windowForm(false);
    const a = select(templates.map((t) => ({ value: t.id, label: t.name })), "ema_cross");
    const b = select(templates.map((t) => ({ value: t.id, label: t.name })), "ma_cross");
    const min = input("20", { type: "number" });
    const spec = (id) => templates.find((t) => t.id === id).spec;
    const r = runner("/api/research/experiments", () => ({ ...w.body(), a: spec(a.value), b: spec(b.value), min_trades: Number(min.value) }));
    w.f.append(field("Variant A", a), field("Variant B", b), field("Minimum trades each", min), r.go, r.status);
    host().replaceChildren(card("A/B experiment (historical simulation, same window and costs)", w.f, r.out));
  },

  async regimes() {
    const w = windowForm(false);
    const m = msg();
    const out = el("div");
    const go = button("Classify", "btn", async () => {
      try {
        const v = await post("/api/regimes/classify", w.body());
        out.replaceChildren(el("p", "st-about", v.method), obj({ current: v.current, distribution: v.distribution, source: v.source, rule_version: v.rule_version }), rows(v.series.slice(-60).reverse(), 5));
        say(m, "");
      } catch (e) {
        say(m, e.message, "err");
      }
    });
    w.f.append(go, m);
    host().replaceChildren(card("Market regime (rules-v1)", w.f, out));
  },

  async researcher() {
    const w = windowForm(false);
    const tpl = el("select");
    tpl.multiple = true;
    tpl.size = 6;
    for (const t of templates.filter((t) => t.spec.kind === "signal")) {
      const o = el("option", null, t.name);
      o.value = t.id;
      tpl.append(o);
    }
    const objective = select([{ value: "return" }, { value: "sharpe" }, { value: "profit_factor" }], "return");
    const r = runner("/api/research/runs", () => ({ ...w.body(), templates: [...tpl.selectedOptions].map((o) => o.value), objective: objective.value }), (rep) => (setTimeout(load, 0), obj(rep)));
    w.f.append(field("Templates (up to 8; none = first 8)", tpl), field("Objective", objective), r.go, r.status);
    const runs = (await api("/api/research/runs")).runs;
    const list = el("div");
    for (const run of runs) {
      const c = el("div", "card st-card");
      const m = msg();
      const decide = (d) =>
        button(d === "approve" ? "Approve: publish as a strategy version" : "Reject", d === "approve" ? "btn" : "btn ghost", async () => {
          try {
            const res = await post(`/api/research/runs/${run.id}/review`, { decision: d });
            say(m, res.note || res.review_status, "ok");
            setTimeout(load, 800);
          } catch (e) {
            say(m, e.message, "err");
          }
        });
      c.append(el("h4", null, `Run #${run.id} · ${run.request.symbol} ${run.request.interval} · ${run.review_status}`), el("p", "st-about", run.report?.conclusion || ""));
      if (run.report?.candidate) c.append(obj({ candidate: { template: run.report.candidate.template, changes: run.report.candidate.changes, in_sample: run.report.candidate.in_sample, out_of_sample: run.report.candidate.out_of_sample }, robustness_score: run.report.robustness?.score }));
      if (run.review_status === "pending") c.append(decide("approve"), decide("reject"));
      c.append(m);
      list.append(c);
    }
    host().replaceChildren(
      card("Autonomous quant researcher (bounded)", el("p", "st-about", "Tries template variants in-sample, re-tests the best out-of-sample and scores the winner. It cannot place orders, change risk limits or read credentials; a candidate is only a proposal until you approve it, and approval creates a strategy version, not a running bot."), w.f, r.out),
      list,
    );
  },

  async news() {
    const sym = input("", { placeholder: "filter by symbol" });
    const out = el("div");
    const paint = async () => out.replaceChildren(rows((await api(`/api/news${sym.value ? `?symbol=${encodeURIComponent(sym.value.trim())}` : ""}`)).items, 10, ["published_utc", "label", "publisher", "title", "symbols", "sentiment", "confidence", "copies", "url"]));
    sym.addEventListener("change", paint);
    const m = msg();
    const imp = el("textarea");
    imp.rows = 4;
    imp.placeholder = '[{"id":"x1","title":"…","published_utc":"2026-11-05T10:00:00Z","symbols":["BTCUSDT"],"publisher":"…","url":"https://…"}]';
    const doImport = button("Import news (JSON list)", "btn ghost", async () => {
      try {
        const r = await post("/api/news/import", { items: JSON.parse(imp.value) });
        say(m, `Added ${r.added}, duplicates ${r.duplicates}, rejected ${r.rejected.length}.`, "ok");
        paint();
      } catch (e) {
        say(m, e.message, "err");
      }
    });
    const sample = button("Load sample news (sample data only)", "btn ghost sm", async () => {
      try {
        await post("/api/news/sample", {});
        paint();
      } catch (e) {
        say(m, e.message, "err");
      }
    });
    const ev = await api("/api/econ/events");
    const csv = el("textarea");
    csv.rows = 3;
    csv.placeholder = "id,name,region,scheduled_utc,importance,prior,forecast,actual";
    const m2 = msg();
    const evImport = button("Import calendar (CSV)", "btn ghost", async () => {
      try {
        const r = await post("/api/econ/import", { csv: csv.value });
        say(m2, `Added ${r.added}, revised/delayed ${r.revised_or_delayed}, released ${r.released}, rejected ${r.rejected.length}.`, "ok");
        setTimeout(load, 600);
      } catch (e) {
        say(m2, e.message, "err");
      }
    });
    host().replaceChildren(
      card("News", el("p", "st-about", "No live news provider is configured: items come from your imports (or made-up sample items, labelled). Sentiment is a word-list heuristic with a confidence, not advice."), field("Symbol", sym), out, imp, doImport, sample, m),
      card("Economic calendar", rows(ev.events, 10, ["scheduled_utc", "name", "region", "importance", "prior", "forecast", "actual", "status", "provider"]), csv, evImport, m2),
    );
    paint();
  },

  async options() {
    const [spot, days, vol, rate, width, mult] = [input("100", { type: "number", step: "any" }), input("30", { type: "number" }), input("20", { type: "number", step: "any" }), input("5", { type: "number", step: "any" }), input("5", { type: "number", step: "any" }), input("1", { type: "number" })];
    const tpl = select(["long_call", "long_put", "covered_call", "protective_put", "bull_call_spread", "bear_put_spread", "long_straddle", "long_strangle", "iron_condor"].map((v) => ({ value: v, label: v.replace(/_/g, " ") })), "bull_call_spread");
    const m = msg();
    const out = el("div");
    const go = button("Analyse", "btn", async () => {
      try {
        const a = await post("/api/options/strategy", { spot: Number(spot.value), days: Number(days.value), vol_pct: Number(vol.value), rate_pct: Number(rate.value), template: tpl.value, width: Number(width.value), multiplier: Number(mult.value) });
        const chart = el("div", "lab-chart");
        drawPayoff(chart, a.curve);
        out.replaceChildren(el("p", "bdg warn", a.label), obj({ max_profit: a.max_profit, max_loss: a.max_loss, breakevens: a.breakevens, net_premium: a.net_premium, greeks: a.greeks }), chart, rows(a.legs, 4), el("p", "st-about", a.assumptions));
        say(m, "");
      } catch (e) {
        say(m, e.message, "err");
      }
    });
    const f = el("div", "st-form");
    f.append(field("Template", tpl), field("Underlying price", spot), field("Days to expiry", days), field("Volatility %", vol), field("Rate %", rate), field("Strike width", width), field("Multiplier", mult), go, m);
    host().replaceChildren(card("Options strategy lab (theoretical, Black-Scholes)", el("p", "st-about", "No options data or trading: your inputs, a model and its assumptions."), f, out));
  },

  async explain() {
    const tid = input("", { type: "number", placeholder: "closed trade id (Journal)" });
    const m = msg();
    const out = el("div");
    const show = button("Show the evidence", "btn", async () => {
      try {
        const f = await api(`/api/explain/trades/${tid.value}`);
        out.replaceChildren(el("p", "st-about", f.label), rows(f.facts, 2, ["fact", "source"]), el("h5", null, "Missing evidence"), el("p", null, f.missing_evidence.join(" · ") || "none"));
        say(m, "");
      } catch (e) {
        say(m, e.message, "err");
      }
    });
    const ai = button("Ask AI to interpret (labelled)", "btn ghost", async () => {
      try {
        const r = await post(`/api/explain/trades/${tid.value}/ai`, {});
        out.append(el("h5", null, r.ai.label), el("p", null, r.ai.interpretation), el("p", r.ai.numbers_verified ? "st-msg ok" : "st-msg err", r.ai.numbers_verified ? "Every number matches the facts." : `Numbers not in the facts: ${r.ai.unverified_numbers.join(", ")}`));
      } catch (e) {
        say(m, e.message, "err");
      }
    });
    const f = el("div", "st-form");
    f.append(field("Trade", tid), show, ai, m);
    host().replaceChildren(card("Trade explainer", f, out));
  },
};

/** Payoff at expiry and model value today, drawn on a canvas (no chart library needed). */
function drawPayoff(host, curve) {
  const c = el("canvas");
  const w = Math.min(760, host.clientWidth || 760);
  c.width = w * devicePixelRatio;
  c.height = 240 * devicePixelRatio;
  c.style.width = "100%";
  c.style.maxWidth = w + "px";
  c.style.height = "240px";
  host.append(c);
  const g = c.getContext("2d");
  g.scale(devicePixelRatio, devicePixelRatio);
  const xs = curve.map((p) => p.price);
  const ys = curve.flatMap((p) => [p.expiry, p.now]);
  const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys, 0), Math.max(...ys, 0)];
  const X = (x) => 40 + ((x - x0) / (x1 - x0 || 1)) * (w - 50);
  const Y = (y) => 220 - ((y - y0) / (y1 - y0 || 1)) * 200;
  g.strokeStyle = "#cbd5e1";
  g.beginPath();
  g.moveTo(40, Y(0));
  g.lineTo(w - 10, Y(0));
  g.stroke();
  for (const [key, color] of [["now", "#94a3b8"], ["expiry", "#4f46e5"]]) {
    g.strokeStyle = color;
    g.lineWidth = key === "expiry" ? 2 : 1;
    g.beginPath();
    curve.forEach((p, i) => (i ? g.lineTo(X(p.price), Y(p[key])) : g.moveTo(X(p.price), Y(p[key]))));
    g.stroke();
  }
  g.fillStyle = "#64748b";
  g.font = "11px system-ui";
  g.fillText(`${x0.toFixed(2)}`, 40, 236);
  g.fillText(`${x1.toFixed(2)}`, w - 60, 236);
  g.fillText(`${y1.toFixed(2)}`, 2, 14);
  g.fillText(`${y0.toFixed(2)}`, 2, 222);
  g.fillText("— at expiry   — model value today", w - 230, 14);
}
