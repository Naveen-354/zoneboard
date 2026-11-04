// Bot Studio: Explore Bots, My Bots, Create Bot, Strategy Library, Backtesting, Bot Performance.
// Loaded on first visit (nothing on the page-load path). DOM via createElement/textContent only.
import { $, el, api, button, badge, avatar } from "./ui.js";
import { CATALOG, CHECKED } from "./catalog.js";
import * as builder from "./builder.js";
import * as backtest from "./backtest.js";
import * as mybots from "./mybots.js";
import * as replay from "./replay.js";
import * as approvals from "./approvals.js";
import * as ai from "./ai.js";

const TABS = [
  ["explore", "Explore Bots"],
  ["mine", "My Bots"],
  ["create", "Create Bot"],
  ["library", "Strategy Library"],
  ["backtest", "Backtesting"],
  ["replay", "Market Replay"],
  ["ai", "AI Research"],
  ["approvals", "Approval Center"],
  ["perf", "Bot Performance"],
];
const KEY = "zoneboard.studio.tab";
const GROUP_NAMES = { trend: "Trend following", momentum: "Momentum", mean_reversion: "Mean reversion", volume: "Volume", advanced: "Advanced strategies" };

const ctx = { library: null, bots: [], accounts: [], symbols: [], catalog: CATALOG, risk: null };
let tab = "explore";
let wired = false;
let timer = 0;

function savedTab() {
  try {
    const t = localStorage.getItem(KEY);
    return TABS.some(([id]) => id === t) ? t : null;
  } catch {
    return null;
  }
}

export function wire() {
  if (wired) return;
  wired = true;
  const nav = $("#stTabs");
  for (const [id, label] of TABS) {
    const b = el("button", null, label);
    b.type = "button";
    b.dataset.t = id;
    b.setAttribute("role", "tab");
    b.addEventListener("click", () => go(id));
    nav.append(b);
  }
  tab = savedTab() || "explore";
  Object.assign(ctx, {
    go,
    reload: () => refresh(true),
    edit: (b) => {
      builder.load(
        {
          name: b.name, description: b.description, avatar: b.avatar, market: b.market, mode: b.mode, account_id: b.account_id, symbol: b.symbol, symbol2: b.symbol2 || "",
          interval: b.interval, spec: b.spec, template: b.template, source: b.source, allocation: b.allocation, max_daily_loss: b.max_daily_loss || "", max_open_positions: b.max_open_positions, slippage_pct: b.slippage_pct,
        },
        b.id,
      );
      go("create");
    },
    openPerf: (id) => {
      mybots.openPerf(id);
      go("perf");
    },
    openBacktest: (id) => {
      backtest.use({ kind: "bot", id });
      go("backtest");
    },
    goBacktest: (draft) => {
      backtest.use({ kind: "draft", draft });
      go("backtest");
    },
    applyToBuilder: (draft) => {
      const editing = draft.id;
      builder.load({ ...draft, template: null }, editing || null);
      go("create");
    },
    currentDraft: () => builder.current(),
    blankDraft: () => builder.blank(),
    onSaved: async (bot) => {
      await refresh(false);
      mybots.openPerf(bot.id);
    },
  });
  builder.init(ctx);
  backtest.init(ctx);
  mybots.init(ctx);
  replay.init(ctx);
  ai.init(ctx);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && !$("#botsView").hidden) refresh(false);
  });
}

/** Called each time the page is shown. */
export async function load() {
  const body = $("#stBody");
  if (!ctx.library) {
    body.replaceChildren(el("p", "st-about", "Loading Bot Studio…"));
    try {
      const [lib, cfg] = await Promise.all([api("/api/strategies/library"), api("/api/config")]);
      ctx.library = lib;
      ctx.symbols = cfg.crypto?.symbols || [];
      ctx.sample = !!cfg.sample;
    } catch (e) {
      body.replaceChildren(el("p", "st-msg err", `Can't load Bot Studio: ${e.message}`));
      return;
    }
  }
  await refresh(false);
  go(tab);
  clearInterval(timer);
  // keep the dashboard fresh while it is on screen; the runner itself lives in the server
  timer = setInterval(() => {
    if (!$("#botsView").hidden && !document.hidden && (tab === "mine" || tab === "perf")) refresh(true);
  }, 15000);
}

async function refresh(repaint) {
  try {
    const [b, a, r] = await Promise.all([api("/api/bots"), api("/api/accounts"), api("/api/risk")]);
    ctx.bots = b.bots;
    ctx.accounts = a.accounts.filter((x) => !x.archived);
    ctx.risk = r;
  } catch (e) {
    $("#stNote").textContent = `Can't refresh: ${e.message}`;
    return;
  }
  $("#stNote").textContent = ctx.sample ? "Sample data is on: bots cannot start (made-up prices). Backtests work and are labelled SAMPLE." : "";
  if (repaint && (tab === "mine" || tab === "perf")) paint();
}

function go(id) {
  tab = id;
  try {
    localStorage.setItem(KEY, id);
  } catch {
    /* storage unavailable */
  }
  for (const b of $("#stTabs").children) {
    const on = b.dataset.t === id;
    b.classList.toggle("on", on);
    b.setAttribute("aria-selected", on ? "true" : "false");
  }
  paint();
}

function paint() {
  const body = $("#stBody");
  if (!ctx.library) return;
  if (tab !== "replay") replay.stopReplay();
  body.dataset.tab = tab;
  if (tab === "explore") body.replaceChildren(explore());
  else if (tab === "mine") mybots.renderMine(body, ctx.risk);
  else if (tab === "create") builder.render(body);
  else if (tab === "library") body.replaceChildren(library());
  else if (tab === "backtest") backtest.render(body);
  else if (tab === "replay") replay.render(body);
  else if (tab === "approvals") approvals.render(body, ctx);
  else if (tab === "ai") ai.render(body);
  else if (tab === "perf") mybots.renderPerf(body);
}

// ---------------------------------------------------------------- Explore Bots

let filter = "all";

function explore() {
  const wrap = el("div", "st-explore");
  const head = el("div", "st-build-head");
  head.append(el("b", null, "Well-known trading bots"), el("span", "jsub", `Described from their own documentation, checked ${CHECKED}. ZoneBoard does not run or connect to any of them.`));
  const f = el("div", "seg sm");
  for (const [v, label] of [
    ["all", "All"],
    ["open", "Open source"],
    ["commercial", "Commercial"],
  ]) {
    const b = el("button", v === filter ? "on" : null, label);
    b.type = "button";
    b.addEventListener("click", () => {
      filter = v;
      paint();
    });
    f.append(b);
  }
  head.append(f);
  const grid = el("div", "cat-grid");
  for (const c of CATALOG.filter((x) => filter === "all" || x.kind === filter)) grid.append(catCard(c));
  wrap.append(head, grid);
  return wrap;
}

function list(title, items) {
  const d = el("div", "cat-sec");
  d.append(el("h5", null, title));
  const ul = el("ul");
  for (const i of items) ul.append(el("li", null, i));
  d.append(ul);
  return d;
}

function link(href, text) {
  const a = el("a", null, text);
  a.href = href;
  a.target = "_blank";
  a.rel = "noopener noreferrer";
  return a;
}

function catCard(c) {
  const card = el("article", "card cat-card");
  const hd = el("div", "bot-h");
  const t = el("div", "bot-t");
  t.append(el("b", null, c.name), el("small", null, `${c.language} · ${c.licence}`));
  hd.append(avatar({ glyph: c.name.slice(0, 2), color: c.kind === "open" ? "#0b9f6e" : "#b45f06" }, c.name), t, badge(c.kind === "open" ? "OPEN SOURCE" : "COMMERCIAL", c.kind === "open" ? "ok" : "warn"));
  const det = el("details");
  det.append(el("summary", null, "Details: markets, strategies, features, limits, configuration"));
  det.append(
    el("p", "cat-p", c.approach),
    list("Markets", c.markets),
    el("p", "cat-p", c.exchanges),
    list("Strategies and indicators", c.strategies),
    list("Features", c.features),
    list("Limitations", c.limitations),
    list("Configuration", c.config),
  );
  const links = el("div", "cat-links");
  links.append(link(c.docs, "Official site / docs ↗"));
  if (c.repo) links.append(link(c.repo, "Source repository ↗"));
  else links.append(el("small", null, "Closed source: no public repository"));
  const tpl = ctx.library.templates.find((x) => x.id === c.template);
  const how = el("p", "cat-note");
  how.append(el("b", null, "Not integrated. "), document.createTextNode(`Add to My Bots starts a ZoneBoard bot from the ${tpl?.name || c.template} template: ${c.templateWhy}`));
  const add = button("Add to My Bots", "btn", () => {
    const d = builder.blank();
    builder.load({
      ...d,
      name: `${c.name}-style ${tpl?.name || "bot"}`.slice(0, 60),
      description: `ZoneBoard template inspired by ${c.name}'s documented approach. Not an integration: ${c.name} is not involved.`,
      avatar: { glyph: c.name.slice(0, 2), color: c.kind === "open" ? "#0b9f6e" : "#b45f06" },
      spec: JSON.parse(JSON.stringify(tpl.spec)),
      template: tpl.id,
      source: c.id,
      max_open_positions: tpl.spec.kind === "grid" ? Math.max(10, (tpl.spec.grid.levels || 10) - 1) : d.max_open_positions,
      symbol2: tpl.spec.kind === "pairs" ? "ETHUSDT" : "",
    });
    go("create");
  });
  card.append(hd, el("p", "cat-tag", c.tagline), det, links, how, add);
  return card;
}

// ---------------------------------------------------------------- Strategy Library

function library() {
  const wrap = el("div", "st-lib");
  wrap.append(el("p", "st-about", ctx.library.execution));
  const h = el("h3", "lib-h", "Strategies");
  wrap.append(h);
  for (const [g, name] of Object.entries(GROUP_NAMES)) {
    const ts = ctx.library.templates.filter((t) => t.group === g);
    if (!ts.length) continue;
    wrap.append(el("h4", "page-h", name));
    const grid = el("div", "lib-grid");
    for (const t of ts) {
      const c = el("article", "card lib-card");
      c.append(el("b", null, t.name), el("small", "bdg info", t.spec.kind), el("p", null, t.about));
      const inds = (t.spec.indicators || []).map((i) => i.type).join(", ");
      if (inds) c.append(el("small", "st-about", `Uses: ${inds}`));
      const row = el("div", "bot-ctl");
      row.append(
        button("Use in Create Bot", "btn sm", () => {
          builder.load({ ...builder.blank(), name: t.name, spec: JSON.parse(JSON.stringify(t.spec)), template: t.id, symbol2: t.spec.kind === "pairs" ? "ETHUSDT" : "", max_open_positions: t.spec.kind === "grid" ? Math.max(10, (t.spec.grid.levels || 10) - 1) : 10 });
          go("create");
        }),
        button("Backtest", "btn ghost sm", () => {
          const d = { ...builder.blank(), name: t.name, spec: JSON.parse(JSON.stringify(t.spec)), template: t.id, symbol2: t.spec.kind === "pairs" ? "ETHUSDT" : "" };
          ctx.goBacktest(d);
        }),
      );
      c.append(row);
      grid.append(c);
    }
    wrap.append(grid);
  }
  wrap.append(el("h3", "lib-h", "Indicators"), el("p", "st-about", "Every indicator is computed on closed candles only; the value at a candle never uses later candles (tested for each one). Combine any of them with AND / OR rules in Create Bot."));
  for (const [g, name] of Object.entries(GROUP_NAMES)) {
    const is = ctx.library.indicators.filter((i) => i.group === g);
    if (!is.length) continue;
    wrap.append(el("h4", "page-h", name));
    const grid = el("div", "lib-grid");
    for (const i of is) {
      const c = el("article", "card lib-card");
      c.append(el("b", null, i.name), el("p", null, i.about));
      c.append(el("small", "st-about", `Outputs: ${i.outputs.join(", ")}`));
      if (i.params.length) c.append(el("small", "st-about", `Parameters: ${i.params.map((p) => `${p.name} ${p.default}`).join(", ")}`));
      grid.append(c);
    }
    wrap.append(grid);
  }
  return wrap;
}
