//! Scoped developer API tokens.
//!
//! A token is shown once at creation (`zbk_` + 64 hex characters) and stored only as its SHA-256 hash with a
//! 4-character hint. It is sent as `Authorization: Bearer zbk_...`, and only its scopes are allowed:
//!
//! * `read`: GET requests;
//! * `research`: run backtests, optimisations, scans; publish and import strategies; ask the AI assistant;
//! * `trade`: start, pause and stop bots, and paper ticket orders.
//!
//! Some paths are never reachable with a developer token, whatever its scopes: broker connections and
//! credentials, the AI key, live arming, approvals, risk limits and the kill switch, settings, and token
//! management itself. Those need the app's own launch token (a person at the app). Each token is limited to
//! 120 requests a minute, and creation, use and revocation are audited.

use std::collections::HashMap;
use std::sync::Mutex;

use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const SCOPES: [&str; 3] = ["read", "research", "trade"];
const PER_MINUTE: u32 = 120;

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Paths a developer token can never reach.
pub fn forbidden(path: &str) -> bool {
    const NEVER: [&str; 8] = ["/api/dev/", "/api/live/", "/api/approvals", "/api/risk", "/api/ai/key", "/api/ai/settings", "/api/settings", "/api/accounts"];
    NEVER.iter().any(|p| path.starts_with(p)) || (path.starts_with("/api/bots/") && path.ends_with("/approval"))
}

/// The scope a request needs, or `None` when no developer token may make it.
pub fn needed(method: &str, path: &str) -> Option<&'static str> {
    if forbidden(path) {
        return None;
    }
    if method == "GET" {
        return Some("read");
    }
    let research = ["/api/backtest", "/api/scanner/run", "/api/strategies", "/api/ai/draft", "/api/ai/review", "/api/ai/explain/"];
    if research.iter().any(|p| path.starts_with(p)) {
        return Some("research");
    }
    let trade = path == "/api/positions" || path.starts_with("/api/positions/") || (path.starts_with("/api/bots/") && ["/start", "/pause", "/stop"].iter().any(|s| path.ends_with(s)));
    trade.then_some("trade")
}

pub fn create(conn: &Connection, name: &str, scopes: &[String]) -> Result<Value, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("token name must be 1 to 60 characters".into());
    }
    if scopes.is_empty() || scopes.iter().any(|s| !SCOPES.contains(&s.as_str())) {
        return Err(format!("scopes must be some of {}", SCOPES.join(", ")));
    }
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    let token = format!("zbk_{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>());
    let hint = format!("…{}", &token[token.len() - 4..]);
    conn.execute("INSERT INTO api_tokens (name, scopes, token_hash, hint, created_utc) VALUES (?1,?2,?3,?4,?5)", params![name, scopes.join(","), hash(&token), hint, now()]).map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    crate::audit::record(conn, "user", "api_token_created", &format!("api_token:{id}"), &json!({ "id": id, "name": name, "scopes": scopes, "hint": hint })).map_err(|e| e.to_string())?;
    Ok(json!({ "id": id, "name": name, "scopes": scopes, "hint": hint, "token": token, "note": "Shown once. ZoneBoard keeps only its hash." }))
}

pub fn list(conn: &Connection) -> Result<Value, String> {
    let mut st = conn.prepare("SELECT id, name, scopes, hint, created_utc, last_used_utc, revoked_utc FROM api_tokens ORDER BY id DESC").map_err(|e| e.to_string())?;
    let rows: Vec<Value> = st
        .query_map([], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "scopes": r.get::<_, String>(2)?.split(',').collect::<Vec<_>>(), "hint": r.get::<_, String>(3)?, "created_utc": r.get::<_, String>(4)?, "last_used_utc": r.get::<_, Option<String>>(5)?, "revoked": r.get::<_, Option<String>>(6)?.is_some() })))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(json!({ "tokens": rows, "scopes": SCOPES }))
}

pub fn revoke(conn: &Connection, id: i64) -> Result<(), String> {
    let n = conn.execute("UPDATE api_tokens SET revoked_utc = ?1 WHERE id = ?2 AND revoked_utc IS NULL", params![now(), id]).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("no such active token".into());
    }
    crate::audit::record(conn, "user", "api_token_revoked", &format!("api_token:{id}"), &json!({ "id": id })).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Default)]
pub struct Limiter {
    window: Mutex<HashMap<i64, (i64, u32)>>,
}

impl Limiter {
    /// `true` when this token may make another request this minute.
    pub fn allow(&self, id: i64) -> bool {
        let minute = chrono::Utc::now().timestamp() / 60;
        let mut w = self.window.lock().unwrap();
        let e = w.entry(id).or_insert((minute, 0));
        if e.0 != minute {
            *e = (minute, 0);
        }
        e.1 += 1;
        e.1 <= PER_MINUTE
    }
}

/// Check a bearer token for a request. `Ok(id)` when it is valid, active, has the needed scope and is under
/// its rate limit; otherwise the reason (never echoing the token).
pub fn authorize(conn: &Connection, limiter: &Limiter, token: &str, method: &str, path: &str) -> Result<i64, (u16, &'static str)> {
    let row: Option<(i64, String, Option<String>)> = conn.query_row("SELECT id, scopes, revoked_utc FROM api_tokens WHERE token_hash = ?1", [hash(token)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(|_| (500, "token check failed"))?;
    let Some((id, scopes, revoked)) = row else { return Err((401, "unknown API token")) };
    if revoked.is_some() {
        return Err((401, "this API token was revoked"));
    }
    let Some(need) = needed(method, path) else { return Err((403, "developer tokens cannot use this endpoint")) };
    if !scopes.split(',').any(|s| s == need) {
        return Err((403, "this API token lacks the scope this request needs"));
    }
    if !limiter.allow(id) {
        return Err((429, "API token rate limit reached (120 a minute)"));
    }
    let _ = conn.execute("UPDATE api_tokens SET last_used_utc = ?1 WHERE id = ?2", params![now(), id]);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_paths_revocation_and_rate_limit() {
        let c = crate::db::open(":memory:").unwrap();
        let t = create(&c, "notebook", &["read".into(), "research".into()]).unwrap();
        let tok = t["token"].as_str().unwrap().to_string();
        let stored: String = c.query_row("SELECT token_hash FROM api_tokens", [], |r| r.get(0)).unwrap();
        assert_ne!(stored, tok, "only the hash is stored");
        let lim = Limiter::default();
        assert!(authorize(&c, &lim, &tok, "GET", "/api/bots").is_ok());
        assert!(authorize(&c, &lim, &tok, "POST", "/api/backtest").is_ok());
        assert_eq!(authorize(&c, &lim, &tok, "POST", "/api/bots/1/start").unwrap_err().0, 403, "no trade scope");
        for (m, p) in [("POST", "/api/live/arm"), ("POST", "/api/approvals/1/decide"), ("PUT", "/api/risk/limits/demo-1"), ("GET", "/api/accounts"), ("POST", "/api/dev/tokens"), ("PUT", "/api/ai/key"), ("POST", "/api/risk/kill"), ("POST", "/api/bots/1/approval")] {
            assert_eq!(authorize(&c, &lim, &tok, m, p).unwrap_err().0, 403, "{m} {p} must be out of reach");
        }
        assert_eq!(authorize(&c, &lim, "zbk_nope", "GET", "/api/bots").unwrap_err().0, 401);
        for _ in 0..200 {
            let _ = lim.allow(999);
        }
        assert!(!lim.allow(999));
        revoke(&c, t["id"].as_i64().unwrap()).unwrap();
        assert_eq!(authorize(&c, &lim, &tok, "GET", "/api/bots").unwrap_err().0, 401);
        assert!(create(&c, "x", &["admin".into()]).is_err());
    }
}
