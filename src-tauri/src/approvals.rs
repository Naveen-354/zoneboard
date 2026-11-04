//! Human approval of live deployments.
//!
//! An approval is bound to an exact **scope**: the bot, its strategy version and content hash, the account,
//! the symbols, timeframe, capital and risk settings. Any change to any of them (an edit publishes a new
//! strategy version) makes the approval stop matching, so the bot cannot trade live on it. Approvals expire
//! (7 days by default) and can be revoked. Only a person can approve: the decision endpoint takes a typed
//! phrase, and no AI tool can reach it (see `ai.rs`).

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const APPROVE_PHRASE: &str = "APPROVE LIVE DEPLOYMENT";
const DEFAULT_DAYS: i64 = 7;

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

fn iso(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The scope a live deployment of this bot would have right now.
pub fn current_scope(conn: &Connection, bot_id: i64) -> Result<Value, String> {
    conn.query_row(
        "SELECT b.strategy_version_id, v.hash, b.account_id, b.symbol, b.symbol2, b.interval, b.allocation, b.max_daily_loss, b.max_open_positions, b.slippage_pct, a.kind, a.environment
         FROM bots b LEFT JOIN strategy_versions v ON v.id = b.strategy_version_id LEFT JOIN accounts a ON a.id = b.account_id WHERE b.id = ?1",
        [bot_id],
        |r| {
            Ok(json!({
                "bot_id": bot_id, "strategy_version_id": r.get::<_, Option<i64>>(0)?, "spec_hash": r.get::<_, Option<String>>(1)?,
                "account_id": r.get::<_, String>(2)?, "symbols": [r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?],
                "interval": r.get::<_, String>(5)?, "allocation": r.get::<_, String>(6)?, "max_daily_loss": r.get::<_, Option<String>>(7)?,
                "max_open_positions": r.get::<_, i64>(8)?, "slippage_pct": r.get::<_, f64>(9)?,
                "account_kind": r.get::<_, Option<String>>(10)?, "environment": r.get::<_, Option<String>>(11)?,
            }))
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "no such bot".to_string())
}

fn evidence(conn: &Connection, bot_id: i64) -> Value {
    let bt: Option<(i64, String, String)> = conn.query_row("SELECT id, created_utc, metrics FROM backtests WHERE bot_id = ?1 ORDER BY id DESC LIMIT 1", [bot_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().ok().flatten();
    let trades: i64 = conn.query_row("SELECT count(*) FROM trades WHERE bot_id = ?1", [bot_id], |r| r.get(0)).unwrap_or(0);
    json!({
        "latest_backtest": bt.map(|(id, t, m)| json!({ "id": id, "created_utc": t, "metrics": serde_json::from_str::<Value>(&m).unwrap_or(Value::Null) })),
        "closed_trades_recorded": trades,
    })
}

/// Ask for approval of a live deployment of `bot_id` in its current configuration.
pub fn request(conn: &Connection, bot_id: i64, actor: &str) -> Result<Value, String> {
    let scope = current_scope(conn, bot_id)?;
    if scope["environment"] != "live" {
        return Err("approval is for live deployments: this bot's account is not a live broker account".into());
    }
    if scope["strategy_version_id"].is_null() {
        return Err("the bot has no published strategy version".into());
    }
    let ev = evidence(conn, bot_id);
    if ev["latest_backtest"].is_null() {
        return Err("save a backtest of this bot first: an approval needs evidence".into());
    }
    // an older pending request for the same bot is superseded
    conn.execute("UPDATE approvals SET status = 'expired', reason = 'superseded' WHERE bot_id = ?1 AND status = 'pending'", [bot_id]).map_err(|e| e.to_string())?;
    let t = now();
    conn.execute(
        "INSERT INTO approvals (bot_id, strategy_version_id, spec_hash, account_id, scope, requested_by, requested_utc, expires_utc, status, evidence) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'pending',?9)",
        params![bot_id, scope["strategy_version_id"].as_i64(), scope["spec_hash"].as_str(), scope["account_id"].as_str(), scope.to_string(), actor, iso(t), iso(t + chrono::Duration::days(DEFAULT_DAYS)), ev.to_string()],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    conn.execute("UPDATE bots SET stage = 'pending_approval' WHERE id = ?1", [bot_id]).map_err(|e| e.to_string())?;
    crate::audit::record(conn, actor, "approval_requested", &format!("bot:{bot_id}"), &json!({ "approval_id": id, "scope": scope })).map_err(|e| e.to_string())?;
    get(conn, id)
}

/// A person's decision. `actor` must be "user": the AI orchestrator has no path here.
pub fn decide(conn: &Connection, id: i64, approve: bool, phrase: &str, reason: &str, actor: &str) -> Result<Value, String> {
    if actor != "user" {
        return Err("only a person can approve or reject a live deployment".into());
    }
    let (bot_id, status, expires, scope): (i64, String, String, String) =
        conn.query_row("SELECT bot_id, status, expires_utc, scope FROM approvals WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional().map_err(|e| e.to_string())?.ok_or("no such approval")?;
    if status != "pending" {
        return Err(format!("this request is {status}"));
    }
    if chrono::DateTime::parse_from_rfc3339(&expires).map(|e| e < now()).unwrap_or(true) {
        conn.execute("UPDATE approvals SET status = 'expired' WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
        return Err("this request has expired".into());
    }
    if approve {
        if phrase != APPROVE_PHRASE {
            return Err(format!("type {APPROVE_PHRASE} exactly to approve"));
        }
        // the bot must still be exactly what was reviewed
        let now_scope = current_scope(conn, bot_id)?;
        if now_scope.to_string() != scope {
            conn.execute("UPDATE approvals SET status = 'expired', reason = 'bot changed after the request' WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
            return Err("the bot changed after this request was made; request approval again".into());
        }
    }
    let new = if approve { "approved" } else { "rejected" };
    conn.execute("UPDATE approvals SET status = ?1, decided_by = ?2, decided_utc = ?3, reason = ?4 WHERE id = ?5", params![new, actor, iso(now()), reason.chars().take(500).collect::<String>(), id]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE bots SET stage = ?1 WHERE id = ?2", params![if approve { "live" } else { "paper" }, bot_id]).map_err(|e| e.to_string())?;
    crate::audit::record(conn, actor, if approve { "approval_granted" } else { "approval_rejected" }, &format!("bot:{bot_id}"), &json!({ "approval_id": id, "reason": reason })).map_err(|e| e.to_string())?;
    get(conn, id)
}

pub fn revoke(conn: &Connection, id: i64, reason: &str) -> Result<Value, String> {
    let (bot_id, status): (i64, String) = conn.query_row("SELECT bot_id, status FROM approvals WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(|e| e.to_string())?.ok_or("no such approval")?;
    if !["pending", "approved"].contains(&status.as_str()) {
        return Err(format!("this approval is already {status}"));
    }
    conn.execute("UPDATE approvals SET status = 'revoked', reason = ?1, decided_utc = ?2 WHERE id = ?3", params![reason, iso(now()), id]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE bots SET stage = 'paper' WHERE id = ?1 AND stage IN ('live','pending_approval')", [bot_id]).map_err(|e| e.to_string())?;
    crate::audit::record(conn, "user", "approval_revoked", &format!("bot:{bot_id}"), &json!({ "approval_id": id, "reason": reason })).map_err(|e| e.to_string())?;
    get(conn, id)
}

/// Is there a current, unexpired, unrevoked approval whose scope equals the bot's scope right now?
pub fn valid_for(conn: &Connection, bot_id: i64) -> Result<Option<i64>, String> {
    let scope = current_scope(conn, bot_id)?.to_string();
    let rows: Vec<(i64, String, String)> = {
        let mut st = conn.prepare("SELECT id, scope, expires_utc FROM approvals WHERE bot_id = ?1 AND status = 'approved' ORDER BY id DESC").map_err(|e| e.to_string())?;
        let m = st.query_map([bot_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| e.to_string())?;
        m.filter_map(Result::ok).collect()
    };
    for (id, s, exp) in rows {
        let live = chrono::DateTime::parse_from_rfc3339(&exp).map(|e| e > now()).unwrap_or(false);
        if live && s == scope {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

pub fn get(conn: &Connection, id: i64) -> Result<Value, String> {
    conn.query_row(
        "SELECT id, bot_id, strategy_version_id, spec_hash, account_id, scope, requested_by, requested_utc, expires_utc, status, decided_by, decided_utc, reason, evidence FROM approvals WHERE id = ?1",
        [id],
        |r| {
            let j = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "bot_id": r.get::<_, i64>(1)?, "strategy_version_id": r.get::<_, i64>(2)?, "spec_hash": r.get::<_, String>(3)?,
                "account_id": r.get::<_, String>(4)?, "scope": j(r.get(5)?), "requested_by": r.get::<_, String>(6)?, "requested_utc": r.get::<_, String>(7)?,
                "expires_utc": r.get::<_, String>(8)?, "status": r.get::<_, String>(9)?, "decided_by": r.get::<_, Option<String>>(10)?, "decided_utc": r.get::<_, Option<String>>(11)?,
                "reason": r.get::<_, Option<String>>(12)?, "evidence": j(r.get(13)?),
            }))
        },
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "no such approval".to_string())
}

pub fn list(conn: &Connection) -> Result<Value, String> {
    // expire what has run out, so the list is honest
    let _ = conn.execute("UPDATE approvals SET status = 'expired' WHERE status IN ('pending','approved') AND expires_utc < ?1", [iso(now())]);
    let ids: Vec<i64> = {
        let mut st = conn.prepare("SELECT id FROM approvals ORDER BY id DESC LIMIT 200").map_err(|e| e.to_string())?;
        let m = st.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?;
        m.filter_map(Result::ok).collect()
    };
    let mut out = vec![];
    for id in ids {
        let mut a = get(conn, id)?;
        a["matches_current_bot"] = json!(current_scope(conn, a["bot_id"].as_i64().unwrap_or(0)).map(|s| s == a["scope"]).unwrap_or(false));
        out.push(a);
    }
    Ok(json!({ "approvals": out, "phrase": APPROVE_PHRASE }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (rusqlite::Connection, i64) {
        let mut c = crate::db::open(":memory:").unwrap();
        c.execute("INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived) VALUES ('bn-live-1','binance','Binance','live','active','x',0)", []).unwrap();
        let spec = crate::strategy::templates().into_iter().find(|t| t["id"] == "rsi").unwrap()["spec"].clone();
        let id = crate::bots::create(&mut c, &json!({ "name": "Live RSI", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "100", "account_id": "bn-live-1" })).unwrap();
        (c, id)
    }

    #[test]
    fn approval_is_human_exact_and_invalidated_by_any_change() {
        let (mut c, id) = setup();
        assert!(request(&c, id, "user").unwrap_err().contains("backtest"), "evidence is required");
        let vid: i64 = c.query_row("SELECT strategy_version_id FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        c.execute("INSERT INTO backtests (bot_id, created_utc, request, metrics, report, strategy_version_id) VALUES (?1,'x','{}','{\"trades\":3}','{}',?2)", rusqlite::params![id, vid]).unwrap();
        let a = request(&c, id, "user").unwrap();
        let aid = a["id"].as_i64().unwrap();
        assert_eq!(a["status"], "pending");
        assert!(decide(&c, aid, true, APPROVE_PHRASE, "", "ai").unwrap_err().contains("only a person"), "the AI cannot approve");
        assert!(decide(&c, aid, true, "yes", "", "user").unwrap_err().contains(APPROVE_PHRASE));
        assert_eq!(valid_for(&c, id).unwrap(), None);
        decide(&c, aid, true, APPROVE_PHRASE, "reviewed", "user").unwrap();
        assert_eq!(valid_for(&c, id).unwrap(), Some(aid));
        // any change to the bot's scope breaks the match: here, the capital
        crate::bots::update(&mut c, id, &json!({ "allocation": "200" })).unwrap();
        assert_eq!(valid_for(&c, id).unwrap(), None, "approval applies only to the exact reviewed configuration");
        crate::bots::update(&mut c, id, &json!({ "allocation": "100" })).unwrap();
        assert_eq!(valid_for(&c, id).unwrap(), Some(aid), "back to the reviewed configuration");
        revoke(&c, aid, "changed my mind").unwrap();
        assert_eq!(valid_for(&c, id).unwrap(), None);
        let audit: i64 = c.query_row("SELECT count(*) FROM audit_log WHERE action IN ('approval_requested','approval_granted','approval_revoked')", [], |r| r.get(0)).unwrap();
        assert_eq!(audit, 3);
    }

    #[test]
    fn demo_bots_cannot_request_live_approval_and_bad_transitions_are_audited() {
        let mut c = crate::db::open(":memory:").unwrap();
        let spec = crate::strategy::templates().into_iter().find(|t| t["id"] == "rsi").unwrap()["spec"].clone();
        let id = crate::bots::create(&mut c, &json!({ "name": "Paper", "symbol": "BTCUSDT", "spec": spec, "allocation": "100" })).unwrap();
        assert!(request(&c, id, "user").unwrap_err().contains("not a live broker account"));
        assert!(crate::bots::transition(&c, id, "live", "user").is_err(), "draft cannot jump to live");
        let n: i64 = c.query_row("SELECT count(*) FROM audit_log WHERE action = 'bot_transition_rejected'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
}
