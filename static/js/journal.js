// Trade journal tab: log trades, see P&L stats. Trades are kept per market so USDT and rupees never mix.
import { $, el, fmt, fmtAuto, signed, api } from "./util.js";

const form = $("#jform");
let market = null;

const today = () => new Date().toLocaleDateString("en-CA"); // YYYY-MM-DD in local time

function stat(box, label, value, cls) {
  const s = el("div", "stat");
  s.append(el("b", cls, value), el("small", null, label));
  box.append(s);
}

export async function load() {
  if (!market) return;
  const d = await api("/api/journal?market=" + market.id);
  const s = d.stats;
  const cur = market.currency;
  const box = $("#jstats");
  box.replaceChildren();
  stat(box, "Trades", String(s.trades));
  stat(box, `Net P&L (${cur})`, signed(s.net_pnl), s.net_pnl >= 0 ? "up" : "dn");
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
    for (const v of [t.date, t.symbol, t.side, +t.qty.toFixed(8), fmtAuto(t.entry), fmtAuto(t.exit), fmt(t.fees)]) tr.append(el("td", null, String(v)));
    tr.append(el("td", t.pnl >= 0 ? "up" : "dn", signed(t.pnl)));
    tr.append(el("td", "notes", t.notes));
    const td = el("td");
    const b = el("button", null, "✕");
    b.type = "button";
    b.title = "Delete trade";
    b.addEventListener("click", async () => {
      if (!confirm("Delete this trade?")) return;
      await api("/api/journal/" + t.id, { method: "DELETE" });
      load();
    });
    td.append(b);
    tr.append(td);
    tb.append(tr);
  }
}

/** Called when the market switches: trades are filtered per market and the symbol hint changes. */
export function setMarket(m) {
  market = m;
  form.symbol.placeholder = m.id === "crypto" ? "BTCUSDT" : "RELIANCE";
  form.qty.step = m.id === "crypto" ? "any" : "1";
  form.qty.min = m.id === "crypto" ? "0" : "1";
  form.fees.placeholder = `Fees + taxes (${m.currency})`;
}

/** Suggest the symbol that is open on the chart. */
export function prefill(sym) {
  if (sym && !form.symbol.value) form.symbol.value = sym;
}

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const f = Object.fromEntries(new FormData(form));
  try {
    await api("/api/journal", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ ...f, market: market.id }) });
    $("#jerr").hidden = true;
    const date = form.date.value;
    form.reset();
    form.date.value = date;
    load();
  } catch (err) {
    $("#jerr").textContent = err.message;
    $("#jerr").hidden = false;
  }
});

form.date.value = today();
