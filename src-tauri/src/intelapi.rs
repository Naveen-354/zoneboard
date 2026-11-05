//! HTTP handlers for phase 11: research runs, trade explainer, news, economic events, workflows.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::opsapi::{job_started, out};
use crate::server::{err, json_body, Shared};

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Start a research run as a job. Used by the API and by workflow `start_research` nodes.
pub async fn start_research(s: &Shared, mut b: Value, actor: &str) -> Result<i64, String> {
    if !b.is_object() {
        return Err("request body must be a JSON object".into());
    }
    // the backtest request needs a spec to validate; the researcher brings its own
    b["spec"] = json!({ "entry": { "long": { "left": { "price": "close" }, "op": "gt", "right": { "value": 0 } } } });
    b.as_object_mut().map(|o| o.remove("bot_id"));
    let (r, fa, warnings) = crate::opsapi::bt_data(s, &b).await.map_err(|_| "could not load market data for the research run (check symbol, interval and bars)".to_string())?;
    let hypothesis = json!({ "templates": b["templates"], "objective": b["objective"].as_str().unwrap_or("return"), "note": b["note"].as_str().unwrap_or("") });
    let request = json!({ "symbol": r.symbol, "interval": r.interval, "market": r.market, "bars": fa.len(), "hypothesis": hypothesis, "source": crate::botapi::source_label(&r), "actor": actor, "pipeline_version": crate::researcher::PIPELINE_VERSION });
    let cfg = r.cfg;
    let st = s.clone();
    let req2 = request.clone();
    crate::opsapi::spawn_job(s, "research", request, move |prog, cancelled| {
        let rep = crate::researcher::run(&hypothesis, &fa, &cfg, warnings, prog, cancelled)?;
        let status = if rep["candidate"].is_null() { "no_candidate" } else { "pending" };
        let id = st.journal.with_conn(|c| -> Result<i64, String> {
            c.execute(
                "INSERT INTO research_runs (created_utc, request, report, candidate, review_status) VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![now(), req2.to_string(), rep.to_string(), (!rep["candidate"].is_null()).then(|| rep["candidate"].to_string()), status],
            )
            .map_err(|e| e.to_string())?;
            let id = c.last_insert_rowid();
            crate::events::emit(c, "ResearchRunCompleted", &format!("research:{id}"), json!({ "run_id": id, "candidate": status == "pending" }), None, None);
            if status == "pending" {
                crate::events::notify(c, "research", "Research candidate waiting for review", &format!("research run #{id} found a candidate; review it in Research Lab"), Some(&format!("research:{id}")));
            }
            Ok(id)
        })?;
        let mut rep = rep;
        rep["run_id"] = json!(id);
        rep["review_status"] = json!(status);
        Ok(rep)
    })
}

pub async fn research_start(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    job_started(start_research(&s, b, "user").await)
}

pub async fn research_runs(State(s): State<Shared>) -> Response {
    let r = s.journal.with_conn(|c| -> rusqlite::Result<Vec<Value>> {
        let mut st = c.prepare("SELECT id, created_utc, request, report, review_status, reviewed_by, reviewed_utc FROM research_runs ORDER BY id DESC LIMIT 50")?;
        let m = st.query_map([], |r| {
            Ok(json!({ "id": r.get::<_, i64>(0)?, "created_utc": r.get::<_, String>(1)?, "request": serde_json::from_str::<Value>(&r.get::<_, String>(2)?).unwrap_or(Value::Null),
                "report": r.get::<_, Option<String>>(3)?.and_then(|x| serde_json::from_str::<Value>(&x).ok()), "review_status": r.get::<_, String>(4)?, "reviewed_by": r.get::<_, Option<String>>(5)?, "reviewed_utc": r.get::<_, Option<String>>(6)? }))
        })?;
        Ok(m.filter_map(Result::ok).collect())
    });
    out(r.map(|v| json!({ "runs": v })).map_err(|e| e.to_string()))
}

/// `POST /api/research/runs/{id}/review {decision: approve|reject, name?, reason?}`. Approval publishes the
/// candidate as a strategy version (nothing runs); rejection records why.
pub async fn research_review(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| -> Result<Value, String> {
        let (cand, status): (Option<String>, String) = c.query_row("SELECT candidate, review_status FROM research_runs WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|_| "no such research run".to_string())?;
        if status != "pending" {
            return Err(format!("this run is {status}: only a pending candidate can be reviewed"));
        }
        let cand: Value = cand.and_then(|x| serde_json::from_str(&x).ok()).ok_or("this run has no candidate")?;
        match b["decision"].as_str() {
            Some("approve") => {
                let name = b["name"].as_str().map(str::trim).filter(|x| !x.is_empty()).map(String::from).unwrap_or_else(|| format!("Research #{id}: {}", cand["template"].as_str().unwrap_or("candidate")));
                let (sid, vid, hash) = crate::strategies::publish(c, &name, &cand["spec"], &format!("from research run #{id}"), "user")?;
                c.execute("UPDATE research_runs SET review_status = 'approved', reviewed_by = 'user', reviewed_utc = ?2 WHERE id = ?1", rusqlite::params![id, now()]).map_err(|e| e.to_string())?;
                let _ = crate::audit::record(c, "user", "research_approved", &format!("research:{id}"), &json!({ "strategy_version_id": vid }));
                Ok(json!({ "run_id": id, "review_status": "approved", "strategy_id": sid, "strategy_version_id": vid, "hash": hash, "note": "Published as a strategy version. No bot was created and nothing is running." }))
            }
            Some("reject") => {
                c.execute("UPDATE research_runs SET review_status = 'rejected', reviewed_by = 'user', reviewed_utc = ?2 WHERE id = ?1", rusqlite::params![id, now()]).map_err(|e| e.to_string())?;
                let _ = crate::audit::record(c, "user", "research_rejected", &format!("research:{id}"), &json!({ "reason": b["reason"] }));
                Ok(json!({ "run_id": id, "review_status": "rejected" }))
            }
            _ => Err("decision must be approve or reject".into()),
        }
    }))
}

pub async fn explain_trade(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::explain::facts(c, id)))
}

pub async fn explain_trade_ai(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    let facts = match s.journal.with_conn(|c| crate::explain::facts(c, id)) {
        Ok(f) => f,
        Err(e) => return out(Err(e)),
    };
    match crate::ai::explain_trade(&s, &facts).await {
        Ok(v) => Json(json!({ "facts": facts, "ai": v })).into_response(),
        Err(e) => err(StatusCode::SERVICE_UNAVAILABLE, &e),
    }
}

pub async fn news(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let sym = q.get("symbol").map(|x| x.trim().to_uppercase()).filter(|x| !x.is_empty());
    out(s.journal.with_conn(|c| crate::intel::news(c, sym.as_deref(), 100)))
}

pub async fn news_import(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let items = b["items"].as_array().cloned().unwrap_or_default();
    out(s.journal.with_conn(|c| crate::intel::ingest_news(c, "manual", &items)))
}

/// Made-up news, only while the server runs on sample data.
pub async fn news_sample(State(s): State<Shared>) -> Response {
    if !s.data.sample {
        return err(StatusCode::CONFLICT, "sample news is only available on sample data");
    }
    out(s.journal.with_conn(|c| crate::intel::ingest_news(c, "sample", &crate::intel::sample_news(chrono::Utc::now()))))
}

pub async fn econ_events(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let from = q.get("from").cloned().unwrap_or_else(|| (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    let to = q.get("to").cloned().unwrap_or_else(|| (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    out(s.journal.with_conn(|c| crate::intel::events_list(c, &from, &to)))
}

pub async fn econ_import(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let items = match b["csv"].as_str() {
        Some(csv) => match crate::intel::events_from_csv(csv) {
            Ok(i) => i,
            Err(e) => return out(Err(e)),
        },
        None => b["items"].as_array().cloned().unwrap_or_default(),
    };
    out(s.journal.with_conn(|c| crate::intel::ingest_events(c, "manual", &items)))
}

pub async fn econ_blackout(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::intel::set_blackout(c, &b)))
}

pub async fn workflows(State(s): State<Shared>) -> Response {
    out(s.journal.with_conn(|c| crate::workflows::list(c)))
}

pub async fn workflow_create(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::workflows::save(c, None, b["name"].as_str().unwrap_or(""), &b["graph"])))
}

pub async fn workflow_get(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::workflows::get(c, id)))
}

pub async fn workflow_save(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::workflows::save(c, Some(id), b["name"].as_str().unwrap_or(""), &b["graph"])))
}

pub async fn workflow_status(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::workflows::set_status(c, id, b["status"].as_str().unwrap_or(""))))
}

/// `POST /api/workflows/{id}/dryrun {event_id}` or `{event: {type, subject, payload}}`.
pub async fn workflow_dry_run(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| {
        let ev = match b["event_id"].as_i64() {
            Some(eid) => crate::events::list(c, eid - 1, None, 1)?.into_iter().find(|e| e["id"] == eid).ok_or("no such event")?,
            None => {
                let mut e = b["event"].clone();
                if !e.is_object() {
                    return Err("give event_id or an event".into());
                }
                e["id"] = json!(0);
                e
            }
        };
        crate::workflows::dry_run(c, id, &ev)
    }))
}
