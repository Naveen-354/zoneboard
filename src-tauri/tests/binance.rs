//! T007 acceptance: connecting a Binance account against a local fake exchange. No network, no real keys.

mod common;

use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use zoneboard::connections::Endpoints;
use zoneboard::server::{app, AppState};
use zoneboard::vault::{MemoryStore, SecretStore};

use common::fake_binance::{self, KEY, SECRET};

struct T {
    router: axum::Router,
    token: String,
    vault: Arc<MemoryStore>,
    knobs: fake_binance::Shared,
}

async fn setup() -> T {
    let (url, knobs) = fake_binance::spawn().await;
    let vault = Arc::new(MemoryStore::new());
    let state = AppState::with_parts(true, ":memory:", vault.clone(), Endpoints { binance_testnet: url.clone(), binance_live: url }).unwrap();
    T { token: state.api_token().to_string(), router: app(state), vault, knobs }
}

async fn call(t: &T, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value, String) {
    let b = Request::builder().method(method).uri(uri).header(header::HOST, "localhost").header("X-ZB-Token", &t.token);
    let req = match body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = t.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 10_000_000).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

fn connect_body(env: &str) -> Value {
    json!({ "kind": "binance", "environment": env, "label": "Test", "api_key": KEY, "api_secret": SECRET })
}

fn no_secret(text: &str) {
    assert!(!text.contains(SECRET), "secret leaked: {text}");
    assert!(!text.contains(KEY), "api key leaked: {text}");
}

#[tokio::test]
async fn testnet_connect_balances_verify_disconnect_without_leaking_secrets() {
    let t = setup().await;
    let (s, acct, text) = call(&t, "POST", "/api/accounts", Some(connect_body("testnet"))).await;
    assert_eq!(s, StatusCode::CREATED, "{text}");
    no_secret(&text);
    assert_eq!(acct["kind"], "binance");
    assert_eq!(acct["virtual"], false, "a broker account is never labelled virtual");
    assert_eq!(acct["key_hint"], "…WXYZ");
    let id = acct["id"].as_str().unwrap().to_string();
    // the credentials are in the vault, not the database
    assert!(t.vault.exists(&id, "api_secret").unwrap());

    let (s, bal, text) = call(&t, "GET", &format!("/api/accounts/{id}/balances"), None).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    no_secret(&text);
    assert_eq!(bal["virtual"], false);
    assert_eq!(bal["balances"][1]["free"], "10000.12345678", "the broker's decimal text, untouched");
    assert_eq!(bal["used_weight_1m"], 20);

    let (s, _, text) = call(&t, "POST", &format!("/api/accounts/{id}/verify"), None).await;
    assert_eq!(s, StatusCode::OK, "{text}");

    // nothing secret in the audit trail either
    let (_, _, audit) = call(&t, "GET", "/api/audit?limit=100", None).await;
    no_secret(&audit);
    assert!(audit.contains("account_connected") && audit.contains("account_verified"));

    // bots cannot trade it through the paper path: the risk gate refuses non-paper accounts
    let (s, _, _) = call(&t, "DELETE", &format!("/api/accounts/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!t.vault.exists(&id, "api_secret").unwrap(), "disconnect deletes the vault entries");
    let (s, _, _) = call(&t, "GET", &format!("/api/accounts/{id}/balances"), None).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn bad_key_rate_limit_and_skew_are_handled() {
    let t = setup().await;
    let mut bad = connect_body("testnet");
    bad["api_secret"] = json!("WRONGSECRETcccccccccccccccccccccccccccccccc");
    let (s, e, text) = call(&t, "POST", "/api/accounts", Some(bad)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{text}");
    assert!(e["error"].as_str().unwrap().contains("rejected the key"), "{e}");
    assert!(!text.contains("WRONGSECRET"));
    // a short or odd credential is refused before any network call, without echoing it
    let mut odd = connect_body("testnet");
    odd["api_key"] = json!("short");
    let (s, _, text) = call(&t, "POST", "/api/accounts", Some(odd)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(!text.contains("short\""));

    // the server clock is 3 s ahead: the client syncs and signs with the corrected time
    t.knobs.lock().unwrap().skew_ms = 3000;
    let (s, acct, text) = call(&t, "POST", "/api/accounts", Some(connect_body("testnet"))).await;
    assert_eq!(s, StatusCode::CREATED, "{text}");
    let id = acct["id"].as_str().unwrap().to_string();

    // 429: a clear error with the pause, no retry storm, and the last success is reported
    t.knobs.lock().unwrap().rate_limit = true;
    let before = t.knobs.lock().unwrap().calls.len();
    tokio::time::sleep(std::time::Duration::from_millis(10_100)).await; // past the 10 s balance cache
    let (s, e, _) = call(&t, "GET", &format!("/api/accounts/{id}/balances"), None).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    let msg = e["error"].as_str().unwrap();
    assert!(msg.contains("rate limited") && msg.contains("last successful sync"), "{msg}");
    let (s, _, _) = call(&t, "GET", &format!("/api/accounts/{id}/balances"), None).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY);
    assert_eq!(t.knobs.lock().unwrap().calls.len() - before, 1, "the second call fails fast during the pause, without hitting the exchange");
}

#[tokio::test]
async fn live_needs_confirmation_and_withdrawal_keys_are_refused() {
    let t = setup().await;
    let (s, e, _) = call(&t, "POST", "/api/accounts", Some(connect_body("live"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("confirm_live"));
    t.knobs.lock().unwrap().withdrawals = true;
    let mut b = connect_body("live");
    b["confirm_live"] = json!(true);
    let (s, e, _) = call(&t, "POST", "/api/accounts", Some(b.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("can withdraw funds"));
    let (_, list, _) = call(&t, "GET", "/api/accounts", None).await;
    assert_eq!(list["accounts"].as_array().unwrap().len(), 1, "nothing stored for a withdrawal-enabled key");
    // a key without withdrawal permission connects (to the fake: tests never reach a live host)
    t.knobs.lock().unwrap().withdrawals = false;
    let (s, acct, text) = call(&t, "POST", "/api/accounts", Some(b)).await;
    assert_eq!(s, StatusCode::CREATED, "{text}");
    assert_eq!(acct["environment"], "live");
}


/// A bot on a connected Spot Testnet account trades through the risk gate and the order outbox, against the
/// fake exchange: deterministic client order ids, no duplicate on a repeated step, a lost reply reconciled by
/// lookup, a close recorded from the exchange's fill, and the kill switch blocking new entries.
#[tokio::test]
async fn testnet_bot_trades_through_the_outbox_without_duplicates() {
    use zoneboard::execution::Intent;
    let (url, knobs) = fake_binance::spawn().await;
    let state = AppState::with_parts(false, ":memory:", Arc::new(MemoryStore::new()), Endpoints { binance_testnet: url.clone(), binance_live: url }).unwrap();
    let t = T { token: state.api_token().to_string(), router: app(state.clone()), vault: Arc::new(MemoryStore::new()), knobs: knobs.clone() };
    let (s, acct, text) = call(&t, "POST", "/api/accounts", Some(connect_body("testnet"))).await;
    assert_eq!(s, StatusCode::CREATED, "{text}");
    let account = acct["id"].as_str().unwrap().to_string();

    let spec = json!({ "entry": {"long": {"left": {"price": "close"}, "op": "gt", "right": {"value": 0}}}, "sizing": {"type": "fixed_notional", "value": 100} });
    let (s, bot, text) = call(&t, "POST", "/api/bots", Some(json!({ "name": "Testnet bot", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "1000", "account_id": account }))).await;
    assert_eq!(s, StatusCode::CREATED, "{text}");
    assert_eq!(bot["mode"], "live", "a broker account means broker trading");
    let id = bot["id"].as_i64().unwrap();
    // a short strategy cannot run on spot
    let (s, _, _) = call(&t, "POST", "/api/bots", Some(json!({ "name": "Shorty", "symbol": "BTCUSDT", "spec": { "direction": "short", "entry": {"short": {"left": {"price": "close"}, "op": "gt", "right": {"value": 0}}} }, "allocation": "100", "account_id": account }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // the lifecycle: no testnet trading before a saved backtest of this exact version
    let (s, e, _) = call(&t, "POST", &format!("/api/bots/{id}/start"), None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(e["error"].as_str().unwrap().contains("backtest"), "{e}");
    let (s, _, text) = call(&t, "POST", "/api/backtest", Some(json!({ "bot_id": id, "bars": 300, "save": true, "sample": true }))).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    let (s, b, text) = call(&t, "POST", &format!("/api/bots/{id}/start"), None).await;
    assert_eq!(s, StatusCode::OK, "{text}");
    assert_eq!(b["stage"], "paper");

    let b = state.journal().with_conn(|c| zoneboard::bots::load(c, id)).unwrap();
    let open = |bar: i64| Intent::Open { qty: 0.0021234, ref_price: 50_000.0, bar_time: bar, stop: None, target: None, trail: None, tag: "signal".into() };
    let filled = zoneboard::botapi::run_intents(&state, &b, vec![open(1_000)]).await;
    assert_eq!(filled, 1);
    {
        let k = knobs.lock().unwrap();
        assert_eq!(k.orders.len(), 1);
        let o = k.orders.get(&format!("zb{id}-1000-0")).expect("deterministic client order id");
        assert_eq!(o["executedQty"], "0.00212000", "quantity rounded down to the 0.00001 lot step");
    }
    // the same intent again (a replayed step): not sent a second time
    assert_eq!(zoneboard::botapi::run_intents(&state, &b, vec![open(1_000)]).await, 0);
    assert_eq!(knobs.lock().unwrap().orders.len(), 1);

    // a lost reply: the order reached the exchange but the answer did not; the lookup records the fill
    knobs.lock().unwrap().drop_order_reply = true;
    assert_eq!(zoneboard::botapi::run_intents(&state, &b, vec![open(2_000)]).await, 1);
    knobs.lock().unwrap().drop_order_reply = false;
    let (positions, live_mode): (i64, i64) = state.journal().with_conn(|c| {
        (c.query_row("SELECT count(*) FROM positions WHERE bot_id = ?1", [id], |r| r.get(0)).unwrap(), c.query_row("SELECT count(*) FROM positions WHERE bot_id = ?1 AND mode = 'live' AND account_id = ?2", rusqlite::params![id, account], |r| r.get(0)).unwrap())
    });
    assert_eq!((positions, live_mode), (2, 2));

    // close one: a SELL through the outbox, the trade recorded at the exchange's price
    let pos: i64 = state.journal().with_conn(|c| c.query_row("SELECT min(id) FROM positions WHERE bot_id = ?1", [id], |r| r.get(0)).unwrap());
    knobs.lock().unwrap().price = "51000".into();
    assert_eq!(zoneboard::botapi::run_intents(&state, &b, vec![Intent::Close { position_id: pos, bar_time: 3_000, reason: "exit signal".into() }]).await, 1);
    let (exit, side): (f64, String) = state.journal().with_conn(|c| c.query_row("SELECT exit, side FROM trades WHERE bot_id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap());
    assert_eq!((exit, side.as_str()), (51_000.0, "long"));

    // the kill switch blocks new entries before anything is sent
    state.gate().set_kill(true);
    assert_eq!(zoneboard::botapi::run_intents(&state, &b, vec![open(4_000)]).await, 0);
    assert_eq!(knobs.lock().unwrap().orders.len(), 3, "2 opens + 1 close; nothing after the kill switch");
    let refused: String = state.journal().with_conn(|c| c.query_row("SELECT reason FROM bot_orders WHERE bot_id = ?1 AND status = 'rejected' ORDER BY id DESC LIMIT 1", [id], |r| r.get(0)).unwrap());
    assert!(refused.contains("kill switch"), "{refused}");
    // the outbox has no order left in an uncertain state
    let unknown: i64 = state.journal().with_conn(|c| c.query_row("SELECT count(*) FROM broker_orders WHERE status IN ('submitting','unknown')", [], |r| r.get(0)).unwrap());
    assert_eq!(unknown, 0);
}
