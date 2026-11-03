// Journal tab: the history of closed paper trades and their stats. Trades are made on the chart screen (see
// trading.js) and logged here when a position is closed. They're kept per market, and per mode (live-price or
// demo) so USDT and rupees never mix and practice trades never pollute real-price ones.
import { $, el, fmt, fmtAuto, fmtQty, signed, api } from "./util.js";

let market = null;
let mode = "live";

function stat(box, label, value, cls) {
  const s = el("div", "stat");
  s.append(el("b", cls, value), el("small", null, label));
  box.append(s);
}

/** Which journal is showing: set when the market or the Live/Demo mode changes. */
export function setContext(m, md) {
  market = m;
  mode = md;
  $("#jctx").textContent = `${m.label} journal`;
  $("#jsub").textContent =
    md === "live"
      ? "Live account: trades made through a connected broker (none connected yet)"
      : m.id === "india"
        ? "Demo account: paper trades at real NSE prices (about 15 min old)"
        : "Demo account: paper trades at real prices";
  $("#jempty").textContent =
    md === "live"
      ? "No live-account trades. Live trading isn't connected yet; your practice trades are in the Demo account."
      : "No closed trades yet. Place an order on the chart screen; when you close the position it is logged here.";
}

export async function load() {
  if (!market) return;
  const m = market;
  const md = mode;
  const d = await api(`/api/journal?market=${m.id}&mode=${md}`);
  if (m !== market || md !== mode) return; // switched while loading
  const s = d.stats;
  const box = $("#jstats");
  box.replaceChildren();
  stat(box, "Closed trades", String(s.trades));
  stat(box, `Net P&L (${m.currency})`, signed(s.net_pnl), s.net_pnl >= 0 ? "up" : "dn");
  stat(box, "Win rate", s.win_rate + "%");
  stat(box, "Avg win", fmt(s.avg_win));
  stat(box, "Avg loss", fmt(s.avg_loss));
  stat(box, "Profit factor", s.profit_factor == null ? "–" : String(s.profit_factor));
  stat(box, "Fees paid", fmt(s.fees));

  const tb = $("#jbody");
  tb.replaceChildren();
  $("#jempty").hidden = d.trades.length > 0;
  for (const t of d.trades) {
    const tr = el("tr");
    for (const v of [t.date, t.symbol, t.side, fmtQty(t.qty), fmtAuto(t.entry), fmtAuto(t.exit), fmt(t.fees)]) tr.append(el("td", null, String(v)));
    tr.append(el("td", t.pnl >= 0 ? "up" : "dn", signed(t.pnl)));
    tr.append(el("td", "notes", t.notes));
    const td = el("td");
    const b = el("button", null, "✕");
    b.type = "button";
    b.title = "Delete this trade from the journal";
    b.addEventListener("click", async () => {
      if (!confirm("Delete this trade from the journal?")) return;
      await api("/api/journal/" + t.id, { method: "DELETE" });
      load();
    });
    td.append(b);
    tr.append(td);
    tb.append(tr);
  }
}
