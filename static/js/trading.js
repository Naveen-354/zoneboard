// Paper trading on the chart screen: an order ticket plus the open positions.
//
// Orders are SIMULATED. A market order fills at the price on screen, and closing a position logs the trade
// to the Journal. No broker is involved and no real money moves: this tool never places real orders.
// Positions live in the server's database, so they survive a reload.
import { $, el, fmt, fmtAuto, fmtQty, api } from "./util.js";
import { nseOpen } from "./hours.js";

const baseOf = (sym) => (sym.endsWith("USDT") ? sym.slice(0, -4) : sym);
/** An amount in the market's currency: ₹ goes in front, USDT after. */
const amount = (n, cur) => (cur === "₹" ? `₹${fmt(n, 2)}` : cur ? `${fmt(n, 2)} ${cur}` : fmt(n, 2));
/** A signed profit/loss in the market's currency. */
const money = (v, cur) => `${v < 0 ? "-" : "+"}${amount(Math.abs(v), cur)}`;
const today = () => new Date().toLocaleDateString("en-CA"); // YYYY-MM-DD, local time
const JSON_POST = { method: "POST", headers: { "Content-Type": "application/json" } };

/**
 * @param ctx        () => {market, mode, sym, feedState}: the current market object, "live"/"demo", the symbol on the chart
 * @param openSymbol (sym) => void: show that symbol's chart
 */
export function initTrading({ ctx, openSymbol }) {
  const S = { positions: [], side: "long", busy: false, fees: { crypto: 0.1, india: 0.05 }, rows: new Map(), chipsKey: "", flash: null, posFlash: null, loadedFor: "" };
  const qtyEl = $("#tkQty");
  const goEl = $("#tkGo");

  /**
   * Why an order can't be placed or closed right now, or null if it can.
   * Both accounts see the same real prices. Only the demo account can trade: the live account would need a
   * connected broker, and sample (made-up) data must never reach either account's journal.
   */
  function gate(c, q) {
    if (!c.market) return "Loading…";
    if (c.sample) return "Sample data is on: its prices are made up, so ordering is switched off.";
    if (c.mode === "live") return "Live trading isn't connected: this app places no real orders yet. Switch to Demo to practise on real prices.";
    if (c.market.id === "india" && !nseOpen()) return "NSE is closed (09:15 to 15:30 IST, Mon to Fri). Crypto trades 24/7: try a coin.";
    if (c.market.id === "crypto" && c.feedState !== "live") return "The price feed isn't live yet.";
    return q ? null : "Waiting for a price…";
  }

  function parseQty(c) {
    const v = parseFloat(qtyEl.value);
    if (!Number.isFinite(v) || v <= 0) return null;
    if (c.market?.id === "india" && !Number.isInteger(v)) return null; // NSE lots are whole
    return v;
  }

  const showFlash = (key, text, kind) => (S[key] = { text, kind, until: performance.now() + 7000 });

  // ------------------------------------------------------------------ ticket
  function buildChips(c) {
    const key = c.market ? c.market.id : "";
    if (key === S.chipsKey) return;
    S.chipsKey = key;
    const box = $("#tkChips");
    box.replaceChildren();
    if (!c.market) return;
    const sets = c.market.id === "crypto" ? [[100, "100"], [500, "500"], [1000, "1k"]] : [[10000, "₹10k"], [50000, "₹50k"], [100000, "₹1L"]];
    for (const [amount, label] of sets) {
      const b = el("button", null, label);
      b.type = "button";
      b.title = `Size the order to about ${label} ${c.market.id === "crypto" ? c.market.currency : ""}`.trim();
      b.addEventListener("click", () => {
        const cc = ctx();
        const q = cc.sym && cc.market ? cc.market.quote(cc.sym) : null;
        if (!q) return;
        const raw = amount / q.price;
        const qty = cc.market.id === "india" ? Math.floor(raw) : raw >= 1000 ? Math.floor(raw) : Number(raw.toPrecision(4));
        qtyEl.value = qty > 0 ? String(qty) : "";
        renderTicket();
      });
      box.append(b);
    }
    qtyEl.step = c.market.id === "india" ? "1" : "any";
  }

  function renderTicket() {
    const c = ctx();
    const sym = c.sym;
    const q = sym && c.market ? c.market.quote(sym) : null;
    buildChips(c);

    const off = c.sample || c.mode === "live"; // this account can't place orders at all (not just right now)
    const badge = $("#tkBadge");
    badge.textContent = c.sample ? "SAMPLE" : c.mode === "demo" ? "DEMO" : "LIVE";
    badge.classList.toggle("off", off);
    // NSE prices come from Yahoo, about 15 minutes late, until a live broker feed is connected
    $("#tkNote").textContent = c.market?.id === "india" && !off ? "This NSE price is about 15 minutes old, so a real fill would differ." : "";
    $("#tkSym").textContent = sym ? baseOf(sym) : "–";
    $("#tkPx").textContent = q ? fmt(q.price, q.dp) : "–";

    const qty = parseQty(c);
    const cur = c.market?.currency ?? "";
    const rate = S.fees[c.market?.id] ?? 0;
    $("#tkEst").textContent = qty && q && !off ? `Value ≈ ${amount(qty * q.price, cur)} · fee ≈ ${amount((qty * q.price * rate) / 100, cur)}` : "";
    $("#tkFoot").textContent = c.sample
      ? "Sample data: made-up prices. Nothing is traded or logged."
      : c.mode === "live"
        ? "Live account: real orders need a connected broker, which this app doesn't have yet. Nothing is sent anywhere."
        : `Demo account: a simulated order filled at the real price shown, logged in the demo journal. Fee ${rate}% per side is simulated too. No real money moves.`;

    qtyEl.disabled = off;
    for (const b of document.querySelectorAll("#tkChips button, #tkSide button")) b.disabled = off;

    const long = S.side === "long";
    goEl.className = "tk-go " + (long ? "buy" : "sell");
    goEl.textContent = !sym ? "Pick a symbol" : c.sample ? "Ordering is off" : c.mode === "live" ? "Live trading not connected" : long ? `Buy ${baseOf(sym)}` : `Sell ${baseOf(sym)} (short)`;
    const why = sym ? gate(c, q) : "Pick a symbol on the board.";
    goEl.disabled = S.busy || !!why || !qty;

    const f = S.flash && S.flash.until > performance.now() ? S.flash : null;
    const m = $("#tkMsg");
    m.textContent = f ? f.text : why || (qty ? "" : c.market?.id === "india" ? "Enter a whole number of shares." : "Enter a quantity or pick an amount.");
    m.className = "tk-msg" + (f ? " " + f.kind : "");
  }

  async function place() {
    const c = ctx();
    const q = c.sym && c.market ? c.market.quote(c.sym) : null;
    const why = gate(c, q);
    const qty = parseQty(c);
    if (why || !qty) return renderTicket();
    S.busy = true;
    renderTicket();
    try {
      await api("/api/positions", { ...JSON_POST, body: JSON.stringify({ market: c.market.id, mode: c.mode, symbol: c.sym, side: S.side, qty, price: q.price }) });
      showFlash("flash", `${S.side === "long" ? "Bought" : "Sold short"} ${fmtQty(qty)} ${baseOf(c.sym)} at ${fmt(q.price, q.dp)}`, "ok");
      await refresh();
    } catch (e) {
      showFlash("flash", e.message, "err");
    } finally {
      S.busy = false;
      renderTicket();
    }
  }

  $("#tkSide").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    S.side = b.dataset.s;
    for (const x of document.querySelectorAll("#tkSide button")) x.classList.toggle("on", x === b);
    renderTicket();
  });
  qtyEl.addEventListener("input", renderTicket);
  qtyEl.addEventListener("keydown", (e) => {
    if (e.key === "Enter") place();
  });
  goEl.addEventListener("click", place);

  // --------------------------------------------------------------- positions
  function buildRows() {
    const body = $("#posBody");
    const keep = new Set(S.positions.map((p) => p.id));
    for (const [id, r] of S.rows) if (!keep.has(id)) (r.tr.remove(), S.rows.delete(id));
    S.positions.forEach((p, i) => {
      let r = S.rows.get(p.id);
      if (!r) {
        const tr = el("tr", "click");
        const sym = el("td");
        sym.append(el("b", null, baseOf(p.symbol)));
        const tag = el("span", "side-tag " + p.side, p.side === "long" ? "Long" : "Short");
        const side = el("td");
        side.append(tag);
        const last = el("td");
        const pnl = el("td");
        const pct = el("td");
        const btn = el("button", "pos-close", "Close");
        btn.type = "button";
        btn.addEventListener("click", (e) => (e.stopPropagation(), closePosition(p.id)));
        const act = el("td");
        act.append(btn);
        tr.append(sym, side, el("td", null, fmtQty(p.qty)), el("td", null, fmtAuto(p.entry)), last, pnl, pct, act);
        tr.addEventListener("click", () => openSymbol(p.symbol));
        r = { tr, last, pnl, pct, btn, shown: {} };
        S.rows.set(p.id, r);
      }
      r.p = p;
      if (body.children[i] !== r.tr) body.insertBefore(r.tr, body.children[i] || null); // newest first
    });
  }

  function renderPositions() {
    const c = ctx();
    $("#posCount").textContent = S.positions.length ? `(${S.positions.length})` : "";
    const emptyText =
      c.mode === "live"
        ? "Live account: no positions. No broker is connected, so this account can't trade yet. Switch to Demo to practise."
        : "No open positions. Place an order from the ticket; closing it logs the trade to the Journal.";
    if (S.emptyText !== emptyText) $("#posEmpty").textContent = S.emptyText = emptyText;
    $("#posEmpty").hidden = S.positions.length > 0;
    $("#posWrap").hidden = !S.positions.length;

    let total = 0;
    let priced = 0;
    const cur = c.market?.currency ?? "";
    for (const r of S.rows.values()) {
      const p = r.p;
      const q = c.market ? c.market.quote(p.symbol) : null;
      const set = (k, node, text, cls) => {
        if (r.shown[k] !== text + (cls || "")) {
          node.textContent = text;
          if (cls !== undefined) node.className = cls;
          r.shown[k] = text + (cls || "");
        }
      };
      if (q) {
        const dir = p.side === "long" ? 1 : -1;
        const net = (q.price - p.entry) * p.qty * dir - (p.fee_pct / 100) * p.qty * (p.entry + q.price); // what closing now would record
        const pct = (net / (p.entry * p.qty)) * 100;
        total += net;
        priced++;
        set("last", r.last, fmt(q.price, q.dp));
        set("pnl", r.pnl, money(net, ""), net >= 0 ? "up" : "dn");
        set("pct", r.pct, `${pct < 0 ? "-" : "+"}${fmt(Math.abs(pct), 2)}%`, net >= 0 ? "up" : "dn");
      } else {
        set("last", r.last, "–");
        set("pnl", r.pnl, "–", "");
        set("pct", r.pct, "", "");
      }
      r.btn.disabled = !!gate(c, q);
      r.btn.title = r.btn.disabled ? gate(c, q) : `Close at ${q ? fmt(q.price, q.dp) : "the live price"}`;
    }
    const t = $("#posTotal");
    t.textContent = priced ? `Unrealized ${money(total, cur)}` : "";
    t.className = "pos-total " + (total >= 0 ? "up" : "dn");

    const f = S.posFlash && S.posFlash.until > performance.now() ? S.posFlash : null;
    const m = $("#posMsg");
    m.textContent = f ? f.text : "";
    m.className = "pos-msg" + (f ? " " + f.kind : "");
  }

  async function closePosition(id) {
    const c = ctx();
    const p = S.positions.find((x) => x.id === id);
    const q = p && c.market ? c.market.quote(p.symbol) : null;
    const why = p ? gate(c, q) : "That position is already closed.";
    if (why) return (showFlash("posFlash", why, "err"), renderPositions());
    const r = S.rows.get(id);
    if (r) r.btn.disabled = true;
    try {
      const d = await api(`/api/positions/${id}/close`, { ...JSON_POST, body: JSON.stringify({ price: q.price, date: today() }) });
      const t = d.trade;
      showFlash("posFlash", `Closed ${baseOf(t.symbol)} ${t.side}: ${money(t.pnl, c.market.currency)} after fees. Logged to the Journal.`, t.pnl >= 0 ? "ok" : "err");
    } catch (e) {
      showFlash("posFlash", e.message, "err");
    }
    await refresh();
  }

  /** Reload the open positions for the current market and mode. */
  async function refresh() {
    const c = ctx();
    if (!c.market) return;
    const key = `${c.market.id}/${c.mode}`;
    try {
      const d = await api(`/api/positions?market=${c.market.id}&mode=${c.mode}`);
      const now = ctx();
      if (now.market !== c.market || now.mode !== c.mode) return; // switched while loading
      S.positions = d.positions;
      S.loadedFor = key;
      c.market.watch?.(S.positions.map((p) => p.symbol)); // keep prices flowing for symbols that aren't on screen
      buildRows();
    } catch (e) {
      showFlash("posFlash", `Couldn't load positions: ${e.message}`, "err");
    }
    render();
  }

  function render() {
    renderTicket();
    renderPositions();
  }

  // Prices change many times a second but a ticket only needs a couple of repaints a second.
  setInterval(() => {
    if (!document.hidden && !$("#boardView").hidden) render();
  }, 400);

  return {
    refresh,
    render,
    setFees(f) {
      if (f) S.fees = f;
    },
    /** Forget the previous market/account's positions and messages at once, before the new ones arrive. */
    reset() {
      S.flash = S.posFlash = null; // an order confirmation from the other account must not linger
      S.positions = [];
      buildRows();
      render();
    },
  };
}
