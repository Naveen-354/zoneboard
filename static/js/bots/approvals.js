// Approval Center: live-deployment requests, each bound to an exact bot configuration (strategy version and
// hash, account, symbols, capital, risk). Only you decide; the AI assistant has no way to approve anything.
import { el, api, post, button, say, badge, table, row, td, whenIso, input, money } from "./ui.js";

export async function render(host, ctx) {
  host.replaceChildren(el("p", "st-about", "Loading approvals…"));
  let d;
  try {
    d = await api("/api/approvals");
  } catch (e) {
    host.replaceChildren(el("p", "st-msg err", e.message));
    return;
  }
  const c = el("section", "card st-card");
  const hd = el("div", "st-card-h");
  hd.append(el("h3", null, "Approval Center"), el("span", "jsub", "Live deployments need your explicit approval of the exact configuration. Any change invalidates it."));
  c.append(hd);
  if (!d.approvals.length) {
    c.append(el("p", "st-about", "No requests. A bot on a connected live Binance account asks for approval from Bot Performance → Live readiness, after a saved backtest."));
    host.replaceChildren(c);
    return;
  }
  const bots = new Map(ctx.bots.map((b) => [b.id, b]));
  for (const a of d.approvals) {
    const card = el("article", "card st-card appr");
    const b = bots.get(a.bot_id);
    const status = badge(a.status.toUpperCase(), { pending: "warn", approved: "ok", rejected: "off", revoked: "off", expired: "off" }[a.status]);
    const h = el("div", "bot-h");
    const t = el("div", "bot-t");
    t.append(el("b", null, `#${a.id} · ${b ? b.name : `bot ${a.bot_id}`}`), el("small", null, `requested ${whenIso(a.requested_utc)} by ${a.requested_by} · expires ${whenIso(a.expires_utc)}`));
    h.append(t, status, a.matches_current_bot ? badge("MATCHES THE BOT NOW", "ok") : badge("BOT HAS CHANGED SINCE", "warn"));
    const s = a.scope;
    const ev = a.evidence?.latest_backtest?.metrics;
    card.append(
      h,
      table(
        ["Scope", "Value"],
        [
          row(["Account", `${s.account_id} (${s.environment})`]),
          row(["Strategy version", `#${s.strategy_version_id} · ${String(s.spec_hash).slice(0, 16)}…`]),
          row(["Symbols / timeframe", `${s.symbols.filter(Boolean).join(" / ")} · ${s.interval}`]),
          row(["Capital", money(s.allocation)]),
          row(["Max daily loss / open positions / slippage", `${s.max_daily_loss ? money(s.max_daily_loss) : "none"} / ${s.max_open_positions} / ${s.slippage_pct}%`]),
          row(["Backtest evidence", ev ? `${ev.trades} trades, return ${ev.return_pct}%, max drawdown ${ev.max_drawdown_pct}%, profit factor ${ev.profit_factor ?? "–"}` : "none"]),
          row(["Closed trades recorded", String(a.evidence?.closed_trades_recorded ?? 0)]),
        ].concat(a.decided_utc ? [row(["Decision", `${a.status} by ${a.decided_by || "–"} at ${whenIso(a.decided_utc)}${a.reason ? `: ${a.reason}` : ""}`])] : []),
      ),
    );
    const msg = el("p", "st-msg");
    if (a.status === "pending") {
      const phrase = input("", { placeholder: d.phrase, "aria-label": "approval phrase" });
      const reason = input("", { placeholder: "Note (optional)" });
      const bar = el("div", "st-actions");
      bar.append(
        phrase,
        reason,
        button("Approve", "btn danger sm", async () => {
          try {
            await post(`/api/approvals/${a.id}/decide`, { approve: true, confirm: phrase.value.trim(), reason: reason.value });
            render(host, ctx);
          } catch (e) {
            say(msg, e.message, "err");
          }
        }),
        button("Reject", "btn ghost sm", async () => {
          await post(`/api/approvals/${a.id}/decide`, { approve: false, reason: reason.value });
          render(host, ctx);
        }),
      );
      card.append(el("p", "st-about", `To approve, type ${d.phrase}. This lets the bot place REAL orders with real money once live trading is armed.`), bar);
    }
    if (a.status === "approved")
      card.append(
        button("Revoke", "btn ghost sm", async () => {
          await post(`/api/approvals/${a.id}/revoke`, { reason: "revoked by you" });
          render(host, ctx);
        }),
      );
    card.append(msg);
    c.append(card);
  }
  host.replaceChildren(c);
}

export { td };
