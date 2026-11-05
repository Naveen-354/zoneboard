//! Strategy drift: is a bot's recent paper or broker performance materially different from what its validated
//! baseline (the latest saved backtest of the same strategy version) led us to expect?
//!
//! Compared on the bot's most recent closed trades (window, default 30): win rate, profit factor, average net
//! trade relative to capital, average holding time, trade frequency and drawdown. Below the minimum sample
//! (default 10 trades) the answer is `insufficient_data`, never a verdict. Thresholds are configurable.
//!
//! Drift never changes a strategy. It can pause a bot only when the owner has switched auto-pause on for that
//! bot; the pause is audited and undone by resuming the bot.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

const LEVELS: [&str; 3] = ["healthy", "watch", "degraded"];

pub fn config(conn: &Connection) -> Value {
    let d = json!({ "window_trades": 30, "min_trades": 10, "win_rate_drop_watch": 15.0, "win_rate_drop_degraded": 25.0, "pf_ratio_watch": 0.7, "pf_ratio_degraded": 0.5, "frequency_ratio_min": 0.33, "frequency_ratio_max": 3.0, "drawdown_ratio_degraded": 1.5 });
    let mut v = d.clone();
    if let Some(s) = crate::events::kv_get(conn, "drift.config").and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        for (k, x) in s.as_object().into_iter().flatten() {
            if d.get(k).is_some() && x.is_number() {
                v[k] = x.clone();
            }
        }
    }
    v
}

pub fn set_config(conn: &Connection, body: &Value) -> Result<Value, String> {
    let mut v = config(conn);
    for (k, x) in body.as_object().ok_or("body must be an object")? {
        if v.get(k).is_none() || !x.as_f64().is_some_and(|n| n > 0.0) {
            return Err(format!("{k}: unknown or not a positive number"));
        }
        v[k] = x.clone();
    }
    crate::events::kv_set(conn, "drift.config", &v.to_string());
    Ok(v)
}

pub fn set_autopause(conn: &Connection, bot: i64, on: bool) -> Result<(), String> {
    crate::bots::load(conn, bot).map_err(|e| format!("{e:?}"))?;
    crate::events::kv_set(conn, &format!("drift.autopause.{bot}"), if on { "1" } else { "0" });
    crate::audit::record(conn, "user", "drift_autopause_set", &format!("bot:{bot}"), &json!({ "bot_id": bot, "enabled": on })).map_err(|e| e.to_string())?;
    Ok(())
}

fn stats(pnls: &[f64], bars: &[f64], capital: f64) -> Value {
    let n = pnls.len().max(1) as f64;
    let wins: Vec<f64> = pnls.iter().copied().filter(|p| *p > 0.0).collect();
    let gl: f64 = -pnls.iter().copied().filter(|p| *p <= 0.0).sum::<f64>();
    let (mut eq, mut peak, mut dd) = (capital, capital, 0.0f64);
    for p in pnls {
        eq += p;
        peak = peak.max(eq);
        if peak > 0.0 {
            dd = dd.max((peak - eq) / peak * 100.0);
        }
    }
    json!({ "trades": pnls.len(), "win_rate": wins.len() as f64 / n * 100.0, "profit_factor": if gl > 0.0 { Some(wins.iter().sum::<f64>() / gl) } else { None }, "avg_net_pct_of_capital": pnls.iter().sum::<f64>() / n / capital * 100.0, "avg_bars_held": if bars.is_empty() { None } else { Some(bars.iter().sum::<f64>() / bars.len() as f64) }, "max_drawdown_pct": dd })
}

/// Evaluate one bot. Stores an observation and emits `StrategyDriftDetected` when the status worsens.
pub fn evaluate(conn: &Connection, bot: i64) -> Result<Value, String> {
    let cfg = config(conn);
    let b = crate::bots::load(conn, bot).map_err(|e| format!("{e:?}"))?;
    let alloc: f64 = rust_decimal::prelude::ToPrimitive::to_f64(&b.allocation).unwrap_or(1.0).max(1e-9);
    let vid: Option<i64> = conn.query_row("SELECT strategy_version_id FROM bots WHERE id = ?1", [bot], |r| r.get(0)).map_err(|e| e.to_string())?;
    let base: Option<(i64, String, String)> = conn
        .query_row("SELECT id, metrics, report FROM backtests WHERE bot_id = ?1 AND (?2 IS NULL OR strategy_version_id = ?2) ORDER BY id DESC LIMIT 1", params![bot, vid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    let window = cfg["window_trades"].as_u64().unwrap_or(30) as i64;
    let mut st = conn.prepare("SELECT side, qty, entry, exit, fees, date FROM trades WHERE bot_id = ?1 ORDER BY id DESC LIMIT ?2").map_err(|e| e.to_string())?;
    let recent: Vec<(f64, String)> = st
        .query_map(params![bot, window], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?, r.get::<_, String>(5)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|(s, q, e, x, f, d)| crate::accounts::realised(&s, q, e, x, f).and_then(|p| rust_decimal::prelude::ToPrimitive::to_f64(&p)).map(|p| (p, d)))
        .collect();
    let min = cfg["min_trades"].as_u64().unwrap_or(10) as usize;
    let mut checks = vec![];
    let status;
    let base_v = base.as_ref().map(|(_, m, _)| serde_json::from_str::<Value>(m).unwrap_or(Value::Null));
    let pn: Vec<f64> = recent.iter().rev().map(|x| x.0).collect();
    let live = stats(&pn, &[], alloc);
    match (&base_v, recent.len()) {
        (None, _) => status = "insufficient_data",
        (_, n) if n < min => status = "insufficient_data",
        (Some(bm), _) => {
            let mut level = 0; // 0 healthy, 1 watch, 2 degraded
            let mut flag = |name: &str, base: Value, now: Value, lvl: i32, why: String| {
                checks.push(json!({ "metric": name, "baseline": base, "recent": now, "status": LEVELS[lvl as usize], "why": why }));
                level = level.max(lvl);
            };
            let bwr = bm["win_rate"].as_f64().unwrap_or(0.0);
            let lwr = live["win_rate"].as_f64().unwrap_or(0.0);
            let drop = bwr - lwr;
            let l = if drop >= cfg["win_rate_drop_degraded"].as_f64().unwrap() { 2 } else if drop >= cfg["win_rate_drop_watch"].as_f64().unwrap() { 1 } else { 0 };
            flag("win_rate", json!(bwr), json!((lwr * 100.0).round() / 100.0), l, format!("{drop:.1} points below the baseline"));
            if let (Some(bp), Some(lp)) = (bm["profit_factor"].as_f64(), live["profit_factor"].as_f64()) {
                let r = lp / bp;
                let l = if r <= cfg["pf_ratio_degraded"].as_f64().unwrap() { 2 } else if r <= cfg["pf_ratio_watch"].as_f64().unwrap() { 1 } else { 0 };
                flag("profit_factor", json!(bp), json!((lp * 1000.0).round() / 1000.0), l, format!("{:.0}% of the baseline", r * 100.0));
            }
            let bexp = bm["expectancy"].as_f64().unwrap_or(0.0) / alloc * 100.0;
            let lexp = live["avg_net_pct_of_capital"].as_f64().unwrap_or(0.0);
            flag("avg_trade_pct_of_capital", json!(bexp), json!(lexp), if bexp > 0.0 && lexp < 0.0 { 1 } else { 0 }, "a positive expectancy turned negative is a warning".into());
            let bdd = bm["max_drawdown_pct"].as_f64().unwrap_or(0.0);
            let ldd = live["max_drawdown_pct"].as_f64().unwrap_or(0.0);
            flag("max_drawdown_pct", json!(bdd), json!(ldd), if bdd > 0.0 && ldd > bdd * cfg["drawdown_ratio_degraded"].as_f64().unwrap() { 2 } else { 0 }, "recent drawdown versus the backtest's".into());
            // trade frequency: trades per day in the window versus the backtest's trades per day
            if let (Some(first), Some(last)) = (recent.last(), recent.first()) {
                let days = (chrono::NaiveDate::parse_from_str(&last.1, "%Y-%m-%d").ok(), chrono::NaiveDate::parse_from_str(&first.1, "%Y-%m-%d").ok());
                let base_bars = bm["bars"].as_f64().unwrap_or(0.0);
                let base_trades = bm["trades"].as_f64().unwrap_or(0.0);
                let span_days = base.as_ref().and_then(|(_, _, rep)| serde_json::from_str::<Value>(rep).ok()).and_then(|r| Some((r["data"]["to"].as_i64()? - r["data"]["from"].as_i64()?) as f64 / 86400.0));
                if let (Some(sd), (Some(a), Some(z))) = (span_days, days) {
                    let live_days = ((a - z).num_days().max(1)) as f64;
                    let (bf, lf) = (base_trades / sd.max(1.0), recent.len() as f64 / live_days);
                    if bf > 0.0 && base_bars > 0.0 {
                        let r = lf / bf;
                        let l = if r < cfg["frequency_ratio_min"].as_f64().unwrap() || r > cfg["frequency_ratio_max"].as_f64().unwrap() { 1 } else { 0 };
                        flag("trades_per_day", json!((bf * 1000.0).round() / 1000.0), json!((lf * 1000.0).round() / 1000.0), l, format!("{r:.2}x the backtest's frequency (a signal-frequency change)"));
                    }
                }
            }
            status = LEVELS[level as usize];
        }
    }
    let prev: Option<String> = conn.query_row("SELECT status FROM drift_observations WHERE bot_id = ?1 ORDER BY id DESC LIMIT 1", [bot], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    let detail = json!({ "status": status, "baseline_backtest_id": base.as_ref().map(|b| b.0), "baseline": base_v, "recent": live, "window_trades": recent.len(), "min_trades": min, "checks": checks, "config": cfg });
    let t = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    conn.execute("INSERT INTO drift_observations (bot_id, created_utc, status, detail) VALUES (?1,?2,?3,?4)", params![bot, t, status, detail.to_string()]).map_err(|e| e.to_string())?;
    let worse = matches!(status, "watch" | "degraded") && prev.as_deref() != Some(status);
    let mut paused = false;
    if worse {
        crate::events::emit(conn, "StrategyDriftDetected", &format!("bot:{bot}"), json!({ "bot_id": bot, "status": status, "checks": checks }), None, None);
        crate::events::notify(conn, "drift", &format!("Bot {} looks {status}", b.name), "recent results differ from its backtest baseline", Some(&format!("drift:{bot}:{status}")));
        let auto = crate::events::kv_get(conn, &format!("drift.autopause.{bot}")).as_deref() == Some("1");
        if auto && status == "degraded" && b.status == "running" {
            let _ = crate::bots::set_status(conn, bot, "paused", "auto-paused by drift detection (you enabled it); resume to undo", "system");
            paused = true;
        }
    }
    let mut out = detail;
    out["auto_paused"] = json!(paused);
    out["evaluated_utc"] = json!(t);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(results: &[f64], baseline_win_rate: f64) -> (Connection, i64) {
        let mut c = crate::db::open(":memory:").unwrap();
        let spec = crate::strategy::templates().into_iter().find(|t| t["id"] == "rsi").unwrap()["spec"].clone();
        let id = crate::bots::create(&mut c, &json!({ "name": "d", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "1000" })).unwrap();
        let vid: i64 = c.query_row("SELECT strategy_version_id FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        let metrics = json!({ "win_rate": baseline_win_rate, "profit_factor": 2.0, "expectancy": 5.0, "max_drawdown_pct": 4.0, "trades": 60, "bars": 1000 });
        let report = json!({ "data": { "from": 1_700_000_000, "to": 1_700_000_000 + 60 * 86400 } });
        c.execute("INSERT INTO backtests (bot_id, created_utc, request, metrics, report, strategy_version_id) VALUES (?1,'x','{}',?2,?3,?4)", params![id, metrics.to_string(), report.to_string(), vid]).unwrap();
        for (i, p) in results.iter().enumerate() {
            let exit = 100.0 + p;
            c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode, account_id, bot_id) VALUES (?1,'BTCUSDT','long',1,100,?2,0,'','crypto','demo','demo-1',?3)", params![format!("2026-10-{:02}", 1 + i.min(30)), exit, id]).unwrap();
        }
        (c, id)
    }

    #[test]
    fn synthetic_drift_states() {
        let (c, id) = setup(&[5.0; 4], 55.0);
        assert_eq!(evaluate(&c, id).unwrap()["status"], "insufficient_data", "4 trades are too few");
        // 30 trades, 20% winners, against a 55% baseline: degraded
        let mut bad = vec![-3.0; 24];
        bad.extend([6.0; 6]);
        let (c, id) = setup(&bad, 55.0);
        let r = evaluate(&c, id).unwrap();
        assert_eq!(r["status"], "degraded", "{r}");
        assert!(r["checks"].as_array().unwrap().iter().any(|x| x["metric"] == "win_rate" && x["status"] == "degraded"));
        let n: i64 = c.query_row("SELECT count(*) FROM events WHERE type = 'StrategyDriftDetected'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // the same status again: no second event
        evaluate(&c, id).unwrap();
        let n: i64 = c.query_row("SELECT count(*) FROM events WHERE type = 'StrategyDriftDetected'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // results in line with the baseline: healthy
        let mut ok = vec![4.0; 17];
        ok.extend([-2.0; 13]);
        let (c, id) = setup(&ok, 55.0);
        assert_eq!(evaluate(&c, id).unwrap()["status"], "healthy");
    }

    #[test]
    fn autopause_only_when_enabled() {
        let mut bad = vec![-3.0; 24];
        bad.extend([6.0; 6]);
        let (c, id) = setup(&bad, 55.0);
        c.execute("UPDATE bots SET status = 'running' WHERE id = ?1", [id]).unwrap();
        assert_eq!(evaluate(&c, id).unwrap()["auto_paused"], false, "off by default");
        c.execute("DELETE FROM drift_observations", []).unwrap();
        set_autopause(&c, id, true).unwrap();
        assert_eq!(evaluate(&c, id).unwrap()["auto_paused"], true);
        assert_eq!(crate::bots::load(&c, id).unwrap().status, "paused");
    }
}
