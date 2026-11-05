//! AI Trade Explainer, evidence first.
//!
//! `facts` builds a fact sheet for one closed trade from stored records only: the trade and its P&L, the bot and
//! strategy version (with the rules that were in force), the orders with the engine's own reasons, broker fills
//! and slippage, the market regime recorded nearest the entry, news and economic events around the trade date,
//! and risk rejections that day. Every fact carries its source. Missing evidence is listed as missing, never
//! guessed. An optional model interpretation (`ai::explain_trade`) is labelled as such, may use only these
//! facts, and its numbers are checked against them.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn fact(text: String, source: &str) -> Value {
    json!({ "fact": text, "source": source })
}

fn rules(spec: &Value) -> Vec<String> {
    let mut out = vec![];
    for k in ["entry", "exit"] {
        if let Some(o) = spec[k].as_object() {
            for (side, c) in o {
                out.push(format!("{k} {side}: {}", c));
            }
        }
    }
    for k in ["stop_loss", "take_profit", "trailing_stop", "time_exit"] {
        if !spec[k].is_null() {
            out.push(format!("{k}: {}", spec[k]));
        }
    }
    out
}

pub fn facts(conn: &Connection, trade_id: i64) -> Result<Value, String> {
    let d = crate::attribution::trade_detail(conn, trade_id)?;
    let t = &d["trade"];
    let sym = t["symbol"].as_str().unwrap_or("").to_string();
    let date = t["date"].as_str().unwrap_or("").to_string();
    let mut f = vec![
        fact(format!("{} {} {} x {}: entry {}, exit {}", sym, t["side"].as_str().unwrap_or(""), t["account_id"].as_str().unwrap_or(""), t["qty"], t["entry"], t["exit"]), "trades"),
        fact(format!("gross P&L {}, fees {}, net P&L {}", t["gross_pnl"].as_str().unwrap_or("?"), t["fees"], t["net_pnl"].as_str().unwrap_or("?")), "trades (attribution)"),
        fact(format!("exit reason recorded: {}", t["exit_reason"].as_str().unwrap_or("?")), "trades.notes"),
    ];
    let mut missing = vec![];
    match t["bot_id"].as_i64() {
        Some(b) => f.push(fact(format!("opened by bot #{b} ({})", t["bot"].as_str().unwrap_or("")), "bots")),
        None => f.push(fact("a manual trade (no bot)".into(), "trades")),
    }
    match d["strategy_version"].as_object() {
        Some(v) => {
            f.push(fact(format!("strategy version #{} (v{}) hash {}", v.get("id").unwrap_or(&Value::Null), v.get("version").unwrap_or(&Value::Null), v.get("hash").and_then(Value::as_str).unwrap_or("")), "strategy_versions"));
            for r in rules(v.get("spec").unwrap_or(&Value::Null)) {
                f.push(fact(format!("rule in force: {r}"), "strategy_versions.spec"));
            }
        }
        None => missing.push("strategy version (trade predates version links, or it was manual)"),
    }
    let mut entry_bar = None;
    for o in d["orders"].as_array().into_iter().flatten() {
        if entry_bar.is_none() && o["action"] == "open" {
            entry_bar = o["bar_time"].as_i64();
        }
        f.push(fact(format!("order {} {} {} @ {} ({}): {}", o["action"].as_str().unwrap_or(""), o["side"].as_str().unwrap_or(""), o["qty"], o["price"], o["status"].as_str().unwrap_or(""), o["reason"].as_str().unwrap_or("")), "bot_orders"));
    }
    for b in d["broker_fills"].as_array().into_iter().flatten() {
        f.push(fact(format!("broker fill {} avg {} vs reference {} (slippage against you {}%)", b["client_order_id"].as_str().unwrap_or(""), b["avg_fill_price"], b["reference_price"], b["slippage_pct_against_you"]), "broker_orders"));
    }
    f.push(fact(format!("slippage: {}", d["slippage"].as_str().unwrap_or("")), "attribution"));
    // regime nearest the entry bar (any interval), else the latest that day
    let regime: Option<(String, f64, String, i64)> = conn
        .query_row(
            "SELECT regime, confidence, interval, bar_time FROM regime_observations WHERE symbol = ?1 AND (?2 IS NULL OR bar_time <= ?2) ORDER BY bar_time DESC LIMIT 1",
            params![sym, entry_bar],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    match regime {
        Some((rg, c, iv, bt)) => f.push(fact(format!("regime {rg} (confidence {c}, {iv} bars, observed at bar {bt}, {})", crate::regime::RULE_VERSION), "regime_observations")),
        None => missing.push("market regime (classify the symbol in Research Lab to record one)"),
    }
    let news: Vec<(String, String, Option<f64>, String)> = conn
        .prepare("SELECT title, published_utc, sentiment, provider FROM news_items WHERE (',' || symbols || ',') LIKE '%,' || ?1 || ',%' AND substr(published_utc, 1, 10) BETWEEN date(?2, '-1 day') AND date(?2, '+1 day') ORDER BY published_utc LIMIT 10")
        .and_then(|mut st| st.query_map(params![sym, date], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map(|m| m.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    if news.is_empty() {
        missing.push("news around the trade date (none imported)");
    }
    for (title, at, s, p) in news {
        f.push(fact(format!("news {at} [{p}]: {title} (heuristic sentiment {})", s.map_or("n/a".into(), |x| x.to_string())), "news_items"));
    }
    let evs: Vec<(String, String, String, String)> = conn
        .prepare("SELECT name, region, scheduled_utc, importance FROM econ_events WHERE substr(scheduled_utc, 1, 10) BETWEEN date(?1, '-1 day') AND date(?1, '+1 day') AND importance <> 'low' ORDER BY scheduled_utc LIMIT 10")
        .and_then(|mut st| st.query_map([&date], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map(|m| m.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    for (n, rg, at, imp) in evs {
        f.push(fact(format!("economic event {n} ({rg}, {imp}) at {at}"), "econ_events"));
    }
    let rejects: i64 = conn.query_row("SELECT count(*) FROM events WHERE type = 'OrderRiskRejected' AND substr(ts_utc, 1, 10) = ?1 AND payload LIKE '%' || ?2 || '%'", params![date, sym], |r| r.get(0)).unwrap_or(0);
    if rejects > 0 {
        f.push(fact(format!("{rejects} order(s) for {sym} were refused by risk checks that day"), "events"));
    }
    Ok(json!({
        "trade_id": trade_id, "facts": f, "missing_evidence": missing,
        "label": "Facts from ZoneBoard's own records. Correlation with news or events is shown, not claimed as cause.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_sheet_cites_sources_and_lists_gaps() {
        let c = crate::db::open(":memory:").unwrap();
        c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode, account_id) VALUES ('2026-11-02','BTCUSDT','long',1,100,110,0.5,'Paper trade','crypto','demo','demo-1')", []).unwrap();
        let id = c.last_insert_rowid();
        let f = facts(&c, id).unwrap();
        assert!(f["facts"].as_array().unwrap().iter().all(|x| x["source"].is_string()));
        assert!(f["facts"].to_string().contains("net P&L 9.5"), "{f}");
        let missing = f["missing_evidence"].to_string();
        assert!(missing.contains("regime") && missing.contains("news"));
        crate::intel::ingest_news(&c, "manual", &[json!({ "id": "n", "title": "BTC rallies", "published_utc": "2026-11-02T10:00:00Z", "symbols": ["BTCUSDT"] })]).unwrap();
        let f = facts(&c, id).unwrap();
        assert!(f["facts"].to_string().contains("BTC rallies"));
        assert!(facts(&c, 999).is_err());
    }
}
