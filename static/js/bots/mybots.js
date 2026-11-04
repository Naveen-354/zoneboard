// My Bots (the fleet dashboard with controls) and Bot Performance (one bot in depth: equity, positions, trades,
// execution log, account risk limits and the live-readiness checklist). Every number comes from the server,
// computed from the bot's recorded fills on the demo account.
import { el, api, post, put, del, button, say, stat, pct, num, money, short, pnl, table, row, td, badge, statusBadge, avatar, whenIso, when, fmtAuto, lineChart, select, input, field } from "./ui.js";

let ctx = null;
let perfHost = null;
let perfId = null;
let dispose = () => {};
let logBefore = null;

export function init(c) {
  ctx = c;
}

const cur = (b) => (b.market === "crypto" ? "USDT" : "INR");

// ---------------------------------------------------------------- My Bots

export function renderMine(host, risk) {
  host.replaceChildren(fleetHead(risk), healthLine(), killCard(risk), fleet());
}

function fleetHead(risk) {
  const bots = ctx.bots;
  const by = (s) => bots.filter((b) => b.status === s).length;
  const sums = {};
  for (const b of bots) {
    const c = cur(b);
    sums[c] ||= { alloc: 0, pnl: 0, open: 0 };
    sums[c].alloc += Number(b.allocation);
    sums[c].pnl += Number(b.performance.net_pnl);
    sums[c].open += b.performance.open_positions.length;
  }
  const s = el("div", "stats");
  s.append(stat("Running", String(by("running")), by("running") ? "up" : ""), stat("Paused", String(by("paused"))), stat("Stopped", String(by("stopped"))));
  for (const [c, x] of Object.entries(sums)) {
    s.append(stat(`Allocated ${c}`, money(String(Math.round(x.alloc * 100) / 100))), stat(`Net P&L ${c}`, short(x.pnl), x.pnl > 0 ? "up" : x.pnl < 0 ? "dn" : ""), stat(`Open positions (${c})`, String(x.open)));
  }
  const wrap = el("div");
  const note = el("p", "no-money", "Bots trade the demo account only: VIRTUAL funds at real prices. This app holds no money.");
  wrap.append(note, s);
  if (risk?.kill_switch) wrap.prepend(el("div", "st-live-note", "Kill switch is ON: no bot or ticket order can open a position until you release it."));
  return wrap;
}

function healthLine() {
  const p = el("p", "st-about", "Runner: checking…");
  api("/api/system/health")
    .then((h) => {
      const r = h.runner;
      p.textContent = `Runner ${r.healthy ? "healthy" : r.heartbeat_utc ? "NOT RESPONDING" : "not started yet"}${r.age_s != null ? ` (last beat ${r.age_s} s ago)` : ""} · ${h.bots.running} running, ${h.bots.paused} paused · ${h.unreconciled_broker_orders} broker order(s) awaiting reconciliation`;
      p.className = "st-about" + (r.heartbeat_utc && !r.healthy ? " dn" : "");
    })
    .catch(() => (p.textContent = "Runner status unavailable."));
  return p;
}

function killCard(risk) {
  const c = el("section", "card st-card st-kill");
  const on = !!risk?.kill_switch;
  const msg = el("p", "st-msg");
  const b = button(on ? "Release kill switch" : "Kill switch: stop all bots and close their positions", on ? "btn ghost" : "btn danger", async () => {
    if (!on && !confirm("Stop every bot, close all bot positions at the latest price and block new orders?")) return;
    b.disabled = true;
    say(msg, on ? "Releasing…" : "Stopping every bot…");
    try {
      const r = await post("/api/risk/kill", { engaged: !on });
      say(msg, on ? "Released. Bots stay stopped until you start them." : `Engaged. ${r.stopped.length} bot(s) stopped.`, "ok");
      await ctx.reload();
    } catch (e) {
      say(msg, e.message, "err");
      b.disabled = false;
    }
  });
  const live = el("div", "st-live-chip");
  live.append(badge(risk?.armed ? `LIVE ARMED: ${risk.armed.account_id}` : "LIVE: DISARMED", risk?.armed ? "warn" : "off"), el("small", null, "Arming is runtime only and resets on every restart. Arm it under Bot Performance → Live readiness."));
  c.append(b, live, msg);
  return c;
}

function fleet() {
  const g = el("div", "bot-grid");
  if (!ctx.bots.length) {
    const e = el("div", "card st-card empty-j");
    e.append(el("p", null, "No bots yet."), button("Create a bot", "btn", () => ctx.go("create")), button("Explore famous bots", "btn ghost", () => ctx.go("explore")));
    g.append(e);
    return g;
  }
  for (const b of ctx.bots) g.append(botCard(b));
  return g;
}

function botCard(b) {
  const p = b.performance;
  const c = el("article", "card bot-card");
  const hd = el("div", "bot-h");
  const t = el("div", "bot-t");
  t.append(el("b", null, b.name), el("small", null, `${b.symbol}${b.symbol2 ? " / " + b.symbol2 : ""} · ${b.interval} · ${b.market === "crypto" ? "Binance" : "NSE"} · ${b.spec?.kind || "signal"}`));
  const env = b.mode === "demo" ? badge("DEMO · VIRTUAL", "info") : badge(b.account_id.startsWith("bn-live") ? "LIVE · REAL MONEY" : "BINANCE TESTNET", b.account_id.startsWith("bn-live") ? "warn" : "ok");
  hd.append(avatar(b.avatar, b.name), t, statusBadge(b.status), env);
  const ver = b.strategy_version ? `strategy v${b.strategy_version.version} · ${b.strategy_version.hash.slice(0, 8)}` : "no version";
  t.append(el("small", null, `stage ${b.stage} · ${ver}`));
  const s = el("div", "bot-stats");
  const kv = (k, v, cls) => {
    const d = el("div");
    d.append(el("small", null, k));
    const x = el("b", cls);
    if (v instanceof Node) x.append(v);
    else x.textContent = v;
    d.append(x);
    return d;
  };
  s.append(
    kv("Net P&L", pnl(el("span"), p.net_pnl, true)),
    kv("Return", pct(p.return_pct)),
    kv("Win rate", p.win_rate == null ? "–" : pct(p.win_rate)),
    kv("Profit factor", p.profit_factor == null ? "–" : num(p.profit_factor)),
    kv("Trades", String(p.trades)),
    kv("Max DD", pct(p.max_drawdown_pct)),
    kv("Allocated", `${money(b.allocation)} ${cur(b)}`),
    kv("Positions", String(p.open_positions.length)),
  );
  const info = el("p", "bot-info");
  if (b.last_error) info.append(el("span", "dn", `Last error: ${b.last_error}`));
  else if (b.status !== "stopped") info.textContent = `Last price ${b.last_price == null ? "–" : fmtAuto(b.last_price)} at ${whenIso(b.last_price_utc)}${b.pending_orders ? ` · ${b.pending_orders} order(s) waiting for the next open` : ""}${b.halted ? " · halted by its own stop" : ""}`;
  else info.textContent = "Stopped.";
  const msg = el("p", "st-msg");
  const act = (label, cls, fn, title) =>
    button(label, cls, async (e) => {
      e.target.disabled = true;
      say(msg, "…");
      try {
        await fn();
        say(msg, "");
        await ctx.reload();
      } catch (err) {
        say(msg, err.message, "err");
        e.target.disabled = false;
      }
    }, title);
  const ctl = el("div", "bot-ctl");
  if (b.status !== "running") ctl.append(act(b.status === "paused" ? "Resume" : "Start", "btn sm", () => post(`/api/bots/${b.id}/start`), "Trade the demo account from the next candle close"));
  if (b.status === "running") ctl.append(act("Pause", "btn ghost sm", () => post(`/api/bots/${b.id}/pause`), "No new entries; open positions are still managed"));
  if (b.status !== "stopped")
    ctl.append(
      act("Stop", "btn ghost sm", async () => {
        let policy = "keep_positions";
        if (p.open_positions.length) {
          const close = confirm(`Stop ${b.name}.\n\nOK: also CLOSE its ${p.open_positions.length} position(s) at the market now.\nCancel: stop and KEEP the positions open (close them yourself later).`);
          policy = close ? "close_positions" : "keep_positions";
        }
        await post(`/api/bots/${b.id}/stop`, { policy });
      }, "Stop the bot; you choose whether its positions close or stay open"),
    );
  ctl.append(
    button("Details", "btn ghost sm", () => ctx.openPerf(b.id)),
    button("Backtest", "btn ghost sm", () => ctx.openBacktest(b.id)),
    button("Edit", "btn ghost sm", () => ctx.edit(b)),
    act("Duplicate", "btn ghost sm", () => post(`/api/bots/${b.id}/duplicate`)),
    act("Delete", "btn ghost sm danger-t", async () => {
      if (!confirm(`Delete ${b.name}? Its trades stay in the Journal.`)) throw new Error("cancelled");
      await del(`/api/bots/${b.id}`);
    }, "Stopped bots with no open positions only"),
  );
  c.append(hd, s, info, ctl, msg);
  return c;
}

// ---------------------------------------------------------------- Bot Performance

export function openPerf(id) {
  perfId = id;
  logBefore = null;
}

export async function renderPerf(host) {
  perfHost = host;
  dispose();
  if (!ctx.bots.length) {
    host.replaceChildren(el("div", "card st-card empty-j", "No bots yet. Create one first."));
    return;
  }
  if (!ctx.bots.some((b) => b.id === perfId)) perfId = ctx.bots[0].id;
  const pick = select(ctx.bots.map((b) => ({ value: String(b.id), label: `${b.name} · ${b.symbol} ${b.interval}` })), String(perfId), (v) => {
    perfId = Number(v);
    logBefore = null;
    renderPerf(host);
  });
  const top = el("div", "st-build-head");
  top.append(field("Bot", pick));
  host.replaceChildren(top, el("p", "st-about", "Loading…"));
  let b, logs, ready, limits;
  try {
    [b, logs, ready] = await Promise.all([api(`/api/bots/${perfId}`), api(`/api/bots/${perfId}/logs?limit=100`), api(`/api/bots/${perfId}/live`)]);
    limits = await api(`/api/risk/limits/${encodeURIComponent(b.account_id)}`);
  } catch (e) {
    host.replaceChildren(top, el("p", "st-msg err", e.message));
    return;
  }
  const p = b.performance;
  const stats = el("div", "stats");
  stats.append(
    stat("Status", b.status.toUpperCase(), b.status === "running" ? "up" : ""),
    stat(`Capital (${cur(b)})`, short(p.capital), "", `Exactly ${money(p.capital)}`),
    stat("Net P&L", pnl(el("span"), p.net_pnl, true), "", `Exactly ${money(p.net_pnl)}`),
    stat("Unrealised", short(p.unrealised), p.unrealised > 0 ? "up" : p.unrealised < 0 ? "dn" : "", `Open positions at the latest price (${whenIso(p.price_as_of)})`),
    stat("Return", pct(p.return_pct)),
    stat("Trades", String(p.trades)),
    stat("Win rate", p.win_rate == null ? "–" : pct(p.win_rate)),
    stat("Profit factor", p.profit_factor == null ? "–" : num(p.profit_factor)),
    stat("Max drawdown", pct(p.max_drawdown_pct), "", `On closed-trade equity: ${money(p.max_drawdown)}`),
    stat("Fees", short(p.fees)),
  );
  const eq = el("section", "card st-card");
  eq.append(el("h4", null, "Equity from closed trades"));
  const ech = el("div", "chart-box");
  eq.append(ech);
  const points = p.equity.map((e) => [Math.floor(Date.parse(e.t) / 1000), Number(e.equity)]).filter((x) => Number.isFinite(x[0]));
  host.replaceChildren(top, el("p", "no-money", "Demo account: VIRTUAL funds, real prices."), stats, eq, positions(b), trades(p), logView(b, logs), riskLimits(b, limits), readiness(b, ready));
  dispose = lineChart(ech, points, { baseline: Number(p.allocated) });
}

function positions(b) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, `Current positions (${b.performance.open_positions.length})`));
  const rows = b.performance.open_positions.map((x) => {
    const u = td(x.unrealised == null ? "–" : fmtAuto(x.unrealised), "num " + (x.unrealised > 0 ? "up" : x.unrealised < 0 ? "dn" : ""));
    return row([x.symbol, x.side, td(fmtAuto(x.qty), "num"), td(fmtAuto(x.entry), "num"), td(x.last == null ? "–" : fmtAuto(x.last), "num"), u, td(x.stop == null ? "–" : fmtAuto(x.stop), "num"), td(x.target == null ? "–" : fmtAuto(x.target), "num"), x.tag || "", when(x.opened_bar)]);
  });
  c.append(rows.length ? table(["Symbol", "Side", { label: "Qty", cls: "num" }, { label: "Entry", cls: "num" }, { label: "Last", cls: "num" }, { label: "Unrealised", cls: "num" }, { label: "Stop", cls: "num" }, { label: "Target", cls: "num" }, "Tag", "Opened bar"], rows) : el("p", "st-about", "No open positions."));
  return c;
}

function trades(p) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, `Closed trades (${p.trades})`));
  const rows = p.recent_trades.map((t) => {
    const x = td(null, "num");
    pnl(x, t.pnl);
    return row([whenIso(t.closed_utc) || t.date, t.symbol, t.side, td(fmtAuto(t.qty), "num"), td(fmtAuto(t.entry), "num"), td(fmtAuto(t.exit), "num"), td(fmtAuto(t.fees), "num"), x, td(t.notes, "notes")]);
  });
  c.append(rows.length ? table(["Closed", "Symbol", "Side", { label: "Qty", cls: "num" }, { label: "Entry", cls: "num" }, { label: "Exit", cls: "num" }, { label: "Fees", cls: "num" }, { label: "P&L", cls: "num" }, "Notes"], rows) : el("p", "st-about", "No closed trades yet. They also appear in the Journal (demo account)."));
  return c;
}

function logView(b, data) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, "Execution log"));
  const orders = data.orders.map((o) => row([whenIso(o.ts), o.action, o.side, o.symbol, td(fmtAuto(o.qty), "num"), td(fmtAuto(o.price), "num"), td(o.status.toUpperCase(), o.status === "filled" ? "up" : "dn"), td(o.reason, "notes")]));
  c.append(el("small", "st-about", "Orders (filled and refused, with the reason)"));
  c.append(orders.length ? table(["Time", "Action", "Side", "Symbol", { label: "Qty", cls: "num" }, { label: "Price", cls: "num" }, "Status", "Reason"], orders) : el("p", "st-about", "No orders yet."));
  const list = el("ol", "log-list");
  const add = (logs) => {
    for (const l of logs) {
      const li = el("li", "lv-" + l.level);
      li.append(el("time", null, whenIso(l.ts)), el("b", null, l.level), el("span", null, l.message));
      list.append(li);
    }
  };
  add(data.logs);
  logBefore = data.next_before;
  const more = button("Older", "btn ghost sm", async () => {
    if (!logBefore) return;
    const d = await api(`/api/bots/${b.id}/logs?limit=100&before=${logBefore}`);
    add(d.logs);
    logBefore = d.next_before;
    more.hidden = !logBefore;
  });
  more.hidden = !logBefore;
  c.append(el("small", "st-about", "Bot log"), list, more);
  return c;
}

function riskLimits(b, data) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, `Risk: account limits for ${b.account_id}`), el("p", "st-about", "These bind every order on the account: manual ticket orders and every bot. Empty = no limit. Daily loss counts realised P&L for the market's calendar day."));
  const g = el("div", "lim-grid");
  const inputs = {};
  const order = Object.entries(data.limits).sort(([a], [b2]) => (a === cur(b) ? -1 : b2 === cur(b) ? 1 : 0));
  for (const [curr, l] of order) {
    const box = el("div", "lim");
    box.append(el("b", null, `${curr} (VIRTUAL)`), el("small", null, `Realised today: ${money(l.realised_today)}`));
    inputs[curr] = {};
    for (const [k, label] of [
      ["max_order_notional", "Max order size"],
      ["max_exposure", "Max open exposure"],
      ["max_daily_loss", "Max daily loss"],
    ]) {
      const i = input(l[k] ?? "", { inputmode: "decimal", placeholder: "no limit" });
      inputs[curr][k] = i;
      box.append(field(label, i));
    }
    g.append(box);
  }
  const msg = el("p", "st-msg");
  const save = button("Save limits", "btn ghost", async () => {
    const body = {};
    for (const [curr, f] of Object.entries(inputs)) body[curr] = Object.fromEntries(Object.entries(f).map(([k, i]) => [k, i.value.trim() || null]));
    try {
      await put(`/api/risk/limits/${encodeURIComponent(b.account_id)}`, body);
      say(msg, "Saved. Every new order is checked against them.", "ok");
    } catch (e) {
      say(msg, e.message, "err");
    }
  });
  const bar = el("div", "st-actions");
  bar.append(save, msg);
  c.append(g, bar);
  return c;
}

function readiness(b, r) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, "Live readiness"), el("p", "st-about", r.summary));
  const list = el("ul", "gate-list");
  for (const x of r.checklist) {
    const li = el("li", x.ok ? "ok" : "no");
    li.append(el("i", null, x.ok ? "✓" : "✕"), el("b", null, x.label), el("small", null, x.detail));
    list.append(li);
  }
  const msg = el("p", "st-msg");
  const dep = button("Check live deployment", "btn ghost", async () => {
    try {
      const r = await post(`/api/bots/${b.id}/deploy-live`);
      say(msg, `Every gate passes: ${r.next}.`, "ok");
    } catch (e) {
      say(msg, `${e.message}. Nothing was sent anywhere.`, "err");
    }
  });
  dep.title = "Checks every gate";
  const phrase = input("", { placeholder: "ARM LIVE TRADING", "aria-label": "arming phrase" });
  const bar = el("div", "st-actions");
  bar.append(
    dep,
    button("Request approval", "btn ghost sm", async () => {
      try {
        await post(`/api/bots/${b.id}/approval`);
        say(msg, "Approval requested: decide it in the Approval Center.", "ok");
      } catch (e) {
        say(msg, e.message, "err");
      }
    }),
  );
  const arm = el("div", "st-actions");
  arm.append(
    phrase,
    button("Arm live trading for this bot's account", "btn ghost sm", async () => {
      try {
        await post("/api/live/arm", { account_id: b.account_id, confirm: phrase.value.trim() });
        say(msg, "Armed until you disarm or restart.", "ok");
      } catch (e) {
        say(msg, e.message, "err");
      }
    }),
    button("Disarm", "btn ghost sm", async () => {
      await post("/api/live/disarm");
      say(msg, "Disarmed.", "ok");
    }),
  );
  c.append(list, bar, arm, msg);
  return c;
}

export const currentPerf = () => perfId;
export { perfHost };
