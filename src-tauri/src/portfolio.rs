//! Portfolio and performance: every account, every position and trade, manual and bot, with attribution.
//!
//! Numbers come from recorded trades and positions (realised P&L through the same `accounts::realised` as the
//! wallets), and for connected broker accounts from the broker. Demo (virtual) and broker figures are kept in
//! separate sections and never summed together. Correlation and the capital-allocation proposal are
//! statistics on recorded daily P&L; the proposal is **never applied automatically**.

use std::collections::BTreeMap;

use rusqlite::Connection;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::accounts::{currency_for, dec_str, realised};

struct T {
    date: String,
    account: Option<String>,
    bot: Option<i64>,
    market: String,
    symbol: String,
    pnl: Decimal,
    fees: f64,
}

fn trades(conn: &Connection) -> Result<Vec<T>, String> {
    let mut st = conn.prepare("SELECT date, account_id, bot_id, market, symbol, side, qty, entry, exit, fees FROM trades WHERE account_id IS NOT NULL ORDER BY date, id").map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, f64>(6)?, r.get::<_, f64>(7)?, r.get::<_, f64>(8)?, r.get::<_, f64>(9)?))
        })
        .map_err(|e| e.to_string())?;
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|(date, account, bot, market, symbol, side, q, e, x, f)| realised(&side, q, e, x, f).map(|pnl| T { date, account, bot, market, symbol, pnl, fees: f }))
        .collect())
}

fn period_key(date: &str, period: &str) -> String {
    let Ok(d) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else { return date.to_string() };
    match period {
        "week" => {
            let w = d.iso_week();
            format!("{}-W{:02}", w.year(), w.week())
        }
        "month" => d.format("%Y-%m").to_string(),
        _ => d.format("%Y-%m-%d").to_string(),
    }
}

use chrono::Datelike;

fn pearson(a: &[f64], b: &[f64]) -> Option<f64> {
    let n = a.len();
    if n < 5 {
        return None;
    }
    let (ma, mb) = (a.iter().sum::<f64>() / n as f64, b.iter().sum::<f64>() / n as f64);
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let (va, vb): (f64, f64) = (a.iter().map(|x| (x - ma).powi(2)).sum(), b.iter().map(|y| (y - mb).powi(2)).sum());
    (va > 0.0 && vb > 0.0).then(|| (cov / (va * vb).sqrt() * 1000.0).round() / 1000.0)
}

/// The whole picture. `broker` carries balances already fetched for connected accounts (or their errors).
pub fn overview(conn: &Connection, broker: Value) -> Result<Value, String> {
    let ts = trades(conn)?;
    let accounts = crate::accounts::list(conn).map_err(|e| format!("{e:?}"))?;
    let bot_names: BTreeMap<i64, (String, String)> = {
        let mut st = conn.prepare("SELECT id, name, allocation FROM bots").map_err(|e| e.to_string())?;
        let m = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, (r.get::<_, String>(1)?, r.get::<_, String>(2)?)))).map_err(|e| e.to_string())?;
        m.filter_map(Result::ok).collect()
    };
    let label = |bot: Option<i64>| bot.map(|b| bot_names.get(&b).map(|x| x.0.clone()).unwrap_or(format!("bot {b} (deleted)"))).unwrap_or_else(|| "Manual".into());

    // attribution: per account, per source (manual or bot), per currency
    let mut attr: BTreeMap<(String, String, String), (Decimal, usize, usize, f64)> = BTreeMap::new();
    let mut periods: BTreeMap<(String, String, String), Decimal> = BTreeMap::new();
    let mut daily_by_source: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for t in &ts {
        let cur = currency_for(&t.market, &t.symbol).unwrap_or("?").to_string();
        let acct = t.account.clone().unwrap_or_default();
        let e = attr.entry((acct.clone(), label(t.bot), cur.clone())).or_default();
        e.0 += t.pnl;
        e.1 += 1;
        e.2 += (t.pnl > Decimal::ZERO) as usize;
        e.3 += t.fees;
        for p in ["day", "week", "month"] {
            *periods.entry((p.into(), period_key(&t.date, p), format!("{acct} {cur}"))).or_default() += t.pnl;
        }
        if let Some(b) = t.bot {
            *daily_by_source.entry(format!("{b}")).or_default().entry(t.date.clone()).or_default() += t.pnl.to_f64().unwrap_or(0.0);
        }
    }
    let attribution: Vec<Value> = attr
        .into_iter()
        .map(|((a, src, cur), (p, n, w, f))| json!({ "account_id": a, "source": src, "currency": cur, "net_pnl": dec_str(p), "trades": n, "win_rate": if n > 0 { (w as f64 / n as f64 * 10000.0).round() / 100.0 } else { 0.0 }, "fees": (f * 1e8).round() / 1e8 }))
        .collect();
    let mut reports = serde_json::Map::new();
    for p in ["day", "week", "month"] {
        let rows: Vec<Value> = periods.iter().filter(|((pp, _, _), _)| pp == p).map(|((_, k, ac), v)| json!({ "period": k, "account": ac, "net_pnl": dec_str(*v) })).collect();
        reports.insert(p.into(), Value::Array(rows));
    }

    // open positions: exposure by account, currency, source and symbol (at entry; prices move in the UI)
    let mut st = conn.prepare("SELECT account_id, bot_id, market, symbol, side, qty, entry FROM positions WHERE account_id IS NOT NULL").map_err(|e| e.to_string())?;
    let pos: Vec<(String, Option<i64>, String, String, String, f64, f64)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    let mut exposure: BTreeMap<(String, String), (f64, f64)> = BTreeMap::new();
    let mut by_symbol: BTreeMap<(String, String), f64> = BTreeMap::new();
    let positions: Vec<Value> = pos
        .iter()
        .map(|(a, b, m, s, side, q, e)| {
            let cur = currency_for(m, s).unwrap_or("?").to_string();
            let n = q * e;
            let x = exposure.entry((a.clone(), cur.clone())).or_default();
            if side == "long" { x.0 += n } else { x.1 += n }
            *by_symbol.entry((a.clone(), s.clone())).or_default() += n;
            json!({ "account_id": a, "source": label(*b), "symbol": s, "side": side, "qty": q, "entry": e, "notional": n, "currency": cur })
        })
        .collect();
    let exposure: Vec<Value> = exposure.into_iter().map(|((a, c), (l, s))| json!({ "account_id": a, "currency": c, "long": l, "short": s, "gross": l + s, "net": l - s })).collect();
    let allocation_by_symbol: Vec<Value> = by_symbol.into_iter().map(|((a, s), n)| json!({ "account_id": a, "symbol": s, "notional": n })).collect();

    // correlation of daily bot P&L, and an inverse-volatility allocation proposal (not applied)
    let ids: Vec<String> = daily_by_source.keys().cloned().collect();
    let mut corr = vec![];
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            let days: Vec<&String> = daily_by_source[&ids[i]].keys().filter(|d| daily_by_source[&ids[j]].contains_key(*d)).collect();
            let a: Vec<f64> = days.iter().map(|d| daily_by_source[&ids[i]][*d]).collect();
            let b: Vec<f64> = days.iter().map(|d| daily_by_source[&ids[j]][*d]).collect();
            corr.push(json!({ "a": label(ids[i].parse().ok()), "b": label(ids[j].parse().ok()), "days": days.len(), "correlation": pearson(&a, &b) }));
        }
    }
    let mut vols = vec![];
    for id in &ids {
        let v: Vec<f64> = daily_by_source[id].values().copied().collect();
        if v.len() >= 5 {
            let m = v.iter().sum::<f64>() / v.len() as f64;
            let sd = (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
            if sd > 0.0 {
                vols.push((id.clone(), sd));
            }
        }
    }
    let proposal = if vols.len() >= 2 {
        let total_alloc: f64 = vols.iter().filter_map(|(id, _)| bot_names.get(&id.parse().unwrap_or(0)).and_then(|x| x.1.parse::<f64>().ok())).sum();
        let inv: f64 = vols.iter().map(|(_, sd)| 1.0 / sd).sum();
        json!({
            "method": "inverse volatility of daily realised P&L (needs 5+ trading days per bot)", "applied": false,
            "note": "A proposal only. Allocations change only if you edit each bot yourself.",
            "rows": vols.iter().map(|(id, sd)| json!({ "bot": label(id.parse().ok()), "daily_pnl_sd": (sd * 1e6).round() / 1e6, "weight_pct": ((1.0 / sd) / inv * 10000.0).round() / 100.0, "proposed_allocation": ((1.0 / sd) / inv * total_alloc * 100.0).round() / 100.0 })).collect::<Vec<_>>(),
        })
    } else {
        json!({ "applied": false, "rows": [], "note": "Needs at least two bots with 5+ trading days of closed trades." })
    };

    Ok(json!({
        "accounts": accounts["accounts"], "broker": broker,
        "attribution": attribution, "reports": reports, "positions": positions, "exposure": exposure,
        "allocation_by_symbol": allocation_by_symbol, "correlation": corr, "allocation_proposal": proposal,
        "notice": "Demo figures are VIRTUAL and are never added to broker figures. Exposure is at entry prices.",
    }))
}

/// Compare what ZoneBoard thinks a broker account holds through its bots with what the broker reports.
/// Differences are shown, never hidden or auto-corrected (the owner may also hold coins outside ZoneBoard).
pub fn reconcile(conn: &Connection, account: &str, balances: &Value) -> Value {
    let mut held: BTreeMap<String, f64> = BTreeMap::new();
    if let Ok(mut st) = conn.prepare("SELECT symbol, qty FROM positions WHERE account_id = ?1 AND side = 'long'") {
        if let Ok(m) = st.query_map([account], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))) {
            for (s, q) in m.filter_map(Result::ok) {
                *held.entry(s.trim_end_matches("USDT").to_string()).or_default() += q;
            }
        }
    }
    let rows: Vec<Value> = held
        .iter()
        .map(|(asset, q)| {
            let b = balances["balances"].as_array().and_then(|a| a.iter().find(|x| x["asset"] == asset.as_str()));
            let broker: f64 = b.map(|x| x["free"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) + x["locked"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)).unwrap_or(0.0);
            json!({ "asset": asset, "zoneboard_bot_qty": q, "broker_qty": broker, "short_by": if broker + 1e-12 < *q { Some(q - broker) } else { None::<f64> } })
        })
        .collect();
    json!({ "account_id": account, "rows": rows, "ok": rows.iter().all(|r| r["short_by"].is_null()) })
}

/// Every recorded trade as CSV: date, account, source, symbol, side, qty, entry, exit, fees, net P&L.
pub fn trades_csv(conn: &Connection) -> Result<String, String> {
    let mut st = conn.prepare("SELECT t.date, t.account_id, coalesce(b.name, CASE WHEN t.bot_id IS NULL THEN 'Manual' ELSE 'bot ' || t.bot_id END), t.market, t.mode, t.symbol, t.side, t.qty, t.entry, t.exit, t.fees FROM trades t LEFT JOIN bots b ON b.id = t.bot_id ORDER BY t.date, t.id").map_err(|e| e.to_string())?;
    let q = |x: &str| format!("\"{}\"", x.replace('"', "\"\""));
    let mut out = String::from("date,account,source,market,mode,symbol,side,qty,entry,exit,fees,net_pnl\n");
    let rows = st
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, f64>(7)?, r.get::<_, f64>(8)?, r.get::<_, f64>(9)?, r.get::<_, f64>(10)?)))
        .map_err(|e| e.to_string())?;
    for (d, a, src, m, mode, s, side, qty, e, x, f) in rows.filter_map(Result::ok) {
        let net = realised(&side, qty, e, x, f).map(dec_str).unwrap_or_default();
        out.push_str(&format!("{d},{},{},{m},{mode},{},{side},{qty},{e},{x},{f},{net}\n", q(&a.unwrap_or_default()), q(&src), q(&s)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_reports_and_reconciliation() {
        let c = crate::db::open(":memory:").unwrap();
        for (i, (d, bot, x)) in [("2026-11-01", None, 110.0), ("2026-11-02", Some(1), 90.0), ("2026-11-09", Some(1), 120.0)].iter().enumerate() {
            c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode, account_id, bot_id) VALUES (?1,'BTCUSDT','long',1,100,?2,0,'','crypto','demo','demo-1',?3)", rusqlite::params![d, x, bot]).unwrap();
            let _ = i;
        }
        let v = overview(&c, json!({})).unwrap();
        let manual = v["attribution"].as_array().unwrap().iter().find(|r| r["source"] == "Manual").unwrap();
        assert_eq!(manual["net_pnl"], "10");
        let bot = v["attribution"].as_array().unwrap().iter().find(|r| r["source"] != "Manual").unwrap();
        assert_eq!((bot["net_pnl"].as_str(), bot["trades"].as_u64()), (Some("10"), Some(2)));
        let weeks = v["reports"]["week"].as_array().unwrap();
        // 1 Nov 2026 is a Sunday (ISO week 44), 2 Nov a Monday (week 45), 9 Nov week 46
        assert_eq!(weeks.iter().map(|w| (w["period"].as_str().unwrap(), w["net_pnl"].as_str().unwrap())).collect::<Vec<_>>(), vec![("2026-W44", "10"), ("2026-W45", "-10"), ("2026-W46", "20")]);
        assert_eq!(v["allocation_proposal"]["applied"], false);
        // reconciliation flags a broker that holds less than the bots think
        c.execute("INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived) VALUES ('bn-test-1','binance','T','testnet','active','x',0)", []).unwrap();
        c.execute("INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at, account_id) VALUES ('crypto','live','BTCUSDT','long',0.5,100,0.1,'x','bn-test-1')", []).unwrap();
        let r = reconcile(&c, "bn-test-1", &json!({ "balances": [{ "asset": "BTC", "free": "0.2", "locked": "0.1" }] }));
        assert_eq!(r["ok"], false);
        assert!((r["rows"][0]["short_by"].as_f64().unwrap() - 0.2).abs() < 1e-9);
        assert!(trades_csv(&c).unwrap().lines().count() == 4);
    }
}
