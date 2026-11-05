//! Performance attribution: where profit and loss came from, traceable to each trade and its orders and fills.
//!
//! Per closed trade: gross P&L (price move x quantity), fees (as recorded on the trade), net P&L (gross minus
//! fees, through the same `accounts::realised` as the wallets, so totals reconcile exactly). Slippage is shown
//! only where a reference price exists: broker fills (the bar price the order was sized on versus the
//! exchange's average fill) and paper fills (the bot's configured slippage, labelled as modelled). Funding is
//! not applicable: ZoneBoard trades spot and paper only. Fees are counted once, on the trade.

use std::collections::BTreeMap;

use rusqlite::{params, Connection};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::accounts::{dec_str, realised};

pub const DIMENSIONS: [&str; 7] = ["bot", "strategy_version", "symbol", "account", "exit_reason", "side", "month"];

struct Row {
    id: i64,
    date: String,
    account: String,
    bot: Option<i64>,
    bot_name: String,
    version: Option<i64>,
    symbol: String,
    side: String,
    qty: f64,
    entry: f64,
    exit: f64,
    fees: f64,
    notes: String,
    gross: Decimal,
    net: Decimal,
}

fn exit_reason(notes: &str) -> String {
    // bot trades are noted "Bot NAME: REASON"; broker fills "Bot NAME on ACCOUNT: broker fill N (REASON)"
    if let Some(i) = notes.rfind('(') {
        if notes.ends_with(')') {
            return notes[i + 1..notes.len() - 1].to_string();
        }
    }
    notes.split_once(": ").map(|(_, r)| r.to_string()).unwrap_or_else(|| if notes.is_empty() { "manual".into() } else { notes.to_string() })
}

fn rows(conn: &Connection, from: Option<&str>, to: Option<&str>, account: Option<&str>, bot: Option<i64>) -> Result<Vec<Row>, String> {
    let mut st = conn
        .prepare(
            "SELECT t.id, t.date, t.account_id, t.bot_id, coalesce(b.name, ''), t.strategy_version_id, t.symbol, t.side, t.qty, t.entry, t.exit, t.fees, t.notes
             FROM trades t LEFT JOIN bots b ON b.id = t.bot_id
             WHERE t.account_id IS NOT NULL AND (?1 IS NULL OR t.date >= ?1) AND (?2 IS NULL OR t.date <= ?2) AND (?3 IS NULL OR t.account_id = ?3) AND (?4 IS NULL OR t.bot_id = ?4)
             ORDER BY t.date, t.id",
        )
        .map_err(|e| e.to_string())?;
    let m = st
        .query_map(params![from, to, account, bot], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<i64>>(5)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?, r.get::<_, f64>(8)?, r.get::<_, f64>(9)?, r.get::<_, f64>(10)?, r.get::<_, f64>(11)?, r.get::<_, String>(12)?))
        })
        .map_err(|e| e.to_string())?;
    Ok(m.filter_map(Result::ok)
        .filter_map(|(id, date, account, bot, bot_name, version, symbol, side, qty, entry, exit, fees, notes)| {
            let gross = realised(&side, qty, entry, exit, 0.0)?;
            let net = realised(&side, qty, entry, exit, fees)?;
            Some(Row { id, date, account, bot, bot_name, version, symbol, side, qty, entry, exit, fees, notes, gross, net })
        })
        .collect())
}

/// Totals grouped by `dimension`, plus the grand total, for a date range and optional account / bot filter.
pub fn attribute(conn: &Connection, dimension: &str, from: Option<&str>, to: Option<&str>, account: Option<&str>, bot: Option<i64>) -> Result<Value, String> {
    if !DIMENSIONS.contains(&dimension) {
        return Err(format!("dimension must be one of {}", DIMENSIONS.join(", ")));
    }
    let rs = rows(conn, from, to, account, bot)?;
    let key = |r: &Row| -> String {
        match dimension {
            "bot" => r.bot.map(|b| format!("{} (bot {b})", if r.bot_name.is_empty() { "deleted".into() } else { r.bot_name.clone() })).unwrap_or_else(|| "manual".into()),
            "strategy_version" => r.version.map(|v| format!("version {v}")).unwrap_or_else(|| "none (manual or before versioning)".into()),
            "symbol" => r.symbol.clone(),
            "account" => r.account.clone(),
            "exit_reason" => exit_reason(&r.notes),
            "side" => r.side.clone(),
            _ => r.date.chars().take(7).collect(),
        }
    };
    let mut groups: BTreeMap<String, (Decimal, Decimal, Decimal, usize, usize)> = BTreeMap::new();
    let (mut tg, mut tf, mut tn) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    for r in &rs {
        let fee = r.gross - r.net;
        let e = groups.entry(key(r)).or_default();
        e.0 += r.gross;
        e.1 += fee;
        e.2 += r.net;
        e.3 += 1;
        e.4 += (r.net > Decimal::ZERO) as usize;
        tg += r.gross;
        tf += fee;
        tn += r.net;
    }
    let total_abs: Decimal = groups.values().map(|g| g.2.abs()).sum();
    let out: Vec<Value> = groups
        .into_iter()
        .map(|(k, (g, f, n, c, w))| {
            json!({ "key": k, "gross_pnl": dec_str(g), "fees": dec_str(f.round_dp(8)), "net_pnl": dec_str(n), "trades": c, "win_rate": (w as f64 / c as f64 * 10000.0).round() / 100.0,
                "share_of_abs_pnl_pct": if total_abs > Decimal::ZERO { (n.abs() / total_abs * Decimal::from(100)).to_f64().map(|x| (x * 100.0).round() / 100.0) } else { None } })
        })
        .collect();
    // reconciliation: groups add up to the total, which equals the sum of trade-level net P&L
    let sum_groups: Decimal = out.iter().filter_map(|g| g["net_pnl"].as_str().and_then(|s| s.parse::<Decimal>().ok())).sum();
    Ok(json!({
        "dimension": dimension, "filters": { "from": from, "to": to, "account": account, "bot": bot },
        "groups": out, "total": { "gross_pnl": dec_str(tg), "fees": dec_str(tf.round_dp(8)), "net_pnl": dec_str(tn), "trades": rs.len() },
        "reconciled": sum_groups == tn,
        "costs": { "fees": "as recorded on each trade (paper: the account's fee rate on both sides; broker: estimated at the same rate)", "funding": "not applicable (spot and paper only)", "slippage": "per trade in the drill-down where a reference price exists" },
        "rounding": "decimal arithmetic, 8 places, half-even, as the wallets",
    }))
}

/// One trade with everything behind it: its strategy version, the orders that opened and closed it (filled and
/// refused), broker fills and the slippage they show.
pub fn trade_detail(conn: &Connection, trade_id: i64) -> Result<Value, String> {
    let r = rows(conn, None, None, None, None)?.into_iter().find(|r| r.id == trade_id).ok_or("no such trade (or not on an account)")?;
    let orders: Vec<Value> = conn
        .prepare("SELECT id, ts_utc, bar_time, action, side, qty, price, status, reason, client_order_id, broker_order_id FROM bot_orders WHERE trade_id = ?1 OR (bot_id = ?2 AND position_id IN (SELECT position_id FROM bot_orders WHERE trade_id = ?1))")
        .and_then(|mut st| {
            st.query_map(params![trade_id, r.bot], |x| Ok(json!({ "id": x.get::<_, i64>(0)?, "ts": x.get::<_, String>(1)?, "bar_time": x.get::<_, Option<i64>>(2)?, "action": x.get::<_, String>(3)?, "side": x.get::<_, String>(4)?, "qty": x.get::<_, f64>(5)?, "price": x.get::<_, f64>(6)?, "status": x.get::<_, String>(7)?, "reason": x.get::<_, String>(8)?, "client_order_id": x.get::<_, Option<String>>(9)?, "broker_order_id": x.get::<_, Option<String>>(10)? })))
                .map(|m| m.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    let cids: Vec<String> = orders.iter().filter_map(|o| o["client_order_id"].as_str().map(String::from)).collect();
    let mut fills = vec![];
    for cid in &cids {
        if let Ok(f) = conn.query_row("SELECT client_order_id, side, ref_price, avg_price, executed_qty, status FROM broker_orders WHERE client_order_id = ?1", [cid], |x| {
            let (refp, avg): (Option<f64>, Option<String>) = (x.get(2)?, x.get(3)?);
            let avgf = avg.as_deref().and_then(|a| a.parse::<f64>().ok());
            let side: String = x.get(1)?;
            let slip = match (refp, avgf) {
                (Some(rp), Some(a)) if rp > 0.0 => Some(((a - rp) / rp * if side == "BUY" { 1.0 } else { -1.0 } * 1e6).round() / 1e4),
                _ => None,
            };
            Ok(json!({ "client_order_id": x.get::<_, String>(0)?, "side": side, "reference_price": refp, "avg_fill_price": avg, "executed_qty": x.get::<_, Option<String>>(4)?, "status": x.get::<_, String>(5)?, "slippage_pct_against_you": slip }))
        }) {
            fills.push(f);
        }
    }
    let modelled = r.bot.and_then(|b| conn.query_row("SELECT slippage_pct FROM bots WHERE id = ?1", [b], |x| x.get::<_, f64>(0)).ok());
    Ok(json!({
        "trade": { "id": r.id, "date": r.date, "account_id": r.account, "bot_id": r.bot, "bot": r.bot_name, "symbol": r.symbol, "side": r.side, "qty": r.qty, "entry": r.entry, "exit": r.exit,
            "gross_pnl": dec_str(r.gross), "fees": r.fees, "net_pnl": dec_str(r.net), "exit_reason": exit_reason(&r.notes), "notes": r.notes },
        "strategy_version": r.version.map(|v| crate::strategies::version(conn, v).ok().flatten()).unwrap_or(None),
        "orders": orders, "broker_fills": fills,
        "slippage": if !fills.is_empty() { json!("measured on broker fills (see broker_fills)") } else if let Some(m) = modelled { json!(format!("paper fill: modelled at {m}% per market fill, already inside the entry and exit prices")) } else { json!("not available for a manual trade") },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_reconcile_and_fees_are_counted_once() {
        let c = crate::db::open(":memory:").unwrap();
        for (sym, side, e, x, fees, notes) in [("BTCUSDT", "long", 100.0, 110.0, 1.0, "Bot A: take-profit"), ("BTCUSDT", "short", 100.0, 105.0, 0.5, "Bot A: stop-loss"), ("ETHUSDT", "long", 50.0, 49.0, 0.25, "Paper trade")] {
            c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode, account_id) VALUES ('2026-11-02',?1,?2,2,?3,?4,?5,?6,'crypto','demo','demo-1')", params![sym, side, e, x, fees, notes]).unwrap();
        }
        let a = attribute(&c, "symbol", None, None, None, None).unwrap();
        assert_eq!(a["reconciled"], true);
        // gross: +20, -10, -2 = 8; fees 1.75; net 6.25
        assert_eq!((a["total"]["gross_pnl"].as_str(), a["total"]["fees"].as_str(), a["total"]["net_pnl"].as_str()), (Some("8"), Some("1.75"), Some("6.25")));
        let r = attribute(&c, "exit_reason", None, None, None, None).unwrap();
        let keys: Vec<&str> = r["groups"].as_array().unwrap().iter().map(|g| g["key"].as_str().unwrap()).collect();
        assert_eq!(keys, vec!["Paper trade", "stop-loss", "take-profit"]);
        assert!(attribute(&c, "planet", None, None, None, None).is_err());
        assert_eq!(attribute(&c, "month", Some("2026-12-01"), None, None, None).unwrap()["total"]["trades"], 0);
        let d = trade_detail(&c, 1).unwrap();
        assert_eq!(d["trade"]["exit_reason"], "take-profit");
    }
}
