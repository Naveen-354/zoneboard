//! HTTP handlers for Bot Studio: bots, backtests and optimisation, the strategy library, risk limits, the
//! kill switch and the live gate. Routes are registered in `server.rs` (where the token check covers them),
//! and the background runner lives here too.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::params;
use serde_json::{json, Value};

use crate::backtest2::{self, Axis, Config};
use crate::bots::{self, BotError};
use crate::server::{err, json_body, Shared};
use crate::strategy;
use crate::zones::Frame;

const MAX_BACKTEST_BARS: usize = 5000;
const MAX_SAVED_EQUITY: usize = 1500;
const MAX_SAVED_TRADES: usize = 2000;
const RUNNER_BARS: usize = 500;
const RUNNER_EVERY: Duration = Duration::from_secs(20);

fn bot_err(e: BotError) -> Response {
    match e {
        BotError::NotFound => err(StatusCode::NOT_FOUND, "no such bot"),
        BotError::Invalid(m) => err(StatusCode::BAD_REQUEST, &m),
        BotError::Conflict(m) => err(StatusCode::CONFLICT, &m),
        BotError::Db(m) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("database error: {m}")),
    }
}

fn bot_out(s: &Shared, id: i64, status: StatusCode) -> Response {
    match s.journal.with_conn(|c| bots::load(c, id).and_then(|b| bots::to_json(c, &b, true))) {
        Ok(v) => (status, Json(v)).into_response(),
        Err(e) => bot_err(e),
    }
}

// ------------------------------------------------------------------ bots

pub async fn bots_list(State(s): State<Shared>) -> Response {
    match s.journal.with_conn(|c| bots::list(c)) {
        Ok(v) => Json(v).into_response(),
        Err(e) => bot_err(e),
    }
}

pub async fn bots_create(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    match s.journal.with_conn(|c| bots::create(c, &b)) {
        Ok(id) => bot_out(&s, id, StatusCode::CREATED),
        Err(e) => bot_err(e),
    }
}

pub async fn bot_get(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    bot_out(&s, id, StatusCode::OK)
}

pub async fn bot_update(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    match s.journal.with_conn(|c| bots::update(c, id, &b)) {
        Ok(()) => bot_out(&s, id, StatusCode::OK),
        Err(e) => bot_err(e),
    }
}

pub async fn bot_delete(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    match s.journal.with_conn(|c| bots::delete(c, id)) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => bot_err(e),
    }
}

pub async fn bot_duplicate(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    match s.journal.with_conn(|c| bots::duplicate(c, id)) {
        Ok(n) => bot_out(&s, n, StatusCode::CREATED),
        Err(e) => bot_err(e),
    }
}

pub async fn bot_start(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    match s.journal.with_conn(|c| bots::start(c, id, &s.gate, s.data.sample)) {
        Ok(()) => bot_out(&s, id, StatusCode::OK),
        Err(e) => bot_err(e),
    }
}

pub async fn bot_pause(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    let r = s.journal.with_conn(|c| {
        let b = bots::load(c, id)?;
        if b.status != "running" {
            return Err(BotError::Conflict("only a running bot can be paused".into()));
        }
        bots::set_status(c, id, "paused", "paused by you: open positions are still managed", "user")
    });
    match r {
        Ok(()) => bot_out(&s, id, StatusCode::OK),
        Err(e) => bot_err(e),
    }
}

/// Latest price of each leg of a bot, from the server's own data. `None` when there is no data.
async fn prices_for(s: &Shared, b: &bots::Bot) -> [Option<f64>; 2] {
    let one = |sym: String| {
        let s = s.clone();
        let (m, iv) = (b.market.clone(), b.interval.clone());
        async move { s.data.bars(&m, &sym, &iv, 2, s.data.sample, 10).await.ok().map(|f| f.c[f.len() - 1]) }
    };
    let a = one(b.symbol.clone()).await;
    let bb = match &b.symbol2 {
        Some(s2) => one(s2.clone()).await,
        None => None,
    };
    [a, bb]
}

pub async fn bot_stop(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    let b = match s.journal.with_conn(|c| bots::load(c, id)) {
        Ok(b) => b,
        Err(e) => return bot_err(e),
    };
    let prices = prices_for(&s, &b).await;
    match s.journal.with_conn(|c| bots::stop(c, id, prices, "stopped by you", "user")) {
        Ok(summary) => match s.journal.with_conn(|c| bots::load(c, id).and_then(|b| bots::to_json(c, &b, true))) {
            Ok(mut v) => {
                v["stop"] = summary;
                Json(v).into_response()
            }
            Err(e) => bot_err(e),
        },
        Err(e) => bot_err(e),
    }
}

pub async fn bot_logs(State(s): State<Shared>, Path(id): Path<i64>, Query(q): Query<HashMap<String, String>>) -> Response {
    let limit = q.get("limit").and_then(|x| x.parse::<usize>().ok()).unwrap_or(100).clamp(1, 500);
    let before = q.get("before").and_then(|x| x.parse::<i64>().ok());
    match s.journal.with_conn(|c| bots::logs(c, id, limit, before)) {
        Ok(v) => Json(v).into_response(),
        Err(e) => bot_err(e),
    }
}

pub async fn bot_readiness(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    match s.journal.with_conn(|c| bots::load(c, id).and_then(|b| bots::readiness(c, &b, &s.gate))) {
        Ok(v) => Json(v).into_response(),
        Err(e) => bot_err(e),
    }
}

/// `POST /api/bots/{id}/deploy-live`: always refused in this build, with the checklist saying why.
pub async fn bot_deploy_live(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    match s.journal.with_conn(|c| bots::load(c, id).and_then(|b| bots::readiness(c, &b, &s.gate))) {
        Ok(v) => (StatusCode::CONFLICT, Json(json!({ "error": "live deployment is not available in this build: no live order route exists", "readiness": v }))).into_response(),
        Err(e) => bot_err(e),
    }
}

pub async fn bot_backtests(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    let r = s.journal.with_conn(|c| -> Result<Value, BotError> {
        bots::load(c, id)?;
        let mut st = c.prepare("SELECT id, created_utc, request, metrics FROM backtests WHERE bot_id = ?1 ORDER BY id DESC LIMIT 50")?;
        let rows: Vec<Value> = st
            .query_map([id], |r| {
                let p = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
                Ok(json!({ "id": r.get::<_, i64>(0)?, "created_utc": r.get::<_, String>(1)?, "request": p(r.get(2)?), "metrics": p(r.get(3)?) }))
            })?
            .filter_map(Result::ok)
            .collect();
        Ok(json!({ "backtests": rows }))
    });
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => bot_err(e),
    }
}

pub async fn backtest_get(State(s): State<Shared>, Path(id): Path<i64>) -> Response {
    let r = s.journal.with_conn(|c| {
        c.query_row("SELECT bot_id, created_utc, request, report FROM backtests WHERE id = ?1", [id], |r| {
            let p = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
            Ok(json!({ "id": id, "bot_id": r.get::<_, Option<i64>>(0)?, "created_utc": r.get::<_, String>(1)?, "request": p(r.get(2)?), "report": p(r.get(3)?) }))
        })
    });
    match r {
        Ok(v) => Json(v).into_response(),
        Err(rusqlite::Error::QueryReturnedNoRows) => err(StatusCode::NOT_FOUND, "no such backtest"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ------------------------------------------------------------------ library

pub async fn library() -> Json<Value> {
    Json(json!({
        "indicators": crate::indicators::catalog(),
        "templates": strategy::templates(),
        "operators": ["gt", "lt", "gte", "lte", "crosses_above", "crosses_below", "rising", "falling"],
        "price_fields": ["open", "high", "low", "close", "volume", "hl2", "hlc3"],
        "intervals": { "crypto": bots::CRYPTO_INTERVALS, "india": bots::INDIA_INTERVALS },
        "execution": "Signals are read on a candle's close and filled at the next candle's open, with fees and slippage. Stops are checked before targets when both fall in one candle.",
    }))
}

// ------------------------------------------------------------------ backtests

struct BtReq {
    spec: Value,
    market: String,
    symbol: String,
    symbol2: Option<String>,
    interval: String,
    bars: usize,
    cfg: Config,
    bot_id: Option<i64>,
    save: bool,
    sample: bool,
}

fn bt_request(s: &Shared, b: &Value) -> Result<BtReq, Response> {
    let bad = |m: &str| err(StatusCode::BAD_REQUEST, m);
    let m = b.as_object().ok_or_else(|| bad("request body must be a JSON object"))?;
    let bot = match m.get("bot_id").and_then(Value::as_i64) {
        Some(id) => Some(s.journal.with_conn(|c| bots::load(c, id)).map_err(bot_err)?),
        None => None,
    };
    let text = |k: &str, from_bot: Option<String>| m.get(k).and_then(Value::as_str).map(|x| x.trim().to_string()).or(from_bot);
    let market = text("market", bot.as_ref().map(|b| b.market.clone())).unwrap_or_else(|| "crypto".into());
    if !["crypto", "india"].contains(&market.as_str()) {
        return Err(bad("market must be crypto or india"));
    }
    let symbol = text("symbol", bot.as_ref().map(|b| b.symbol.clone())).unwrap_or_default().to_uppercase();
    if !crate::data::valid_nse_symbol(&symbol) || (market == "crypto" && !symbol.ends_with("USDT")) {
        return Err(bad("symbol is not valid for this market"));
    }
    let symbol2 = text("symbol2", bot.as_ref().and_then(|b| b.symbol2.clone())).filter(|x| !x.is_empty()).map(|x| x.to_uppercase());
    if symbol2.as_ref().is_some_and(|x| !crate::data::valid_nse_symbol(x)) {
        return Err(bad("symbol2 is not valid"));
    }
    let interval = text("interval", bot.as_ref().map(|b| b.interval.clone())).unwrap_or_else(|| "1h".into());
    let allowed: &[&str] = if market == "crypto" { &bots::CRYPTO_INTERVALS } else { &bots::INDIA_INTERVALS };
    if !allowed.contains(&interval.as_str()) {
        return Err(bad(&format!("timeframe must be one of {}", allowed.join(", "))));
    }
    let spec = m.get("spec").cloned().or_else(|| bot.as_ref().map(|b| b.spec.clone())).unwrap_or(Value::Null);
    let num = |k: &str, d: f64, lo: f64, hi: f64| -> Result<f64, Response> {
        match m.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => {
                let x = v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or_else(|| bad(&format!("{k} must be a number")))?;
                if !(lo..=hi).contains(&x) {
                    return Err(bad(&format!("{k} must be between {lo} and {hi}")));
                }
                Ok(x)
            }
        }
    };
    let bars = num("bars", 1000.0, 100.0, MAX_BACKTEST_BARS as f64)? as usize;
    let alloc = bot.as_ref().and_then(|b| rust_decimal::prelude::ToPrimitive::to_f64(&b.allocation)).unwrap_or(10_000.0);
    let capital = num("capital", alloc, 1.0, 1e15)?;
    let fee_pct = num("fee_pct", crate::journal::fee_pct(&market), 0.0, 5.0)?;
    let slippage_pct = num("slippage_pct", bot.as_ref().map_or(0.05, |b| b.slippage_pct), 0.0, 5.0)?;
    let sample = match m.get("sample") {
        Some(Value::Bool(x)) => *x,
        _ => s.data.sample,
    };
    Ok(BtReq {
        spec,
        market: market.clone(),
        symbol,
        symbol2,
        interval,
        bars,
        cfg: Config { capital, fee_pct, slippage_pct, qty_step: [bots::qty_step(&market); 2], trade_from: 0 },
        bot_id: bot.as_ref().map(|b| b.id),
        save: m.get("save").and_then(Value::as_bool).unwrap_or(false),
        sample,
    })
}

async fn frames(s: &Shared, r: &BtReq) -> Result<(Arc<Frame>, Option<Arc<Frame>>), Response> {
    let a = s.data.bars(&r.market, &r.symbol, &r.interval, r.bars, r.sample, 300).await.map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("market data: {e}")))?;
    let b = match &r.symbol2 {
        Some(x) => Some(s.data.bars(&r.market, x, &r.interval, r.bars, r.sample, 300).await.map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("market data: {e}")))?),
        None => None,
    };
    Ok((a, b))
}

fn source_label(r: &BtReq) -> &'static str {
    if r.sample {
        "SAMPLE DATA: made-up prices, for development only"
    } else if r.market == "crypto" {
        "Binance public market data"
    } else {
        "Yahoo Finance (unofficial, delayed)"
    }
}

/// Price-scale indicator lines (drawn over the candles) for the chart.
fn overlays(st: &strategy::Strategy, f: &Frame) -> Value {
    const ON_PRICE: [&str; 8] = ["sma", "ema", "bollinger", "supertrend", "donchian", "ichimoku", "vwap", "atr_bands"];
    let mut out = vec![];
    for d in &st.indicators {
        if !ON_PRICE.contains(&d.kind.as_str()) {
            continue;
        }
        if let Ok(o) = crate::indicators::compute(&d.kind, &d.params, f) {
            for (name, v) in o {
                if matches!(name, "pct_b" | "width" | "direction" | "deviation_pct") {
                    continue;
                }
                let pts: Vec<Value> = f.t.iter().zip(&v).filter(|(_, x)| x.is_finite()).map(|(t, x)| json!([t, x])).collect();
                out.push(json!({ "id": format!("{}.{name}", d.id), "points": pts }));
            }
        }
    }
    Value::Array(out)
}

fn thin<T: Clone>(v: &[T], max: usize) -> Vec<T> {
    if v.len() <= max {
        return v.to_vec();
    }
    let step = v.len() as f64 / max as f64;
    let mut out: Vec<T> = (0..max).map(|i| v[(i as f64 * step) as usize].clone()).collect();
    out.push(v[v.len() - 1].clone());
    out
}

pub async fn backtest_run(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let r = match bt_request(&s, &b) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let st = match strategy::parse(&r.spec) {
        Ok(x) => x,
        Err(e) => return err(StatusCode::BAD_REQUEST, &format!("strategy: {e}")),
    };
    if (st.kind == strategy::Kind::Pairs) != r.symbol2.is_some() {
        return err(StatusCode::BAD_REQUEST, "pairs strategies need symbol2, and only they take one");
    }
    let (fa, fb) = match frames(&s, &r).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let report = match tokio::task::spawn_blocking({
        let (st, fa, fb, cfg) = (st.clone(), fa.clone(), fb.clone(), r.cfg);
        move || backtest2::run(&st, &fa, fb.as_deref(), &cfg)
    })
    .await
    {
        Ok(Ok(x)) => x,
        Ok(Err(e)) => return err(StatusCode::BAD_REQUEST, &e),
        Err(_) => return err(StatusCode::INTERNAL_SERVER_ERROR, "backtest crashed"),
    };
    let candles: Vec<Value> = (0..fa.len()).map(|i| json!([fa.t[i], fa.o[i], fa.h[i], fa.l[i], fa.c[i]])).collect();
    let data = json!({ "source": source_label(&r), "sample": r.sample, "market": r.market, "symbol": r.symbol, "symbol2": r.symbol2, "interval": r.interval, "bars": fa.len(), "from": fa.t.first(), "to": fa.t.last() });
    let request = json!({ "market": r.market, "symbol": r.symbol, "symbol2": r.symbol2, "interval": r.interval, "bars": r.bars, "capital": r.cfg.capital, "fee_pct": r.cfg.fee_pct, "slippage_pct": r.cfg.slippage_pct, "sample": r.sample, "spec": r.spec });
    let mut saved = None;
    if r.save {
        let stored = json!({
            "metrics": report.metrics,
            "trades": report.trades.iter().take(MAX_SAVED_TRADES).collect::<Vec<_>>(),
            "equity": thin(&report.equity, MAX_SAVED_EQUITY),
            "data": data,
        });
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let res = s.journal.with_conn(|c| -> rusqlite::Result<i64> {
            c.execute(
                "INSERT INTO backtests (bot_id, created_utc, request, metrics, report) VALUES (?1,?2,?3,?4,?5)",
                params![r.bot_id, now, request.to_string(), serde_json::to_string(&report.metrics).unwrap(), stored.to_string()],
            )?;
            let id = c.last_insert_rowid();
            if let Some(bid) = r.bot_id {
                bots::log(c, bid, "info", &format!("backtest #{id} saved: {} trades, return {}%", report.metrics.trades, report.metrics.return_pct));
            }
            Ok(id)
        });
        match res {
            Ok(id) => saved = Some(id),
            Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not save the backtest: {e}")),
        }
    }
    let mut out = backtest2::report_json(&report);
    out["equity"] = json!(thin(&report.equity, MAX_SAVED_EQUITY));
    out["candles"] = Value::Array(candles);
    out["overlays"] = overlays(&st, &fa);
    out["data"] = data;
    out["saved_id"] = json!(saved);
    out["assumptions"] = json!(format!(
        "Signals on candle close, fills at the next open. Fees {}% per side, slippage {}% on market fills. Stop before target inside one candle. Open positions at the end are marked to the last close but not counted as trades.",
        r.cfg.fee_pct, r.cfg.slippage_pct
    ));
    Json(out).into_response()
}

pub async fn backtest_optimize(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let r = match bt_request(&s, &b) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let axes: Vec<Axis> = match b.get("params").and_then(Value::as_array) {
        Some(list) => {
            let mut out = vec![];
            for p in list {
                let (Some(path), Some(values)) = (p.get("path").and_then(Value::as_str), p.get("values").and_then(Value::as_array)) else {
                    return err(StatusCode::BAD_REQUEST, "each param needs a path (JSON pointer) and a list of values");
                };
                out.push(Axis { path: path.to_string(), values: values.clone() });
            }
            out
        }
        None => return err(StatusCode::BAD_REQUEST, "params must be a list of {path, values}"),
    };
    let split = b.get("split").and_then(Value::as_f64).unwrap_or(0.7);
    let objective = b.get("objective").and_then(Value::as_str).unwrap_or("sharpe").to_string();
    if let Err(e) = strategy::parse(&r.spec) {
        return err(StatusCode::BAD_REQUEST, &format!("strategy: {e}"));
    }
    let (fa, fb) = match frames(&s, &r).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let spec = r.spec.clone();
    let cfg = r.cfg;
    let out = tokio::task::spawn_blocking(move || backtest2::optimize(&spec, &axes, &fa, fb.as_deref(), &cfg, split, &objective)).await;
    match out {
        Ok(Ok(mut v)) => {
            v["data"] = json!({ "source": source_label(&r), "sample": r.sample, "symbol": r.symbol, "interval": r.interval });
            Json(v).into_response()
        }
        Ok(Err(e)) => err(StatusCode::BAD_REQUEST, &e),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "optimisation crashed"),
    }
}

// ------------------------------------------------------------------ risk, kill switch, live gate

pub async fn risk_limits_get(State(s): State<Shared>, Path(account): Path<String>) -> Response {
    match s.journal.with_conn(|c| crate::risk::limits_json(c, &account)) {
        Ok(v) => Json(v).into_response(),
        Err(e) if e == "no such account" => err(StatusCode::NOT_FOUND, &e),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

pub async fn risk_limits_put(State(s): State<Shared>, Path(account): Path<String>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    match s.journal.with_conn(|c| crate::risk::set_limits(c, &account, &b)) {
        Ok(v) => Json(v).into_response(),
        Err(e) if e == "no such account" => err(StatusCode::NOT_FOUND, &e),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn risk_status(State(s): State<Shared>) -> Response {
    match s.journal.with_conn(|c| crate::risk::live_status(c, &s.gate)) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

/// `POST /api/risk/kill {"engaged": true}`: stop every bot and close its positions at the latest price,
/// disarm live trading, and block new orders until released with `{"engaged": false}`.
pub async fn risk_kill(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    let Some(on) = b.get("engaged").and_then(Value::as_bool) else {
        return err(StatusCode::BAD_REQUEST, "send {\"engaged\": true} or {\"engaged\": false}");
    };
    s.gate.set_kill(on);
    let mut stopped = vec![];
    if on {
        let ids = s.journal.with_conn(|c| bots::ids_with_status(c, &["running", "paused"]));
        for id in ids {
            let Ok(bot) = s.journal.with_conn(|c| bots::load(c, id)) else { continue };
            let prices = prices_for(&s, &bot).await;
            if let Ok(v) = s.journal.with_conn(|c| bots::stop(c, id, prices, "kill switch", "user")) {
                stopped.push(json!({ "bot_id": id, "name": bot.name, "result": v }));
            }
        }
    }
    let _ = s.journal.with_conn(|c| crate::audit::record(c, "user", if on { "kill_switch_on" } else { "kill_switch_off" }, "system", &json!({ "stopped": stopped })));
    match s.journal.with_conn(|c| crate::risk::live_status(c, &s.gate)) {
        Ok(mut v) => {
            v["stopped"] = Value::Array(stopped);
            Json(v).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

pub async fn live_arm(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    match s.journal.with_conn(|c| crate::risk::arm(c, &s.gate, &b)) {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(StatusCode::CONFLICT, &e),
    }
}

pub async fn live_disarm(State(s): State<Shared>) -> Response {
    s.gate.disarm();
    let _ = s.journal.with_conn(|c| crate::audit::record(c, "user", "live_disarmed", "system", &json!({})));
    risk_status(State(s)).await
}

// ------------------------------------------------------------------ the runner

/// Is anything due for this bot? Only a new candle close, an order waiting for a candle's open, a first
/// baseline, or a stale displayed price need a data fetch.
fn due(b: &bots::Bot, now: i64) -> bool {
    let Some(iv) = bots::interval_secs(&b.interval) else { return false };
    let e = &b.state.engine;
    let stale_price = b.last_price_utc.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_none_or(|t| now - t.timestamp() >= 60);
    match e.last_closed {
        None => true,
        Some(lc) => now >= lc + 2 * iv || (!e.pending.is_empty() && e.last_opened.is_none_or(|lo| now >= lo + iv)) || stale_price,
    }
}

pub async fn tick(s: &Shared) {
    let now = chrono::Utc::now().timestamp();
    let ids = s.journal.with_conn(|c| bots::ids_with_status(c, &["running", "paused"]));
    for id in ids {
        let Ok(b) = s.journal.with_conn(|c| bots::load(c, id)) else { continue };
        if !due(&b, now) {
            continue;
        }
        let fa = s.data.bars(&b.market, &b.symbol, &b.interval, RUNNER_BARS, false, 10).await;
        let fb = match &b.symbol2 {
            Some(x) => Some(s.data.bars(&b.market, x, &b.interval, RUNNER_BARS, false, 10).await),
            None => None,
        };
        let res = match (fa, fb) {
            (Ok(a), None) => s.journal.with_conn(|c| bots::process(c, &s.gate, id, &a, None, now)),
            (Ok(a), Some(Ok(bb))) => s.journal.with_conn(|c| bots::process(c, &s.gate, id, &a, Some(&bb), now)),
            (Err(e), _) | (_, Some(Err(e))) => Err(format!("market data: {e}")),
        };
        if let Err(e) = res {
            s.journal.with_conn(|c| {
                let same = b.last_error.as_deref() == Some(e.as_str());
                let _ = c.execute("UPDATE bots SET last_error = ?1 WHERE id = ?2", params![e, id]);
                if !same {
                    bots::log(c, id, "error", &e);
                }
            });
        }
    }
}

/// Runs for the life of the server. Bots that are not running or paused cost nothing.
pub async fn run_forever(s: Shared) {
    let mut every = tokio::time::interval(RUNNER_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        every.tick().await;
        tick(&s).await;
    }
}
