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
        crate::intel::announce(c, chrono::Utc::now());
    });
    // workflows: research requests become bounded research jobs (fetching data is async)
    let research = s.journal.with_conn(|c| crate::workflows::process(c));
    for h in research {
        let st = s.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::intelapi::start_research(&st, h, "workflow").await {
                st.journal.with_conn(|c| crate::events::notify(c, "workflow", "Workflow research run not started", &e, None));
            }
        });
    }
}

// ------------------------------------------------------------------ phase 10: quant research

pub(crate) fn job_started(r: Result<i64, String>) -> Response {
    match r {
        Ok(id) => (StatusCode::ACCEPTED, Json(json!({ "job_id": id, "status_url": format!("/api/jobs/{id}") }))).into_response(),
        Err(e) if e.contains("already running") => err(StatusCode::TOO_MANY_REQUESTS, &e),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

/// `POST /api/research/montecarlo`: resample a saved backtest's trades (`backtest_id`) or a bot's recorded
/// trades (`bot_id`).
pub async fn montecarlo(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let src = s.journal.with_conn(|c| -> Result<(Vec<f64>, f64, String), String> {
        if let Some(id) = b.get("backtest_id").and_then(Value::as_i64) {
            let (rep, req): (String, String) = c.query_row("SELECT report, request FROM backtests WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|_| "no such backtest".to_string())?;
            let rep: Value = serde_json::from_str(&rep).unwrap_or(Value::Null);
            let req: Value = serde_json::from_str(&req).unwrap_or(Value::Null);
            let p: Vec<f64> = rep["trades"].as_array().cloned().unwrap_or_default().iter().filter_map(|t| t["pnl"].as_str().and_then(|x| x.parse().ok())).collect();
            Ok((p, req["capital"].as_f64().unwrap_or(10_000.0), format!("saved backtest #{id}")))
        } else if let Some(id) = b.get("bot_id").and_then(Value::as_i64) {
            let bot = crate::bots::load(c, id).map_err(|_| "no such bot".to_string())?;
            let mut st = c.prepare("SELECT side, qty, entry, exit, fees FROM trades WHERE bot_id = ?1 ORDER BY id").map_err(|e| e.to_string())?;
            let p: Vec<f64> = st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?))).map_err(|e| e.to_string())?
                .filter_map(Result::ok).filter_map(|(sd, q, e, x, f)| crate::accounts::realised(&sd, q, e, x, f).and_then(|d| rust_decimal::prelude::ToPrimitive::to_f64(&d))).collect();
            Ok((p, rust_decimal::prelude::ToPrimitive::to_f64(&bot.allocation).unwrap_or(1.0), format!("bot {id}'s recorded trades")))
        } else {
            Err("give backtest_id or bot_id".into())
        }
    });
    let (pnls, cap0, source) = match src {
        Ok(x) => x,
        Err(e) => return out(Err(e)),
    };
    let p = crate::research::McParams {
        method: b.get("method").and_then(Value::as_str).unwrap_or("bootstrap").to_string(),
        sims: b.get("simulations").and_then(Value::as_u64).unwrap_or(2000) as usize,
        seed: b.get("seed").and_then(Value::as_u64).unwrap_or(42),
        block: b.get("block").and_then(Value::as_u64).unwrap_or(5) as usize,
        horizon: b.get("horizon").and_then(Value::as_u64).map(|x| x as usize),
        extra_cost: b.get("extra_cost").and_then(Value::as_f64).unwrap_or(0.0).max(0.0),
        ruin_pct: b.get("ruin_pct").and_then(Value::as_f64).unwrap_or(50.0).clamp(1.0, 99.0),
        dd_thresholds: b.get("drawdown_thresholds").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_else(|| vec![10.0, 20.0, 30.0]),
    };
    let capital = b.get("capital").and_then(Value::as_f64).unwrap_or(cap0);
    let req = json!({ "source": source, "capital": capital, "method": p.method, "simulations": p.sims, "seed": p.seed });
    job_started(spawn_job(&s, "montecarlo", req, move |prog, cancelled| {
        crate::research::monte_carlo(&pnls, capital, &p, prog, cancelled).map(|mut v| {
            v["source"] = json!(source);
            v
        })
    }))
}

pub(crate) async fn bt_data(s: &Shared, b: &Value) -> Result<(crate::botapi::BtReq, std::sync::Arc<crate::zones::Frame>, usize), Response> {
    let r = crate::botapi::bt_request(s, b)?;
    let (fa, _) = crate::botapi::frames(s, &r).await?;
    let iv = crate::bots::interval_secs(&r.interval).unwrap_or(3600);
    let q = crate::marketdata::check(&fa, &r.market, iv, chrono::Utc::now().timestamp());
    Ok((r, std::sync::Arc::new(crate::marketdata::repair(&fa).0), q.warnings.len()))
}

pub async fn walkforward(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let (r, fa, _) = match bt_data(&s, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let axes: Vec<crate::backtest2::Axis> = b.get("params").and_then(Value::as_array).map(|l| l.iter().filter_map(|p| Some(crate::backtest2::Axis { path: p.get("path")?.as_str()?.to_string(), values: p.get("values")?.as_array()?.clone() })).collect()).unwrap_or_default();
    let n = fa.len();
    let p = crate::research::WfParams {
        mode: b.get("mode").and_then(Value::as_str).unwrap_or("rolling").to_string(),
        train: b.get("train_bars").and_then(Value::as_u64).map_or(n / 2, |x| x as usize),
        test: b.get("test_bars").and_then(Value::as_u64).map_or(n / 8, |x| x as usize),
        step: b.get("step_bars").and_then(Value::as_u64).map_or(n / 8, |x| x as usize).max(1),
        objective: b.get("objective").and_then(Value::as_str).unwrap_or("return").to_string(),
    };
    let (spec, cfg) = (r.spec.clone(), r.cfg);
    let req = json!({ "symbol": r.symbol, "interval": r.interval, "bars": n, "mode": p.mode, "train": p.train, "test": p.test, "step": p.step, "source": crate::botapi::source_label(&r) });
    let st = s.clone();
    job_started(spawn_job(&s, "walkforward", req, move |prog, cancelled| {
        let v = crate::research::walk_forward(&spec, &axes, &fa, &cfg, &p, prog, cancelled)?;
        st.journal.with_conn(|c| crate::events::emit(c, "WalkForwardCompleted", "research", json!({ "folds": v["folds"].as_array().map(|a| a.len()), "failed": v["failed_folds"] }), None, None));
        Ok(v)
    }))
}

pub async fn robustness(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let (r, fa, warnings) = match bt_data(&s, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let (spec, cfg, bot) = (r.spec.clone(), r.cfg, r.bot_id);
    let req = json!({ "symbol": r.symbol, "interval": r.interval, "bars": fa.len(), "bot_id": bot, "source": crate::botapi::source_label(&r), "score_version": crate::research::SCORE_VERSION });
    let st = s.clone();
    job_started(spawn_job(&s, "robustness", req, move |prog, cancelled| {
        let v = crate::research::robustness(&spec, &fa, &cfg, warnings, prog, cancelled)?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        st.journal.with_conn(|c| c.execute("INSERT INTO robustness (bot_id, score_version, score, components, created_utc) VALUES (?1,?2,?3,?4,?5)", rusqlite::params![bot, crate::research::SCORE_VERSION, v["score"].as_f64(), v.to_string(), now])).map_err(|e| e.to_string())?;
        Ok(v)
    }))
}

pub async fn robustness_list(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let bot = q.get("bot_id").and_then(|b| b.parse::<i64>().ok());
    let r = s.journal.with_conn(|c| -> rusqlite::Result<Vec<Value>> {
        let mut st = c.prepare("SELECT id, bot_id, score_version, score, created_utc FROM robustness WHERE ?1 IS NULL OR bot_id = ?1 ORDER BY id DESC LIMIT 50")?;
        let m = st.query_map([bot], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "bot_id": r.get::<_, Option<i64>>(1)?, "score_version": r.get::<_, String>(2)?, "score": r.get::<_, Option<f64>>(3)?, "created_utc": r.get::<_, String>(4)? })))?;
        Ok(m.filter_map(Result::ok).collect())
    });
    out(r.map(|v| json!({ "assessments": v })).map_err(|e| e.to_string()))
}

/// `POST /api/research/experiments`: variants `a` and `b` as strategy specs or `{"version_id": N}`, plus the
/// backtest window (symbol, interval, bars...).
pub async fn experiment(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let resolve = |v: &Value| -> Result<Value, String> {
        match v.get("version_id").and_then(Value::as_i64) {
            Some(id) => s.journal.with_conn(|c| crate::strategies::version(c, id)).and_then(|x| x.map(|x| x["spec"].clone()).ok_or_else(|| "no such strategy version".into())),
            None => Ok(v.clone()),
        }
    };
    let (sa, sb) = match (resolve(&b["a"]), resolve(&b["b"])) {
        (Ok(a), Ok(b2)) => (a, b2),
        (Err(e), _) | (_, Err(e)) => return out(Err(e)),
    };
    let mut req_b = b.clone();
    req_b["spec"] = sa.clone();
    let (r, fa, _) = match bt_data(&s, &req_b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let cfg = r.cfg;
    let min = b.get("min_trades").and_then(Value::as_u64).unwrap_or(20) as usize;
    let seed = b.get("seed").and_then(Value::as_u64).unwrap_or(42);
    let req = json!({ "symbol": r.symbol, "interval": r.interval, "bars": fa.len(), "a_hash": crate::strategies::hash(&sa), "b_hash": crate::strategies::hash(&sb), "min_trades": min, "seed": seed, "source": crate::botapi::source_label(&r) });
    job_started(spawn_job(&s, "experiment", req, move |_, _| crate::research::experiment(&sa, &sb, &fa, &cfg, min, seed)))
}

/// `POST /api/regimes/classify {market, symbol, interval, bars}`: classify, store the latest observations and
/// emit `MarketRegimeChanged` when the current regime differs from the last stored one.
pub async fn regimes_classify(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let mut b = json_body(&headers, &body);
    if b.get("spec").is_none() {
        b["spec"] = json!({ "entry": { "long": { "left": { "price": "close" }, "op": "gt", "right": { "value": 0 } } } });
    }
    let (r, fa, _) = match bt_data(&s, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let iv = crate::bots::interval_secs(&r.interval).unwrap_or(3600);
    let now = chrono::Utc::now().timestamp();
    let closed = crate::backtest2::slice(&fa, 0, fa.t.iter().take_while(|&&t| t + iv <= now).count());
    let obs = crate::regime::classify(&closed);
    let mut v = crate::regime::to_json(&closed, &obs, 300);
    let created = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    s.journal.with_conn(|c| {
        let prev: Option<String> = c.query_row("SELECT regime FROM regime_observations WHERE market = ?1 AND symbol = ?2 AND interval = ?3 AND rule_version = ?4 ORDER BY bar_time DESC LIMIT 1", rusqlite::params![r.market, r.symbol, r.interval, crate::regime::RULE_VERSION], |x| x.get(0)).ok();
        for i in obs.len().saturating_sub(200)..obs.len() {
            let _ = c.execute("INSERT OR IGNORE INTO regime_observations (market, symbol, interval, bar_time, regime, confidence, features, rule_version, created_utc) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                rusqlite::params![r.market, r.symbol, r.interval, closed.t[i], obs[i].regime, obs[i].confidence, json!({ "trend": obs[i].features, "volatility": obs[i].volatility }).to_string(), crate::regime::RULE_VERSION, created]);
        }
        if let (Some(p), Some(cur)) = (prev, obs.last()) {
            if p != cur.regime {
                crate::events::emit(c, "MarketRegimeChanged", &format!("{}:{}:{}", r.market, r.symbol, r.interval), json!({ "from": p, "to": cur.regime, "confidence": cur.confidence, "rule_version": crate::regime::RULE_VERSION }), None, closed.t.last().map(|t| format!("regime:{}:{}:{t}", r.symbol, r.interval)).as_deref());
            }
        }
    });
    v["source"] = json!(crate::botapi::source_label(&r));
    v["symbol"] = json!(r.symbol);
    v["interval"] = json!(r.interval);
    Json(v).into_response()
}

pub async fn regimes_history(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let sym = q.get("symbol").map_or(String::new(), |x| x.to_uppercase());
    let iv = q.get("interval").cloned().unwrap_or_else(|| "1h".into());
    let r = s.journal.with_conn(|c| -> rusqlite::Result<Vec<Value>> {
        let mut st = c.prepare("SELECT bar_time, regime, confidence, features, rule_version FROM regime_observations WHERE symbol = ?1 AND interval = ?2 ORDER BY bar_time DESC LIMIT 300")?;
        let m = st.query_map(rusqlite::params![sym, iv], |r| Ok(json!({ "bar_time": r.get::<_, i64>(0)?, "regime": r.get::<_, String>(1)?, "confidence": r.get::<_, f64>(2)?, "features": serde_json::from_str::<Value>(&r.get::<_, String>(3)?).unwrap_or(Value::Null), "rule_version": r.get::<_, String>(4)? })))?;
        Ok(m.filter_map(Result::ok).collect())
    });
    out(r.map(|v| json!({ "symbol": sym, "interval": iv, "observations": v })).map_err(|e| e.to_string()))
}
