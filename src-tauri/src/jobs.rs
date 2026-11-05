//! Durable background jobs for long research work (Monte Carlo, walk-forward, research, experiments).
//!
//! A job row is written before the work starts and updated with progress and the result, so a caller polls
//! `GET /api/jobs/{id}` instead of holding a request open. Cancellation is cooperative: the work checks the flag
//! between units (simulations, folds, candidates). A job still `running` when the app starts was interrupted by
//! the restart and is marked failed with that reason: nothing resumes half-done work silently.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Default)]
pub struct Cancels {
    flags: Mutex<HashMap<i64, Arc<AtomicBool>>>,
}

impl Cancels {
    pub fn flag(&self, id: i64) -> Arc<AtomicBool> {
        self.flags.lock().unwrap().entry(id).or_default().clone()
    }
    pub fn cancel(&self, id: i64) {
        self.flag(id).store(true, Ordering::SeqCst);
    }
    pub fn forget(&self, id: i64) {
        self.flags.lock().unwrap().remove(&id);
    }
}

pub const MAX_RUNNING: i64 = 3;

pub fn create(conn: &Connection, kind: &str, request: &Value) -> Result<i64, String> {
    let running: i64 = conn.query_row("SELECT count(*) FROM jobs WHERE status IN ('queued','running')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if running >= MAX_RUNNING {
        return Err(format!("{running} jobs are already running; wait or cancel one (limit {MAX_RUNNING})"));
    }
    conn.execute("INSERT INTO jobs (kind, status, request, created_utc) VALUES (?1,'queued',?2,?3)", params![kind, request.to_string(), now()]).map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

pub fn start(conn: &Connection, id: i64) {
    let _ = conn.execute("UPDATE jobs SET status = 'running', started_utc = ?1 WHERE id = ?2", params![now(), id]);
}

pub fn progress(conn: &Connection, id: i64, p: f64) {
    let _ = conn.execute("UPDATE jobs SET progress = ?1 WHERE id = ?2", params![p.clamp(0.0, 1.0), id]);
}

pub fn finish(conn: &Connection, id: i64, r: Result<Value, String>, cancelled: bool) {
    let (status, result, error) = match (cancelled, r) {
        (true, _) => ("cancelled", None, Some("cancelled by you".to_string())),
        (false, Ok(v)) => ("done", Some(v.to_string()), None),
        (false, Err(e)) => ("failed", None, Some(e)),
    };
    let _ = conn.execute("UPDATE jobs SET status = ?1, result = ?2, error = ?3, progress = CASE WHEN ?1 = 'done' THEN 1 ELSE progress END, finished_utc = ?4 WHERE id = ?5", params![status, result, error, now(), id]);
}

pub fn request_cancel(conn: &Connection, id: i64) -> Result<(), String> {
    let n = conn.execute("UPDATE jobs SET cancel_requested = 1 WHERE id = ?1 AND status IN ('queued','running')", [id]).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("no such running job".into());
    }
    Ok(())
}

/// Called at startup: jobs interrupted by a restart are failed, with the reason.
pub fn fail_interrupted(conn: &Connection) {
    let _ = conn.execute("UPDATE jobs SET status = 'failed', error = 'interrupted by an app restart; run it again', finished_utc = ?1 WHERE status IN ('queued','running')", [now()]);
}

pub fn get(conn: &Connection, id: i64) -> Result<Value, String> {
    conn.query_row("SELECT id, kind, status, request, result, error, progress, created_utc, started_utc, finished_utc FROM jobs WHERE id = ?1", [id], |r| {
        let j = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok()).unwrap_or(Value::Null);
        Ok(json!({ "id": r.get::<_, i64>(0)?, "kind": r.get::<_, String>(1)?, "status": r.get::<_, String>(2)?, "request": j(r.get(3)?), "result": j(r.get(4)?), "error": r.get::<_, Option<String>>(5)?, "progress": r.get::<_, f64>(6)?, "created_utc": r.get::<_, String>(7)?, "started_utc": r.get::<_, Option<String>>(8)?, "finished_utc": r.get::<_, Option<String>>(9)? }))
    })
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "no such job".into())
}

pub fn list(conn: &Connection, kind: Option<&str>) -> Result<Value, String> {
    let mut st = conn.prepare("SELECT id FROM jobs WHERE ?1 IS NULL OR kind = ?1 ORDER BY id DESC LIMIT 50").map_err(|e| e.to_string())?;
    let ids: Vec<i64> = st.query_map([kind], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    let mut out = vec![];
    for id in ids {
        let mut j = get(conn, id)?;
        j["result"] = Value::Null; // the list stays light; fetch one job for its result
        out.push(j);
    }
    Ok(json!({ "jobs": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_limit_cancel_and_restart() {
        let c = crate::db::open(":memory:").unwrap();
        let id = create(&c, "montecarlo", &json!({ "n": 1 })).unwrap();
        start(&c, id);
        progress(&c, id, 0.5);
        finish(&c, id, Ok(json!({ "ok": true })), false);
        let j = get(&c, id).unwrap();
        assert_eq!((j["status"].as_str(), j["progress"].as_f64()), (Some("done"), Some(1.0)));
        for _ in 0..MAX_RUNNING {
            create(&c, "x", &json!({})).unwrap();
        }
        assert!(create(&c, "x", &json!({})).unwrap_err().contains("already running"));
        fail_interrupted(&c);
        assert_eq!(get(&c, id + 1).unwrap()["status"], "failed");
        assert!(request_cancel(&c, id).is_err(), "a finished job cannot be cancelled");
        let cs = Cancels::default();
        cs.cancel(7);
        assert!(cs.flag(7).load(Ordering::SeqCst));
    }
}
