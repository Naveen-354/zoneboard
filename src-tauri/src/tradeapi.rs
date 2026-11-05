//! HTTP handlers for phase 12: multi-account orchestration, smart execution (paper), options lab, ledger.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use crate::opsapi::out;
use crate::server::{err, json_body, Shared};

// ------------------------------------------------------------------ accounts

/// `POST /api/accounts/{id}/status {status: active|disabled}`. A disabled account takes no new orders on any
/// path (the central risk check refuses them); open positions can still be closed.
pub async fn account_status(State(s): State<Shared>, Path(id): Path<String>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| -> Result<Value, String> {
        let to = b["status"].as_str().unwrap_or("");
        if !["active", "disabled"].contains(&to) {
            return Err("status must be active or disabled".into());
        }
        let n = c.execute("UPDATE accounts SET status = ?2 WHERE id = ?1", rusqlite::params![id, to]).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("no such account".into());
        }
        let _ = crate::audit::record(c, "user", &format!("account_{to}"), &format!("account:{id}"), &json!({}));
        Ok(json!({ "id": id, "status": to }))
    }))
}

/// One view across accounts: environment and status, open positions, bots by status, working smart orders,
/// and the account's risk limits. Demo credits stay labelled VIRTUAL (the accounts payload carries the label).
pub async fn orchestrator(State(s): State<Shared>) -> Response {
    out(s.journal.with_conn(|c| -> Result<Value, String> {
        let list = crate::accounts::list(c).map_err(|e| format!("{e:?}"))?;
        let mut rows = vec![];
        for a in list["accounts"].as_array().into_iter().flatten() {
            let id = a["id"].as_str().unwrap_or("");
            let count = |sql: &str| c.query_row(sql, [id], |r| r.get::<_, i64>(0)).unwrap_or(0);
            let bots: HashMap<String, i64> = c
                .prepare("SELECT status, count(*) FROM bots WHERE account_id = ?1 GROUP BY status")
                .and_then(|mut st| st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).map(|m| m.filter_map(Result::ok).collect()))
                .unwrap_or_default();
            let limits: Vec<Value> = c
                .prepare("SELECT currency, max_order_notional, max_daily_loss, max_exposure, max_position_notional, max_open_positions FROM account_limits WHERE account_id = ?1")
                .and_then(|mut st| {
                    st.query_map([id], |r| Ok(json!({ "currency": r.get::<_, String>(0)?, "max_order_notional": r.get::<_, Option<String>>(1)?, "max_daily_loss": r.get::<_, Option<String>>(2)?, "max_exposure": r.get::<_, Option<String>>(3)?, "max_position_notional": r.get::<_, Option<String>>(4)?, "max_open_positions": r.get::<_, Option<i64>>(5)? })))
                        .map(|m| m.filter_map(Result::ok).collect())
                })
                .unwrap_or_default();
            rows.push(json!({
                "account": a, "open_positions": count("SELECT count(*) FROM positions WHERE account_id = ?1"),
                "bots": bots, "working_smart_orders": count("SELECT count(*) FROM parent_orders WHERE account_id = ?1 AND status IN ('working','paused')"), "limits": limits,
            }));
        }
        Ok(json!({ "accounts": rows, "portfolio": crate::riskintel::view(c), "notice": list["notice"] }))
    }))
}

/// Pause every running bot on one account (a per-account brake; the global kill switch stays separate).
pub async fn orchestrator_pause(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    out(s.journal.with_conn(|c| -> Result<Value, String> {
        let ids: Vec<i64> = c.prepare("SELECT id FROM bots WHERE account_id = ?1 AND status = 'running'").and_then(|mut st| st.query_map([&id], |r| r.get(0)).map(|m| m.filter_map(Result::ok).collect())).map_err(|e| e.to_string())?;
        for b in &ids {
            crate::bots::set_status(c, *b, "paused", "paused from the account orchestrator", "user").map_err(|e| format!("{e:?}"))?;
        }
        Ok(json!({ "account_id": id, "paused": ids }))
    }))
}

// ------------------------------------------------------------------ smart execution

pub async fn smart_create(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let r = match crate::smartexec::parse(&b) {
        Ok(r) => r,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e),
    };
    let res = s.journal.with_conn(|c| crate::smartexec::create(c, &r, chrono::Utc::now(), |c, o| crate::risk::check(c, o, &s.gate)));
    if let Err(e) = &res {
        s.journal.with_conn(|c| crate::riskintel::on_rejection(c, "smart_execution", &r.account_id, &r.symbol, e));
    }
    match res {
        Ok(v) => {
            // the first child is due now: fill it straight away rather than wait for the runner's next tick
            smart_tick(&s).await;
            let id = v["id"].as_i64().unwrap_or(0);
            (StatusCode::CREATED, Json(s.journal.with_conn(|c| crate::smartexec::get(c, id)).unwrap_or(v))).into_response()
        }
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn smart_list(State(s): State<Shared>) -> Response {
    out(s.journal.with_conn(|c| crate::smartexec::list(c)))
}

pub async fn smart_get(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::smartexec::get(c, id)))
}

pub async fn smart_cancel(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::smartexec::cancel(c, id)))
}

pub async fn smart_resume(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    out(s.journal.with_conn(|c| crate::smartexec::resume(c, id, chrono::Utc::now())))
}

/// Fill due child orders: the latest closed price is the paper fill price, and each child goes through the
/// same `open_position_checked` + `risk::check` as a manual order. Called by the runner and by tests.
pub async fn smart_tick(s: &Shared) {
    let now = chrono::Utc::now();
    let due = s.journal.with_conn(|c| crate::smartexec::due(c, now));
    for d in due {
        let iv = if d.market == "crypto" { "5m" } else { "15m" };
        let price = match s.data.bars(&d.market, &d.symbol, iv, 100, s.data.sample, 30).await {
            Ok(f) if !f.c.is_empty() => *f.c.last().unwrap(),
            _ => {
                // no price: try again on the next tick rather than fill blind
                continue;
            }
        };
        let outcome = if !crate::smartexec::within_limit(&d.side, price, d.limit_price) {
            crate::smartexec::Outcome::Skipped(format!("price {price} outside the limit {}", d.limit_price.unwrap_or(0.0)))
        } else {
            let body = json!({ "account_id": d.account_id, "market": d.market, "mode": "demo", "symbol": d.symbol, "side": d.side, "qty": d.qty, "price": price });
            match s.journal.open_position_checked(&body, |c, o| crate::risk::check(c, o, &s.gate)) {
                Ok(p) => crate::smartexec::Outcome::Filled { position_id: p.id, price },
                Err(e) => crate::smartexec::Outcome::Rejected(e),
            }
        };
        s.journal.with_conn(|c| crate::smartexec::record(c, &d, outcome, now));
    }
}

// ------------------------------------------------------------------ options lab

pub async fn options_price(headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let i = match crate::options::inputs_from(&b) {
        Ok(i) => i,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e),
    };
    let g = crate::options::bs(&i);
    let iv = b.get("market_price").and_then(Value::as_f64).map(|p| crate::options::implied_vol(&i, p));
    let r4 = |x: f64| (x * 1e4).round() / 1e4;
    Json(json!({
        "label": "THEORETICAL (Black-Scholes, your inputs; not a quote and not advice)",
        "price": r4(g.price), "delta": r4(g.delta), "gamma": (g.gamma * 1e6).round() / 1e6, "vega_per_vol_point": r4(g.vega), "theta_per_day": r4(g.theta), "rho_per_rate_point": r4(g.rho),
        "implied_vol_pct": iv.map(|v| v.map(|x| r4(x * 100.0))),
    }))
    .into_response()
}

/// `POST /api/options/strategy {spot, days, vol_pct, rate_pct, div_pct, multiplier, legs | template, width}`
pub async fn options_strategy(headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let r = (|| -> Result<Value, String> {
        let base = crate::options::inputs_from(&b)?;
        let legs = match b.get("template").and_then(Value::as_str) {
            Some(t) => crate::options::template(t, base.spot, b.get("width").and_then(Value::as_f64).unwrap_or((base.spot * 0.05).max(0.01)), &base)?,
            None => crate::options::legs_from(&b)?,
        };
        let mult = b.get("multiplier").and_then(Value::as_f64).unwrap_or(1.0);
        if !(mult > 0.0 && mult <= 100_000.0) {
            return Err("multiplier must be between 0 and 100000".into());
        }
        let mut a = crate::options::analyse(&legs, base.spot, &base, mult)?;
        a["legs"] = json!(legs.iter().map(crate::options::leg_json).collect::<Vec<_>>());
        a["templates"] = json!(crate::options::TEMPLATES);
        Ok(a)
    })();
    out(r)
}

// ------------------------------------------------------------------ ledger

pub async fn ledger_entries(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    out(s.journal.with_conn(|c| -> Result<Value, String> {
        let added = crate::ledger::sync(c)?;
        let e = crate::ledger::entries(c, q.get("account").map(String::as_str), q.get("from").map(String::as_str), q.get("to").map(String::as_str), 1000)?;
        Ok(json!({ "entries": e, "synced_now": added, "kinds": crate::ledger::KINDS }))
    }))
}

pub async fn ledger_adjust(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let amount = b["amount"].as_str().map(String::from).or_else(|| b["amount"].as_f64().map(|x| x.to_string())).unwrap_or_default();
    out(s.journal.with_conn(|c| crate::ledger::adjust(c, b["account_id"].as_str().unwrap_or(""), b["currency"].as_str().unwrap_or(""), &amount, b["memo"].as_str().unwrap_or(""))))
}

pub async fn ledger_reverse(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    out(s.journal.with_conn(|c| crate::ledger::reverse(c, id, b["memo"].as_str().unwrap_or(""))))
}

pub async fn ledger_report(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let basis = q.get("basis").cloned().unwrap_or_else(|| "calendar".into());
    out(s.journal.with_conn(|c| {
        crate::ledger::sync(c)?;
        crate::ledger::report(c, &basis, q.get("account").map(String::as_str))
    }))
}

pub async fn ledger_export(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let r = s.journal.with_conn(|c| {
        crate::ledger::sync(c)?;
        crate::ledger::export_csv(c, q.get("account").map(String::as_str), q.get("from").map(String::as_str), q.get("to").map(String::as_str))
    });
    match r {
        Ok(csv) => ([(header::CONTENT_TYPE, "text/csv; charset=utf-8"), (header::CONTENT_DISPOSITION, "attachment; filename=\"zoneboard-ledger.csv\"")], csv).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn ledger_reconcile(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let csv = b["csv"].as_str().unwrap_or("");
    if csv.len() > 2_000_000 {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "statement is too large (2 MB at most)");
    }
    out(s.journal.with_conn(|c| crate::ledger::reconcile(c, b["account_id"].as_str().unwrap_or(""), csv)))
}
