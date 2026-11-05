//! Durable, typed event log: the one event bus of this single-process app (ADR-005).
//!
//! Every event has a type from [`TYPES`], a schema version, a UTC timestamp, an optional correlation id and an
//! optional idempotency key (a second emit with the same key is ignored, so retries and replays cannot create
//! duplicates). Payloads carry identifiers and numbers only, never credentials. Consumers (workflows, the
//! monitor, notifications) read by increasing id and remember the last id they handled.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const VERSION: i64 = 1;

pub const TYPES: [&str; 22] = [
    "MarketDataUpdated", "MarketRegimeChanged", "StrategyVersionCreated", "BacktestCompleted", "WalkForwardCompleted",
    "RiskLimitBreached", "PortfolioExposureChanged", "OrderProposed", "OrderRiskApproved", "OrderRiskRejected",
    "OrderSubmitted", "OrderPartiallyFilled", "OrderFilled", "OrderRejected", "BotStateChanged", "StrategyDriftDetected",
    "BrokerConnectionChanged", "DataFeedStale", "EconomicEventApproaching", "NewsSentimentChanged", "WorkflowTriggered",
    "WorkflowActionCompleted",
];

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Record an event. Returns its id, or `None` when the idempotency key was already used (or the type is unknown,
/// which is a programming error and is logged).
pub fn emit(conn: &Connection, ty: &str, subject: &str, payload: Value, correlation: Option<&str>, idem: Option<&str>) -> Option<i64> {
    if !TYPES.contains(&ty) {
        tracing::error!(event_type = ty, "unknown event type");
        return None;
    }
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO events (ts_utc, type, version, correlation_id, idem_key, subject, payload) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![now(), ty, VERSION, correlation, idem, subject, payload.to_string()],
        )
        .ok()?;
    (n == 1).then(|| conn.last_insert_rowid())
}

pub fn list(conn: &Connection, after: i64, ty: Option<&str>, limit: usize) -> Result<Vec<Value>, String> {
    let mut st = conn.prepare("SELECT id, ts_utc, type, version, correlation_id, subject, payload FROM events WHERE id > ?1 AND (?2 IS NULL OR type = ?2) ORDER BY id LIMIT ?3").map_err(|e| e.to_string())?;
    let rows = st
        .query_map(params![after, ty, limit as i64], |r| {
            Ok(json!({ "id": r.get::<_, i64>(0)?, "ts_utc": r.get::<_, String>(1)?, "type": r.get::<_, String>(2)?, "version": r.get::<_, i64>(3)?, "correlation_id": r.get::<_, Option<String>>(4)?, "subject": r.get::<_, String>(5)?, "payload": serde_json::from_str::<Value>(&r.get::<_, String>(6)?).unwrap_or(Value::Null) }))
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

pub fn latest_id(conn: &Connection) -> i64 {
    conn.query_row("SELECT coalesce(max(id), 0) FROM events", [], |r| r.get(0)).unwrap_or(0)
}

/// Small key/value configuration (monitor thresholds, consumer cursors).
pub fn kv_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM kv_config WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
}

pub fn kv_set(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute("INSERT INTO kv_config (key, value, updated_utc) VALUES (?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_utc = excluded.updated_utc", params![key, value, now()]);
}

/// A user-facing notification, grouped by `dedupe_key` (a repeat bumps the count instead of adding a row).
pub fn notify(conn: &Connection, source: &str, title: &str, detail: &str, dedupe_key: Option<&str>) {
    let r = conn.execute(
        "INSERT INTO notifications (ts_utc, source, title, detail, dedupe_key) VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(dedupe_key) DO UPDATE SET count = count + 1, ts_utc = excluded.ts_utc, detail = excluded.detail, read = 0",
        params![now(), source, title, detail, dedupe_key],
    );
    if let Err(e) = r {
        tracing::warn!("notification not stored: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotent_typed_events_and_grouped_notifications() {
        let c = crate::db::open(":memory:").unwrap();
        let a = emit(&c, "BotStateChanged", "bot:1", json!({ "to": "running" }), Some("corr-1"), Some("bot1-start-1"));
        assert!(a.is_some());
        assert_eq!(emit(&c, "BotStateChanged", "bot:1", json!({}), None, Some("bot1-start-1")), None, "same key, no duplicate");
        assert_eq!(emit(&c, "NotAType", "x", json!({}), None, None), None);
        let l = list(&c, 0, None, 10).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!((l[0]["version"].as_i64(), l[0]["correlation_id"].as_str()), (Some(1), Some("corr-1")));
        notify(&c, "monitor", "Feed stale", "a", Some("feed:BTC"));
        notify(&c, "monitor", "Feed stale", "b", Some("feed:BTC"));
        let (n, count): (i64, i64) = c.query_row("SELECT count(*), max(count) FROM notifications", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((n, count), (1, 2));
    }
}
