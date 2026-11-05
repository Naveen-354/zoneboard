//! Trading infrastructure monitor: one status per component, derived from real state, never hard-coded.
//!
//! Components: the bot runner (heartbeat), each broker connection (last successful sync, last error), each
//! active bot's market-data feed (age of its latest price against its timeframe) and worker health (last
//! error, repeated refusals), the broker-order outbox (orders stuck submitting or unknown), background jobs
//! (recent failures), and the HTTP API (error rate and latency from the request metrics).
//!
//! Each problem becomes an **incident** keyed by component and kind; repeats bump its count instead of adding
//! rows, and it resolves itself when the condition clears. Thresholds are configurable (`kv_config`).

use std::collections::VecDeque;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

const CONFIG_KEY: &str = "monitor.thresholds";

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn age_s(iso: Option<&str>) -> Option<i64> {
    iso.and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| (now() - t.with_timezone(&chrono::Utc)).num_seconds())
}

/// Request metrics kept in memory (the last 2000 API requests).
#[derive(Default)]
pub struct Metrics {
    recent: Mutex<VecDeque<(u64, u16)>>,
}

impl Metrics {
    pub fn record(&self, ms: u64, status: u16) {
        let mut r = self.recent.lock().unwrap();
        if r.len() >= 2000 {
            r.pop_front();
        }
        r.push_back((ms, status));
    }
    pub fn summary(&self) -> Value {
        let r = self.recent.lock().unwrap();
        let mut ms: Vec<u64> = r.iter().map(|x| x.0).collect();
        ms.sort_unstable();
        let p = |q: f64| if ms.is_empty() { None } else { Some(ms[((ms.len() - 1) as f64 * q).round() as usize]) };
        let errors = r.iter().filter(|x| x.1 >= 500).count();
        json!({ "requests": r.len(), "server_errors": errors, "error_rate_pct": if r.is_empty() { 0.0 } else { (errors as f64 / r.len() as f64 * 10000.0).round() / 100.0 }, "p50_ms": p(0.5), "p95_ms": p(0.95), "max_ms": ms.last() })
    }
}

pub fn thresholds(conn: &Connection) -> Value {
    let defaults = json!({ "runner_degraded_s": 90, "runner_unavailable_s": 300, "broker_sync_degraded_s": 900, "feed_stale_intervals": 3, "stuck_order_s": 60, "rejects_per_hour": 5, "api_error_rate_pct": 5.0 });
    let mut v = defaults.clone();
    if let Some(saved) = crate::events::kv_get(conn, CONFIG_KEY).and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
        for (k, x) in saved.as_object().into_iter().flatten() {
            if defaults.get(k).is_some() && x.is_number() {
                v[k] = x.clone();
            }
        }
    }
    v
}

pub fn set_thresholds(conn: &Connection, body: &Value) -> Result<Value, String> {
    let cur = thresholds(conn);
    let mut next = cur.clone();
    for (k, x) in body.as_object().ok_or("body must be an object")? {
        if cur.get(k).is_none() {
            return Err(format!("unknown threshold '{k}'"));
        }
        match x.as_f64() {
            Some(n) if n > 0.0 && n < 1e7 => next[k] = x.clone(),
            _ => return Err(format!("{k} must be a positive number")),
        }
    }
    crate::events::kv_set(conn, CONFIG_KEY, &next.to_string());
    let _ = crate::audit::record(conn, "user", "monitor_thresholds_updated", "monitor", &next);
    Ok(next)
}

fn comp(id: &str, kind: &str, name: &str, status: &str, detail: String, last_ok: Option<String>) -> Value {
    json!({ "id": id, "kind": kind, "name": name, "status": status, "detail": detail, "last_ok_utc": last_ok })
}

/// Evaluate every component now. Pure read: incidents are updated by [`record_incidents`].
pub fn evaluate(conn: &Connection, metrics: &Metrics) -> Vec<Value> {
    let t = thresholds(conn);
    let num = |k: &str| t[k].as_f64().unwrap_or(0.0);
    let mut out = vec![];
    // runner
    let hb: Option<(String, String)> = conn.query_row("SELECT heartbeat_utc, detail FROM workers WHERE name = 'bot-runner'", [], |r| Ok((r.get(0)?, r.get(1)?))).optional().ok().flatten();
    let runner = match hb.as_ref().and_then(|h| age_s(Some(&h.0))) {
        None => comp("runner", "worker", "Bot runner", "warning", "no heartbeat recorded yet (it beats every 20 s once the server runs)".into(), None),
        Some(a) if (a as f64) < num("runner_degraded_s") => comp("runner", "worker", "Bot runner", "ok", format!("last beat {a} s ago"), hb.clone().map(|h| h.0)),
        Some(a) if (a as f64) < num("runner_unavailable_s") => comp("runner", "worker", "Bot runner", "degraded", format!("last beat {a} s ago"), hb.clone().map(|h| h.0)),
        Some(a) => comp("runner", "worker", "Bot runner", "unavailable", format!("no heartbeat for {a} s"), hb.clone().map(|h| h.0)),
    };
    out.push(runner);
    // broker connections
    if let Ok(mut st) = conn.prepare("SELECT id, label, environment, last_sync_utc, last_error FROM accounts WHERE kind <> 'demo' AND archived = 0") {
        let rows: Vec<(String, String, String, Option<String>, Option<String>)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map(|m| m.filter_map(Result::ok).collect()).unwrap_or_default();
        for (id, label, env, sync, error) in rows {
            let a = age_s(sync.as_deref());
            let (status, detail) = match (&error, a) {
                (Some(e), _) if e.contains("withdraw") => ("unavailable", e.clone()),
                (Some(e), _) => ("degraded", format!("last error: {e}")),
                (None, None) => ("warning", "never synced".to_string()),
                (None, Some(s)) if (s as f64) > num("broker_sync_degraded_s") => ("warning", format!("last successful sync {s} s ago (balances refresh when viewed or traded)")),
                (None, Some(s)) => ("ok", format!("synced {s} s ago")),
            };
            out.push(comp(&format!("broker:{id}"), "broker", &format!("{label} ({env})"), status, detail, sync));
        }
    }
    // bots: feed freshness, worker errors, repeated refusals
    if let Ok(mut st) = conn.prepare("SELECT id, name, symbol, interval, status, last_price_utc, last_error FROM bots WHERE status IN ('running','paused')") {
        let rows: Vec<(i64, String, String, String, String, Option<String>, Option<String>)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map(|m| m.filter_map(Result::ok).collect()).unwrap_or_default();
        for (id, name, sym, iv, status, price_t, err) in rows {
            let ivs = crate::bots::interval_secs(&iv).unwrap_or(3600) as f64;
            let feed = match age_s(price_t.as_deref()) {
                None => ("warning", "no price received yet".to_string()),
                Some(a) if (a as f64) > num("feed_stale_intervals") * ivs => ("degraded", format!("latest price is {a} s old ({iv} candles)")),
                Some(a) => ("ok", format!("price {a} s old")),
            };
            out.push(comp(&format!("feed:{id}"), "market_data", &format!("{sym} {iv} feed for {name}"), feed.0, feed.1, price_t.clone()));
            let rejects: i64 = conn.query_row("SELECT count(*) FROM bot_orders WHERE bot_id = ?1 AND status = 'rejected' AND ts_utc > ?2", params![id, (now() - chrono::Duration::hours(1)).to_rfc3339()], |r| r.get(0)).unwrap_or(0);
            let w = if let Some(e) = &err {
                ("degraded", format!("{status}; last error: {e}"))
            } else if rejects as f64 >= num("rejects_per_hour") {
                ("warning", format!("{rejects} refused orders in the last hour"))
            } else {
                ("ok", format!("{status}; {rejects} refused order(s) in the last hour"))
            };
            out.push(comp(&format!("bot:{id}"), "worker", &format!("Bot {name}"), w.0, w.1, None));
        }
    }
    // order lifecycle: broker orders stuck or unknown
    let cutoff = (now() - chrono::Duration::seconds(num("stuck_order_s") as i64)).to_rfc3339();
    let (stuck, unknown): (i64, i64) = conn
        .query_row("SELECT (SELECT count(*) FROM broker_orders WHERE status = 'submitting' AND updated_utc < ?1), (SELECT count(*) FROM broker_orders WHERE status = 'unknown')", [cutoff], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap_or((0, 0));
    out.push(match (stuck, unknown) {
        (0, 0) => comp("orders", "orders", "Broker order lifecycle", "ok", "no order waiting or in an unknown state".into(), None),
        (_, u) if u > 0 => comp("orders", "orders", "Broker order lifecycle", "unavailable", format!("{u} order(s) with an unknown outcome: reconcile before trading"), None),
        (s, _) => comp("orders", "orders", "Broker order lifecycle", "degraded", format!("{s} order(s) stuck submitting"), None),
    });
    // jobs
    let failed: i64 = conn.query_row("SELECT count(*) FROM jobs WHERE status = 'failed' AND finished_utc > ?1", [(now() - chrono::Duration::hours(24)).to_rfc3339()], |r| r.get(0)).unwrap_or(0);
    out.push(comp("jobs", "worker", "Background jobs", if failed > 0 { "warning" } else { "ok" }, format!("{failed} failed in the last 24 h"), None));
    // api
    let m = metrics.summary();
    let er = m["error_rate_pct"].as_f64().unwrap_or(0.0);
    out.push(comp("api", "api", "Local API", if er > num("api_error_rate_pct") { "degraded" } else { "ok" }, format!("{} requests, {}% server errors, p95 {} ms", m["requests"], er, m["p95_ms"]), None));
    out
}

/// Open, bump or resolve incidents to match the current evaluation. Returns how many are open.
pub fn record_incidents(conn: &Connection, comps: &[Value]) -> i64 {
    let t = now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    for c in comps {
        let key = c["id"].as_str().unwrap_or("");
        let status = c["status"].as_str().unwrap_or("ok");
        if status == "ok" {
            let _ = conn.execute("UPDATE incidents SET status = 'resolved', resolved_utc = ?1 WHERE dedupe_key = ?2 AND status = 'open'", params![t, key]);
            continue;
        }
        let n = conn
            .execute("UPDATE incidents SET count = count + 1, last_utc = ?1, severity = ?2, detail = ?3 WHERE dedupe_key = ?4 AND status = 'open'", params![t, status, c["detail"].as_str(), key])
            .unwrap_or(0);
        if n == 0 {
            let _ = conn.execute(
                "INSERT INTO incidents (dedupe_key, component, severity, title, detail, first_utc, last_utc) VALUES (?1,?2,?3,?4,?5,?6,?6)",
                params![key, c["kind"].as_str(), status, format!("{}: {}", c["name"].as_str().unwrap_or(""), status), c["detail"].as_str(), t],
            );
            crate::events::notify(conn, "monitor", &format!("{} is {status}", c["name"].as_str().unwrap_or("")), c["detail"].as_str().unwrap_or(""), Some(&format!("incident:{key}")));
            if c["kind"] == "market_data" {
                crate::events::emit(conn, "DataFeedStale", key, json!({ "component": key, "detail": c["detail"] }), None, Some(&format!("stale:{key}:{t}")));
            }
            if c["kind"] == "broker" {
                crate::events::emit(conn, "BrokerConnectionChanged", key, json!({ "component": key, "status": status }), None, Some(&format!("broker:{key}:{t}")));
            }
        }
    }
    conn.query_row("SELECT count(*) FROM incidents WHERE status = 'open'", [], |r| r.get(0)).unwrap_or(0)
}

pub fn overview(conn: &Connection, metrics: &Metrics) -> Value {
    let comps = evaluate(conn, metrics);
    let worst = ["unavailable", "degraded", "warning"].into_iter().find(|s| comps.iter().any(|c| c["status"] == *s)).unwrap_or("ok");
    let incidents: Vec<Value> = conn
        .prepare("SELECT id, dedupe_key, component, severity, title, detail, first_utc, last_utc, count, status, resolved_utc FROM incidents ORDER BY id DESC LIMIT 100")
        .and_then(|mut st| {
            st.query_map([], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "key": r.get::<_, String>(1)?, "component": r.get::<_, String>(2)?, "severity": r.get::<_, String>(3)?, "title": r.get::<_, String>(4)?, "detail": r.get::<_, String>(5)?, "first_utc": r.get::<_, String>(6)?, "last_utc": r.get::<_, String>(7)?, "count": r.get::<_, i64>(8)?, "status": r.get::<_, String>(9)?, "resolved_utc": r.get::<_, Option<String>>(10)? })))
                .map(|m| m.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    json!({ "overall": worst, "components": comps, "incidents": incidents, "metrics": metrics.summary(), "thresholds": thresholds(conn), "evaluated_utc": now().to_rfc3339() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_come_from_state_and_incidents_group_and_resolve() {
        let c = crate::db::open(":memory:").unwrap();
        let m = Metrics::default();
        // no heartbeat yet
        assert_eq!(evaluate(&c, &m)[0]["status"], "warning");
        let old = (now() - chrono::Duration::seconds(400)).to_rfc3339();
        c.execute("INSERT INTO workers (name, started_utc, heartbeat_utc) VALUES ('bot-runner', ?1, ?1)", [&old]).unwrap();
        assert_eq!(evaluate(&c, &m)[0]["status"], "unavailable");
        // a broker account with an error, a stale bot feed, an unknown order
        c.execute("INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived, last_error) VALUES ('bn-test-1','binance','B','testnet','active','x',0,'rate limited (429) by the exchange')", []).unwrap();
        let spec = crate::strategy::templates().into_iter().find(|t| t["id"] == "rsi").unwrap()["spec"].clone();
        let mut conn = c;
        let id = crate::bots::create(&mut conn, &json!({ "name": "b", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "100" })).unwrap();
        conn.execute("UPDATE bots SET status = 'running', last_price_utc = ?1 WHERE id = ?2", params![(now() - chrono::Duration::hours(5)).to_rfc3339(), id]).unwrap();
        conn.execute("INSERT INTO broker_orders (bot_id, account_id, client_order_id, symbol, side, action, qty, status, created_utc, updated_utc) VALUES (1,'bn-test-1','zb1-1-0','BTCUSDT','BUY','open','1','unknown','x','x')", []).unwrap();
        let comps = evaluate(&conn, &m);
        let st = |id: &str| comps.iter().find(|x| x["id"] == id).unwrap()["status"].as_str().unwrap().to_string();
        assert_eq!(st("broker:bn-test-1"), "degraded");
        assert_eq!(st(&format!("feed:{id}")), "degraded");
        assert_eq!(st("orders"), "unavailable");
        let open = record_incidents(&conn, &comps);
        assert!(open >= 4);
        record_incidents(&conn, &comps);
        let (rows, count): (i64, i64) = conn.query_row("SELECT count(*), max(count) FROM incidents", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((rows, count), (open, 2), "repeats are grouped");
        // the order is reconciled: its incident resolves
        conn.execute("UPDATE broker_orders SET status = 'filled'", []).unwrap();
        record_incidents(&conn, &evaluate(&conn, &m));
        let s: String = conn.query_row("SELECT status FROM incidents WHERE dedupe_key = 'orders'", [], |r| r.get(0)).unwrap();
        assert_eq!(s, "resolved");
        assert!(set_thresholds(&conn, &json!({ "nope": 1 })).is_err());
        assert_eq!(set_thresholds(&conn, &json!({ "stuck_order_s": 120 })).unwrap()["stuck_order_s"], 120);
        let stale: i64 = conn.query_row("SELECT count(*) FROM events WHERE type = 'DataFeedStale'", [], |r| r.get(0)).unwrap();
        assert_eq!(stale, 1);
    }
}
