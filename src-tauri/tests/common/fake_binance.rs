//! A local fake of the Binance Spot REST API for tests. It checks signatures the way Binance does (hex
//! HMAC-SHA256 of the query before `&signature=`), enforces `recvWindow` against its own (skewed) clock, and can
//! be told to rate-limit, reject the key, or report a withdrawal-enabled key. Never a real network call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

pub const KEY: &str = "FAKEKEYaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaWXYZ";
pub const SECRET: &str = "FAKESECRETbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[derive(Default)]
pub struct Knobs {
    pub skew_ms: i64,
    pub rate_limit: bool,
    pub withdrawals: bool,
    pub calls: Vec<String>,
    /// client order id -> order JSON (idempotency: a repeated id is refused like Binance does)
    pub orders: HashMap<String, Value>,
    pub price: String,
    /// accept the order but never answer (an unknown outcome for the client)
    pub drop_order_reply: bool,
}

pub type Shared = Arc<Mutex<Knobs>>;

fn server_ms(k: &Knobs) -> i64 {
    chrono::Utc::now().timestamp_millis() + k.skew_ms
}

fn err(status: StatusCode, code: i64, msg: &str) -> Response {
    (status, Json(json!({ "code": code, "msg": msg }))).into_response()
}

fn params(q: &str) -> HashMap<String, String> {
    q.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Binance's checks for a SIGNED call, in its order.
fn check_signed(k: &Knobs, headers: &HeaderMap, q: &str) -> Result<HashMap<String, String>, Response> {
    if k.rate_limit {
        return Err((StatusCode::TOO_MANY_REQUESTS, [("Retry-After", "7")], Json(json!({ "code": -1003, "msg": "Too many requests." }))).into_response());
    }
    if headers.get("x-mbx-apikey").and_then(|v| v.to_str().ok()) != Some(KEY) {
        return Err(err(StatusCode::UNAUTHORIZED, -2015, "Invalid API-key, IP, or permissions for action."));
    }
    let Some((body, sig)) = q.rsplit_once("&signature=") else { return Err(err(StatusCode::BAD_REQUEST, -1102, "Mandatory parameter 'signature' was not sent.")) };
    if zoneboard::exchange::binance::sign(SECRET, body) != sig {
        return Err(err(StatusCode::BAD_REQUEST, -1022, "Signature for this request is not valid."));
    }
    let p = params(body);
    let ts: i64 = p.get("timestamp").and_then(|t| t.parse().ok()).unwrap_or(0);
    let rw: i64 = p.get("recvWindow").and_then(|t| t.parse().ok()).unwrap_or(5000);
    let now = server_ms(k);
    if ts > now + 1000 || now - ts > rw {
        return Err(err(StatusCode::BAD_REQUEST, -1021, "Timestamp for this request is outside of the recvWindow."));
    }
    Ok(p)
}

pub async fn spawn() -> (String, Shared) {
    let knobs: Shared = Arc::new(Mutex::new(Knobs { price: "50000".into(), ..Default::default() }));
    let app = Router::new()
        .route("/api/v3/time", get(|State(k): State<Shared>| async move { let k = k.lock().unwrap(); Json(json!({ "serverTime": server_ms(&k) })) }))
        .route(
            "/api/v3/account",
            get(|State(k): State<Shared>, h: HeaderMap, RawQuery(q): RawQuery| async move {
                let mut k = k.lock().unwrap();
                k.calls.push("account".into());
                if let Err(r) = check_signed(&k, &h, &q.unwrap_or_default()) {
                    return r;
                }
                ([("X-MBX-USED-WEIGHT-1M", "20")], Json(json!({
                    "canTrade": true, "canWithdraw": true, "accountType": "SPOT", "permissions": ["SPOT"],
                    "balances": [{ "asset": "BTC", "free": "1.00000000", "locked": "0.00000000" }, { "asset": "USDT", "free": "10000.12345678", "locked": "5.00000000" }]
                }))).into_response()
            }),
        )
        .route(
            "/sapi/v1/account/apiRestrictions",
            get(|State(k): State<Shared>, h: HeaderMap, RawQuery(q): RawQuery| async move {
                let k = k.lock().unwrap();
                if let Err(r) = check_signed(&k, &h, &q.unwrap_or_default()) {
                    return r;
                }
                Json(json!({ "ipRestrict": true, "enableReading": true, "enableWithdrawals": k.withdrawals, "enableSpotAndMarginTrading": true })).into_response()
            }),
        )
        .route(
            "/api/v3/exchangeInfo",
            get(|| async {
                Json(json!({ "symbols": [{ "symbol": "BTCUSDT", "status": "TRADING", "filters": [
                    { "filterType": "LOT_SIZE", "minQty": "0.00001000", "maxQty": "9000.00000000", "stepSize": "0.00001000" },
                    { "filterType": "NOTIONAL", "minNotional": "5.00000000" }
                ] }] }))
            }),
        )
        .route(
            "/api/v3/order",
            get(|State(k): State<Shared>, h: HeaderMap, RawQuery(q): RawQuery| async move {
                let k = k.lock().unwrap();
                let p = match check_signed(&k, &h, &q.unwrap_or_default()) {
                    Ok(p) => p,
                    Err(r) => return r,
                };
                match p.get("origClientOrderId").and_then(|id| k.orders.get(id)) {
                    Some(o) => Json(o.clone()).into_response(),
                    None => err(StatusCode::BAD_REQUEST, -2013, "Order does not exist."),
                }
            })
            .post(|State(k): State<Shared>, h: HeaderMap, RawQuery(q): RawQuery| async move {
                let mut k = k.lock().unwrap();
                let p = match check_signed(&k, &h, &q.unwrap_or_default()) {
                    Ok(p) => p,
                    Err(r) => return r,
                };
                k.calls.push("order".into());
                let id = p.get("newClientOrderId").cloned().unwrap_or_default();
                if k.orders.contains_key(&id) {
                    return err(StatusCode::BAD_REQUEST, -2010, "Duplicate order sent.");
                }
                let qty: f64 = p.get("quantity").and_then(|x| x.parse().ok()).unwrap_or(0.0);
                let px: f64 = k.price.parse().unwrap();
                let o = json!({
                    "symbol": p.get("symbol"), "orderId": k.orders.len() as i64 + 1, "clientOrderId": id, "status": "FILLED",
                    "executedQty": format!("{qty:.8}"), "cummulativeQuoteQty": format!("{:.8}", qty * px), "side": p.get("side"),
                    "fills": [{ "price": k.price, "qty": format!("{qty:.8}"), "commission": "0", "commissionAsset": "USDT" }]
                });
                k.orders.insert(id, o.clone());
                if k.drop_order_reply {
                    return (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response();
                }
                Json(o).into_response()
            }),
        )
        .with_state(knobs.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, knobs)
}
