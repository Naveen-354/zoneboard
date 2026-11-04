// Portfolio: accounts, broker balances with reconciliation, attribution by bot and manual trading, period
// reports, exposure, correlation and an allocation proposal that is never applied by itself.
import { $, el, api, fmtAuto } from "./util.js";

function table(headers, rows) {
  const t = el("table");
  const hr = el("tr");
  for (const h of headers) hr.append(el("th", null, h));
  const th = el("thead");
  th.append(hr);
  const tb = el("tbody");
  for (const r of rows) {
    const tr = el("tr");
    for (const c of r) tr.append(c instanceof Node ? c : el("td", null, c == null ? "–" : String(c)));
    tb.append(tr);
  }
  t.append(th, tb);
  const w = el("div", "table-wrap");
  w.append(t);
  return w;
}

const num = (x, cls = "num") => el("td", cls + (Number(x) > 0 ? " up" : Number(x) < 0 ? " dn" : ""), x == null ? "–" : String(x));

function card(title, ...kids) {
  const c = el("section", "card st-card");
  c.append(el("h4", null, title), ...kids);
  return c;
}

export async function load() {
  const host = $("#pfBody");
  host.replaceChildren(el("p", "st-about", "Loading…"));
  let d;
  try {
    d = await api("/api/portfolio");
  } catch (e) {
    host.replaceChildren(el("p", "st-msg err", e.message));
    return;
  }
  const out = [el("p", "no-money", d.notice)];
  // accounts
  const acc = d.accounts.filter((a) => !a.archived);
  out.push(
    card(
      "Accounts",
      table(
        ["Account", "Kind", "Environment", "Balances"],
        acc.map((a) => {
          if (a.kind === "demo") return [a.label + " (VIRTUAL)", a.kind, a.environment, a.wallets.map((w) => `${w.currency} ${w.balance} (available ${w.available})`).join(" · ")];
          const b = d.broker[a.id];
          const text = b?.error ? `unavailable: ${b.error}` : (b?.balances?.balances || []).map((x) => `${x.asset} ${x.free}${x.locked !== "0.00000000" ? ` (+${x.locked} locked)` : ""}`).join(" · ") || "no balances";
          return [a.label, a.kind, a.environment === "live" ? "LIVE (real)" : "TESTNET", text];
        }),
      ),
    ),
  );
  // reconciliation
  const rec = Object.entries(d.broker).filter(([, b]) => b.reconciliation);
  if (rec.length)
    out.push(
      card(
        "Reconciliation: bot positions vs broker balances",
        table(
          ["Account", "Asset", "Bots hold", "Broker reports", "Status"],
          rec.flatMap(([id, b]) => b.reconciliation.rows.map((r) => [id, r.asset, fmtAuto(r.zoneboard_bot_qty), fmtAuto(r.broker_qty), el("td", r.short_by == null ? "up" : "dn", r.short_by == null ? "ok" : `broker short by ${fmtAuto(r.short_by)}`)])),
        ),
        el("p", "st-about", "Differences are shown, never corrected automatically. You may also hold coins outside ZoneBoard."),
      ),
    );
  out.push(card("Performance attribution (closed trades)", table(["Account", "Source", "Currency", "Net P&L", "Trades", "Win rate %", "Fees"], d.attribution.map((r) => [r.account_id, r.source, r.currency, num(r.net_pnl), r.trades, r.win_rate, r.fees]))));
  out.push(card("Open exposure (at entry prices)", table(["Account", "Currency", "Long", "Short", "Gross", "Net"], d.exposure.map((r) => [r.account_id, r.currency, fmtAuto(r.long), fmtAuto(r.short), fmtAuto(r.gross), fmtAuto(r.net)]))));
  const per = el("select");
  for (const p of ["day", "week", "month"]) {
    const o = el("option", null, p === "day" ? "Daily" : p === "week" ? "Weekly" : "Monthly");
    o.value = p;
    per.append(o);
  }
  per.value = "month";
  const rep = el("div");
  const paint = () => rep.replaceChildren(table(["Period", "Account", "Net P&L"], (d.reports[per.value] || []).slice(-60).reverse().map((r) => [r.period, r.account, num(r.net_pnl)])));
  per.addEventListener("change", paint);
  paint();
  const csv = el("button", "btn ghost sm", "Download all trades (CSV)");
  csv.type = "button";
  csv.addEventListener("click", async () => {
    const r = await fetch("/api/portfolio/trades.csv", { headers: { "X-ZB-Token": document.querySelector('meta[name="zb-token"]').content } });
    const url = URL.createObjectURL(await r.blob());
    const a = el("a");
    a.href = url;
    a.download = "zoneboard-trades.csv";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  out.push(card("Reports", per, rep, csv));
  out.push(card("Correlation of bots' daily P&L", d.correlation.length ? table(["Bot A", "Bot B", "Shared days", "Correlation"], d.correlation.map((r) => [r.a, r.b, r.days, r.correlation ?? "too few days"])) : el("p", "st-about", "Needs at least two bots with closed trades.")));
  const p = d.allocation_proposal;
  out.push(card("Capital allocation proposal (not applied)", el("p", "st-about", p.note + (p.method ? ` Method: ${p.method}.` : "")), p.rows.length ? table(["Bot", "Daily P&L s.d.", "Weight %", "Proposed allocation"], p.rows.map((r) => [r.bot, r.daily_pnl_sd, r.weight_pct, r.proposed_allocation])) : el("span")));
  host.replaceChildren(...out);
}
