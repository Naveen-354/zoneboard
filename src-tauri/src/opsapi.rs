//! HTTP handlers for operations and intelligence (phases 9 to 12): monitor, events and notifications, portfolio
//! risk, attribution, drift, strategy version control, jobs. Routes are registered in `server.rs`.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::server::{err, json_body, Shared};

pub(crate) fn out(r: Result<Value, String>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) if e.starts_with("no such") => err(StatusCode::NOT_FOUND, &e),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

// ------------------------------------------------------------------ monitor, events, notifications

pub async fn monitor(State(s): State<Shared>) -> Response {
    Json(s.journal.with_conn(|c| crate::monitor::overview(c, &s.metrics))).into_response()
}

pub async fn monitor_thresholds(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::monitor::set_thresholds(c, &b)))
}

pub async fn events(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let after = q.get("after").and_then(|x| x.parse().ok()).unwrap_or(0);
    let limit = q.get("limit").and_then(|x| x.parse().ok()).unwrap_or(100usize).clamp(1, 500);
    let ty = q.get("type").cloned();
    out(s.journal.with_conn(|c| crate::events::list(c, after, ty.as_deref(), limit).map(|e| json!({ "events": e, "types": crate::events::TYPES }))))
}

pub async fn notifications(State(s): State<Shared>) -> Response {
    let r = s.journal.with_conn(|c| -> rusqlite::Result<Value> {
        let mut st = c.prepare("SELECT id, ts_utc, source, title, detail, count, read FROM notifications ORDER BY id DESC LIMIT 100")?;
        let rows: Vec<Value> = st.query_map([], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "ts_utc": r.get::<_, String>(1)?, "source": r.get::<_, String>(2)?, "title": r.get::<_, String>(3)?, "detail": r.get::<_, String>(4)?, "count": r.get::<_, i64>(5)?, "read": r.get::<_, i64>(6)? != 0 })))?.filter_map(Result::ok).collect();
        let unread: i64 = c.query_row("SELECT count(*) FROM notifications WHERE read = 0", [], |r| r.get(0))?;
        Ok(json!({ "notifications": rows, "unread": unread }))
    });
    out(r.map_err(|e| e.to_string()))
}

pub async fn notifications_read(State(s): State<Shared>) -> Response {
    let _ = s.journal.with_conn(|c| c.execute("UPDATE notifications SET read = 1 WHERE read = 0", []));
    Json(json!({ "ok": true })).into_response()
}

// ------------------------------------------------------------------ risk

pub async fn portfolio_risk(State(s): State<Shared>) -> Response {
    Json(s.journal.with_conn(|c| crate::riskintel::view(c))).into_response()
}

pub async fn portfolio_risk_put(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::riskintel::set_portfolio(c, &b)))
}

pub async fn extra_limits_put(State(s): State<Shared>, Path(account): Path<String>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| {
        crate::risk::limits_json(c, &account)?;
        crate::riskintel::set_extra(c, &account, &b)
    }))
}

// ------------------------------------------------------------------ attribution and drift

pub async fn attribution(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let dim = q.get("dimension").map_or("bot", String::as_str).to_string();
    let date = |k: &str| q.get(k).filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()).cloned();
    let (from, to, acct) = (date("from"), date("to"), q.get("account").cloned());
    let bot = q.get("bot").and_then(|b| b.parse::<i64>().ok());
    out(s.journal.with_conn(|c| crate::attribution::attribute(c, &dim, from.as_deref(), to.as_deref(), acct.as_deref(), bot)))
}

pub async fn attribution_trade(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::attribution::trade_detail(c, id)))
}

pub async fn drift(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::drift::evaluate(c, id)))
}

pub async fn drift_config(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::drift::set_config(c, &b)))
}

pub async fn drift_autopause(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let on = b.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    out(s.journal.with_conn(|c| crate::drift::set_autopause(c, id, on).map(|_| json!({ "bot_id": id, "autopause": on }))))
}

// ------------------------------------------------------------------ strategy version control

pub async fn version_detail(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::strategies::detail(c, id)))
}

pub async fn version_diff(State(s): State<Shared>, Path((a, b)): Path<(i64, i64)>) -> Response {
    out(s.journal.with_conn(|c| crate::strategies::diff(c, a, b)))
}

pub async fn version_tag(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let tag = b.get("tag").and_then(Value::as_str).unwrap_or("").to_string();
    out(s.journal.with_conn(|c| crate::strategies::tag(c, id, &tag)))
}

pub async fn version_rollback(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::strategies::rollback(c, id)))
}

// ------------------------------------------------------------------ jobs

pub async fn jobs(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let kind = q.get("kind").cloned();
    out(s.journal.with_conn(|c| crate::jobs::list(c, kind.as_deref())))
}

pub async fn job(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::jobs::get(c, id)))
}

pub async fn job_cancel(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    match s.journal.with_conn(|c| crate::jobs::request_cancel(c, id)) {
        Ok(()) => {
            s.cancels.cancel(id);
            Json(json!({ "ok": true, "note": "cancellation requested: the job stops at its next checkpoint" })).into_response()
        }
        Err(e) => err(StatusCode::NOT_FOUND, &e),
    }
}

/// Run `work` as a background job. It gets a progress callback and a cancelled() check.
pub fn spawn_job<F>(s: &Shared, kind: &str, request: Value, work: F) -> Result<i64, String>
where
    F: FnOnce(&dyn Fn(f64), &dyn Fn() -> bool) -> Result<Value, String> + Send + 'static,
{
    let id = s.journal.with_conn(|c| crate::jobs::create(c, kind, &request))?;
    let flag = s.cancels.flag(id);
    let st = s.clone();
    tokio::task::spawn_blocking(move || {
        st.journal.with_conn(|c| crate::jobs::start(c, id));
        let progress = |p: f64| st.journal.with_conn(|c| crate::jobs::progress(c, id, p));
        let cancelled = || flag.load(std::sync::atomic::Ordering::SeqCst);
        let r = work(&progress, &cancelled);
        let was_cancelled = cancelled();
        st.journal.with_conn(|c| crate::jobs::finish(c, id, r, was_cancelled));
        st.cancels.forget(id);
    });
    Ok(id)
}

/// Called by the runner after each tick: monitor incidents every tick, drift for active bots every hour.
pub fn housekeeping(s: &Shared) {
    s.journal.with_conn(|c| {
        let comps = crate::monitor::evaluate(c, &s.metrics);
        crate::monitor::record_incidents(c, &comps);
        let hour = chrono::Utc::now().format("%Y%m%d%H").to_string();
        if crate::events::kv_get(c, "drift.last_hour").as_deref() != Some(hour.as_str()) {
            crate::events::kv_set(c, "drift.last_hour", &hour);
            for id in crate::bots::ids_with_status(c, &["running", "paused"]) {
                let _ = crate::drift::evaluate(c, id);
            }
        }
    });
}
