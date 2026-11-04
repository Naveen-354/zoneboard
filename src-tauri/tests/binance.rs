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
