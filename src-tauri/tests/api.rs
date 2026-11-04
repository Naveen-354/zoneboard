//! HTTP-level tests of the local server: the same contract the Flask app had (see test_app.py).
//! Runs against the in-process router with synthetic data, so no network is needed.

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;
use zoneboard::server::{app, AppState};

struct TestApp {
    router: Router,
    token: String,
}

impl TestApp {
    fn new() -> Self {
        let state = AppState::new(true, ":memory:").unwrap();
        let token = state.api_token().to_string();
        Self {
            router: app(state),
            token,
        }
    }
}

fn router() -> TestApp {
    TestApp::new()
}

async fn do_call(
    app: &TestApp,
    method: &str,
    uri: &str,
    host: &str,
    json_body: Option<Value>,
    extra: &[(&str, &str)],
    send_token: bool,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri).header(header::HOST, host);
    let mut has_token = false;
    for (k, v) in extra {
        if k.eq_ignore_ascii_case("x-zb-token") {
            has_token = true;
        }
        b = b.header(*k, *v);
    }
    if send_token && !has_token {
        b = b.header("X-ZB-Token", &app.token);
    }
    let req = match json_body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    (parts.status, parts.headers, to_bytes(body, 10_000_000).await.unwrap().to_vec())
}

async fn call(
    app: &TestApp,
    method: &str,
    uri: &str,
    host: &str,
    json_body: Option<Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    do_call(app, method, uri, host, json_body, extra, true).await
}

async fn call_without_token(
    app: &TestApp,
    method: &str,
    uri: &str,
    host: &str,
    json_body: Option<Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    do_call(app, method, uri, host, json_body, extra, false).await
}

async fn get_json(app: &TestApp, uri: &str) -> (StatusCode, Value) {
    let (s, _, b) = call(app, "GET", uri, "localhost", None, &[]).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

#[tokio::test]
async fn config_lists_crypto_symbols() {
    let (s, c) = get_json(&router(), "/api/config").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["sample"], true); // this test server runs on made-up sample data
    let syms = c["crypto"]["symbols"].as_array().unwrap();
    assert!(syms.iter().any(|x| x == "BTCUSDT") && syms.len() >= 20);
    assert!(c["india"]["count"].as_u64().unwrap() > 10);
    assert_eq!(c["fees"], json!({"crypto": 0.1, "india": 0.05})); // simulated fee per side, percent
}

#[tokio::test]
async fn sample_data_flag_per_request() {
    // the server default is sample data here; ?sample=1 is explicit. (?sample=0 would fetch real NSE data: not in unit tests.)
    let r = router();
    assert_eq!(get_json(&r, "/api/scan").await.1["sample"], true);
    assert_eq!(get_json(&r, "/api/scan?sample=1").await.1["sample"], true);
    assert_eq!(get_json(&r, "/api/chart/SBIN?interval=1d&sample=1").await.0, StatusCode::OK);
}

/// An order on the demo account (paper trading on real prices), with `extra` fields overriding the defaults.
fn order(extra: Value) -> Value {
    let mut o = json!({"market": "crypto", "mode": "demo", "symbol": "BTCUSDT", "side": "long", "qty": 0.01, "price": 78000});
    for (k, v) in extra.as_object().unwrap() {
        o[k] = v.clone();
    }
    o
}

#[tokio::test]
async fn paper_positions_open_close_and_reach_the_demo_journal() {
    let r = router();
    let (s, _, b) = call(&r, "POST", "/api/positions", "localhost", Some(order(json!({}))), &[]).await;
    assert_eq!(s, StatusCode::CREATED);
    let pos: Value = serde_json::from_slice(&b).unwrap();
    let id = pos["position"]["id"].as_i64().unwrap();
    assert_eq!(pos["position"]["fee_pct"], 0.1);
    assert_eq!(pos["position"]["entry"], 78000.0);
    assert_eq!(pos["position"]["mode"], "demo");

    assert_eq!(get_json(&r, "/api/positions?market=crypto&mode=demo").await.1["positions"].as_array().unwrap().len(), 1);

    let (s, _, b) = call(&r, "POST", &format!("/api/positions/{id}/close"), "localhost", Some(json!({"price": 79000, "date": "2026-11-02"})), &[]).await;
    assert_eq!(s, StatusCode::OK);
    let t: Value = serde_json::from_slice(&b).unwrap();
    let fees = 0.1 / 100.0 * 0.01 * (78000.0 + 79000.0);
    assert!((t["trade"]["fees"].as_f64().unwrap() - fees).abs() < 1e-9);
    assert_eq!(t["trade"]["pnl"].as_f64().unwrap(), ((0.01 * 1000.0 - fees) * 100.0).round() / 100.0);
    assert_eq!((t["trade"]["date"].as_str(), t["trade"]["mode"].as_str()), (Some("2026-11-02"), Some("demo")));

    assert_eq!(call(&r, "POST", &format!("/api/positions/{id}/close"), "localhost", Some(json!({"price": 79000})), &[]).await.0, StatusCode::NOT_FOUND);
    assert_eq!(get_json(&r, "/api/positions?market=crypto&mode=demo").await.1["positions"].as_array().unwrap().len(), 0);
    assert_eq!(get_json(&r, "/api/journal?market=crypto&mode=demo").await.1["trades"].as_array().unwrap().len(), 1);
    assert_eq!(get_json(&r, "/api/journal?market=crypto&mode=live").await.1["trades"].as_array().unwrap().len(), 0); // never touched the live account
}

#[tokio::test]
async fn live_account_refuses_orders_and_bad_orders_are_refused() {
    let r = router();
    // no broker is connected, so a "live" order would claim a real trade that never happened
    let (s, _, b) = call(&r, "POST", "/api/positions", "localhost", Some(order(json!({"mode": "live"}))), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&b).contains("live orders"));
    assert_eq!(get_json(&r, "/api/positions?market=crypto&mode=live").await.1["positions"].as_array().unwrap().len(), 0);

    for bad in [json!({"price": 0}), json!({"price": "nan"}), json!({"qty": 0}), json!({"side": "buy"}), json!({"market": "forex"}), json!({"mode": "paper"}), json!({"symbol": "../etc"}), json!({"market": "india", "qty": 1.5})] {
        assert_eq!(call(&r, "POST", "/api/positions", "localhost", Some(order(bad.clone())), &[]).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(get_json(&r, "/api/positions?market=forex").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get_json(&r, "/api/positions?market=crypto&mode=paper").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "POST", "/api/positions/999999/close", "localhost", Some(json!({"price": 1})), &[]).await.0, StatusCode::NOT_FOUND);

    // like the journal: a body not declared as JSON is refused
    let req = Request::builder().method("POST").uri("/api/positions").header(header::HOST, "localhost").header("X-ZB-Token", &r.token).header(header::CONTENT_TYPE, "text/plain").body(Body::from(order(json!({})).to_string())).unwrap();
    assert_eq!(r.router.clone().oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn static_files_have_module_safe_mime_types_and_caching() {
    let r = router();
    for (path, mime) in [("/static/js/main.js", "text/javascript"), ("/static/js/markets/crypto.js", "text/javascript"), ("/static/css/app.css", "text/css"), ("/static/vendor/lightweight-charts.5.2.1.js", "text/javascript")] {
        let (s, h, b) = call(&r, "GET", path, "localhost", None, &[]).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert!(h[header::CONTENT_TYPE].to_str().unwrap().starts_with(mime), "{path}");
        assert!(!b.is_empty());
    }
    let (_, h, _) = call(&r, "GET", "/static/vendor/lightweight-charts.5.2.1.js", "localhost", None, &[]).await;
    assert!(h[header::CACHE_CONTROL].to_str().unwrap().contains("immutable"));

    // conditional requests: the unchanged file comes back as 304 with no body
    let (_, h, _) = call(&r, "GET", "/static/js/main.js", "localhost", None, &[]).await;
    let etag = h[header::ETAG].to_str().unwrap().to_string();
    let (s, _, b) = call(&r, "GET", "/static/js/main.js", "localhost", None, &[("if-none-match", &etag)]).await;
    assert_eq!(s, StatusCode::NOT_MODIFIED);
    assert!(b.is_empty());

    for bad in ["/static/../Cargo.toml", "/static/..%2FCargo.toml", "/static/nope.js", "/static/%2e%2e/src/lib.rs"] {
        assert_eq!(call(&r, "GET", bad, "localhost", None, &[]).await.0, StatusCode::NOT_FOUND, "{bad}");
    }
}

#[tokio::test]
async fn index_sets_csp_and_nosniff() {
    let (s, h, _) = call(&router(), "GET", "/", "127.0.0.1:5000", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("script-src 'self'") && csp.contains("wss://data-stream.binance.vision"));
    assert_eq!(h["x-content-type-options"], "nosniff");
}

#[tokio::test]
async fn rejects_foreign_host_header() {
    let r = router();
    assert_eq!(call(&r, "GET", "/api/config", "evil.example.com", None, &[]).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&r, "GET", "/api/config", "evil.example.com:5000", None, &[]).await.0, StatusCode::FORBIDDEN);
    for ok in ["127.0.0.1:5000", "localhost", "localhost:5000", "[::1]:5000", "[::1]"] {
        assert_eq!(call(&r, "GET", "/api/config", ok, None, &[]).await.0, StatusCode::OK, "{ok}");
    }
}

#[tokio::test]
async fn scan_and_chart_shapes_for_the_ui() {
    let r = router();
    let (s, scan) = get_json(&r, "/api/scan").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(scan["sample"], true);
    let total = scan["counts"]["total"].as_u64().unwrap();
    assert!(total > 10);
    let all = scan["all"].as_array().unwrap();
    assert_eq!(all.len() as u64, total);
    let mut keys: Vec<&String> = all[0].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["change_pct", "label", "price", "score", "symbol"]);
    assert!(scan["bullish"].as_array().unwrap().iter().all(|x| x["score"].as_f64().unwrap() >= 15.0));
    let first = &scan["bullish"][0];
    for k in ["symbol", "price", "bias", "score", "label", "change_pct", "rvol", "atr", "support", "resistance", "liquidity", "ob"] {
        assert!(first.get(k).is_some(), "scan row is missing {k}");
    }

    let sym = all[0]["symbol"].as_str().unwrap();
    for interval in ["1d", "1h", "15m"] {
        let (s, ch) = get_json(&r, &format!("/api/chart/{sym}?interval={interval}")).await;
        assert_eq!(s, StatusCode::OK);
        let candles = ch["candles"].as_array().unwrap();
        assert!(candles.len() > 40 && ch.get("summary").is_some());
        assert!(candles.iter().all(|c| c.get("volume").is_some()));
        assert!(ch["zones"].as_array().unwrap().iter().all(|z| z["top"].as_f64() > z["bottom"].as_f64()));
    }
    // times are true unix seconds (the browser localises them), not India-shifted
    let (_, ch) = get_json(&r, &format!("/api/chart/{sym}?interval=1d")).await;
    let last = ch["candles"].as_array().unwrap().last().unwrap()["time"].as_i64().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    assert!((now - last).abs() < 3 * 86400);

    assert_eq!(get_json(&r, "/api/chart/..%2Fetc?interval=1d").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get_json(&r, "/api/chart/SBIN?interval=bad").await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn journal_is_per_market_and_validated() {
    let r = router();
    let base = json!({"date":"2026-11-02","side":"long","entry":100,"exit":110,"fees":1});
    let post = |extra: Value| {
        let mut b = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        b
    };
    let (s, _, _) = call(&r, "POST", "/api/journal", "localhost", Some(post(json!({"market":"crypto","symbol":"btcusdt","qty":0.0153}))), &[]).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _, _) = call(&r, "POST", "/api/journal", "localhost", Some(post(json!({"market":"india","symbol":"SBIN","qty":10}))), &[]).await;
    assert_eq!(s, StatusCode::CREATED);

    let (_, crypto) = get_json(&r, "/api/journal?market=crypto").await;
    let (_, india) = get_json(&r, "/api/journal?market=india").await;
    assert_eq!(crypto["trades"].as_array().unwrap().len(), 1);
    assert_eq!(crypto["trades"][0]["symbol"], "BTCUSDT");
    assert_eq!(crypto["trades"][0]["qty"], 0.0153);
    assert!((crypto["stats"]["net_pnl"].as_f64().unwrap() - (0.0153 * 10.0 - 1.0)).abs() < 0.01);
    assert_eq!(india["trades"].as_array().unwrap().len(), 1);
    assert_eq!(get_json(&r, "/api/journal").await.1["trades"][0]["symbol"], "SBIN"); // market defaults to india

    for bad in [json!({"market":"forex","symbol":"X","qty":1}), json!({"market":"india","symbol":"X","qty":1.5}), json!({"market":"crypto","symbol":"X","qty":0}), json!({"market":"crypto","symbol":"X","qty":"nan"}), json!({"market":"crypto","symbol":"X","qty":"inf"})] {
        assert_eq!(call(&r, "POST", "/api/journal", "localhost", Some(post(bad.clone())), &[]).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(get_json(&r, "/api/journal?market=forex").await.0, StatusCode::BAD_REQUEST);

    // a body that isn't declared as JSON is refused (a cross-site form post can't send this content type)
    let req = Request::builder().method("POST").uri("/api/journal").header(header::HOST, "localhost").header("X-ZB-Token", &r.token).header(header::CONTENT_TYPE, "text/plain").body(Body::from(post(json!({"market":"crypto","symbol":"X","qty":1})).to_string())).unwrap();
    assert_eq!(r.router.clone().oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);

    let id = crypto["trades"][0]["id"].as_i64().unwrap();
    assert_eq!(call(&r, "DELETE", &format!("/api/journal/{id}"), "localhost", None, &[]).await.0, StatusCode::OK);
    assert_eq!(get_json(&r, "/api/journal?market=crypto").await.1["trades"].as_array().unwrap().len(), 0);
}

/// Single source of truth for tested API routes: (method, path_template, concrete_uri, body)
const API_ROUTES: &[(&str, &str, &str, Option<&str>)] = &[
    ("GET", "/api/config", "/api/config", None),
    ("GET", "/api/scan", "/api/scan", None),
    ("GET", "/api/chart/{symbol}", "/api/chart/SBIN?interval=1d&sample=1", None),
    ("GET", "/api/journal", "/api/journal?market=india", None),
    (
        "POST",
        "/api/journal",
        "/api/journal",
        Some(r#"{"market":"india","symbol":"SBIN","date":"2026-11-02","side":"long","qty":1,"entry":100,"exit":110,"fees":0}"#),
    ),
    ("DELETE", "/api/journal/{id}", "/api/journal/999999", None),
    ("GET", "/api/positions", "/api/positions?market=crypto&mode=demo", None),
    (
        "POST",
        "/api/positions",
        "/api/positions",
        Some(r#"{"market":"crypto","mode":"demo","symbol":"BTCUSDT","side":"long","qty":0.01,"price":78000}"#),
    ),
    (
        "POST",
        "/api/positions/{id}/close",
        "/api/positions/999999/close",
        Some(r#"{"price": 79000}"#),
    ),
    ("GET", "/api/audit", "/api/audit?limit=5", None),
    ("GET", "/api/settings", "/api/settings", None),
    ("PUT", "/api/settings", "/api/settings", Some(r#"{"timezone":"UTC"}"#)),
    ("GET", "/api/accounts", "/api/accounts", None),
    ("POST", "/api/accounts", "/api/accounts", Some(r#"{"kind":"demo","label":"Second"}"#)),
    ("PATCH", "/api/accounts/{id}", "/api/accounts/demo-1", Some(r#"{"label":"Renamed"}"#)),
    ("POST", "/api/accounts/{id}/reset", "/api/accounts/demo-1/reset", None),
    ("GET", "/api/bots", "/api/bots", None),
    ("POST", "/api/bots", "/api/bots", Some(r#"{"name":"x"}"#)),
    ("GET", "/api/bots/{id}", "/api/bots/999999", None),
    ("PUT", "/api/bots/{id}", "/api/bots/999999", Some(r#"{"name":"x"}"#)),
    ("DELETE", "/api/bots/{id}", "/api/bots/999999", None),
    ("POST", "/api/bots/{id}/duplicate", "/api/bots/999999/duplicate", None),
    ("POST", "/api/bots/{id}/start", "/api/bots/999999/start", None),
    ("POST", "/api/bots/{id}/pause", "/api/bots/999999/pause", None),
    ("POST", "/api/bots/{id}/stop", "/api/bots/999999/stop", None),
    ("GET", "/api/bots/{id}/logs", "/api/bots/999999/logs", None),
    ("GET", "/api/bots/{id}/backtests", "/api/bots/999999/backtests", None),
    ("GET", "/api/bots/{id}/live", "/api/bots/999999/live", None),
    ("POST", "/api/bots/{id}/deploy-live", "/api/bots/999999/deploy-live", None),
    ("GET", "/api/strategies/library", "/api/strategies/library", None),
    ("POST", "/api/backtest/walkforward", "/api/backtest/walkforward", Some(r#"{"symbol":"BTCUSDT"}"#)),
    ("GET", "/api/backtests/{id}/trades.csv", "/api/backtests/999999/trades.csv", None),
    ("GET", "/api/strategies", "/api/strategies", None),
    ("GET", "/api/approvals", "/api/approvals", None),
    ("POST", "/api/bots/{id}/approval", "/api/bots/999999/approval", None),
    ("POST", "/api/approvals/{id}/decide", "/api/approvals/999999/decide", Some(r#"{"approve":false}"#)),
    ("POST", "/api/approvals/{id}/revoke", "/api/approvals/999999/revoke", None),
    ("GET", "/api/system/health", "/api/system/health", None),
    ("GET", "/api/ai/status", "/api/ai/status", None),
    ("GET", "/api/scanner", "/api/scanner", None),
    ("GET", "/api/portfolio", "/api/portfolio", None),
    ("GET", "/api/portfolio/trades.csv", "/api/portfolio/trades.csv", None),
    ("POST", "/api/scanner/run", "/api/scanner/run", Some(r#"{"template":"nope"}"#)),
    ("GET", "/api/scanner/history", "/api/scanner/history", None),
    ("PUT", "/api/ai/key", "/api/ai/key", Some(r#"{"api_key":"x"}"#)),
    ("DELETE", "/api/ai/key", "/api/ai/key", None),
    ("POST", "/api/ai/settings", "/api/ai/settings", Some(r#"{}"#)),
    ("POST", "/api/ai/draft", "/api/ai/draft", Some(r#"{"objective":"x"}"#)),
    ("POST", "/api/ai/explain/{id}", "/api/ai/explain/999999", None),
    ("POST", "/api/ai/review", "/api/ai/review", Some(r#"{}"#)),
    ("GET", "/api/ai/proposals", "/api/ai/proposals", None),
    ("POST", "/api/strategies", "/api/strategies", Some(r#"{"name":"x"}"#)),
    ("POST", "/api/strategies/import", "/api/strategies/import", Some(r#"{}"#)),
    ("GET", "/api/strategies/{id}/versions", "/api/strategies/999999/versions", None),
    ("GET", "/api/strategy-versions/{id}/export", "/api/strategy-versions/999999/export", None),
    ("GET", "/api/market-data/health", "/api/market-data/health?market=crypto&symbol=BTCUSDT&interval=1h", None),
    ("DELETE", "/api/accounts/{id}", "/api/accounts/demo-1", None),
    ("GET", "/api/accounts/{id}/balances", "/api/accounts/demo-1/balances", None),
    ("POST", "/api/accounts/{id}/verify", "/api/accounts/demo-1/verify", None),
    ("POST", "/api/backtest", "/api/backtest", Some(r#"{"symbol":"BTCUSDT","spec":{"kind":"nope"}}"#)),
    ("POST", "/api/backtest/optimize", "/api/backtest/optimize", Some(r#"{"symbol":"BTCUSDT"}"#)),
    ("GET", "/api/backtests/{id}", "/api/backtests/999999", None),
    ("GET", "/api/risk", "/api/risk", None),
    ("POST", "/api/risk/kill", "/api/risk/kill", Some(r#"{"engaged":false}"#)),
    ("GET", "/api/risk/limits/{account}", "/api/risk/limits/demo-1", None),
    ("PUT", "/api/risk/limits/{account}", "/api/risk/limits/demo-1", Some(r#"{}"#)),
    ("POST", "/api/live/arm", "/api/live/arm", Some(r#"{"confirm":"no"}"#)),
    ("POST", "/api/live/disarm", "/api/live/disarm", None),
];

#[tokio::test]
async fn api_token_route_coverage_table() {
    let app = router();
    for &(method, _path_template, uri, body_str) in API_ROUTES {
        let body: Option<Value> = body_str.map(|s| serde_json::from_str(s).expect("valid json in API_ROUTES"));

        // 1. Missing token -> 401
        let (s_none, _, b_none) = call_without_token(&app, method, uri, "localhost", body.clone(), &[]).await;
        assert_eq!(s_none, StatusCode::UNAUTHORIZED, "route {method} {uri} should require token");
        let v_none: Value = serde_json::from_slice(&b_none).unwrap();
        assert!(v_none["error"].is_string(), "route {method} {uri} missing error string on 401");

        // 2. Wrong token -> 401
        let (s_wrong, _, b_wrong) = call(&app, method, uri, "localhost", body.clone(), &[("X-ZB-Token", "invalid-token-12345")]).await;
        assert_eq!(s_wrong, StatusCode::UNAUTHORIZED, "route {method} {uri} should reject wrong token");
        let v_wrong: Value = serde_json::from_slice(&b_wrong).unwrap();
        assert!(v_wrong["error"].is_string(), "route {method} {uri} missing error string on wrong token");

        // 3. Right token -> normal behaviour (not 401)
        let (s_right, _, _) = call(&app, method, uri, "localhost", body.clone(), &[]).await;
        assert_ne!(s_right, StatusCode::UNAUTHORIZED, "route {method} {uri} should accept valid token");
    }
}

#[tokio::test]
async fn api_token_unregistered_prefix_returns_401_without_token() {
    let app = router();
    // Path matching no route under /api
    let (s_no_auth, _, _) = call_without_token(&app, "GET", "/api/__unregistered__", "localhost", None, &[]).await;
    assert_eq!(s_no_auth, StatusCode::UNAUTHORIZED);

    let (s_wrong, _, _) = call(&app, "GET", "/api/__unregistered__", "localhost", None, &[("X-ZB-Token", "wrong")]).await;
    assert_eq!(s_wrong, StatusCode::UNAUTHORIZED);

    // With valid token, unmapped route returns normal 404
    let (s_auth, _, _) = call(&app, "GET", "/api/__unregistered__", "localhost", None, &[]).await;
    assert_eq!(s_auth, StatusCode::NOT_FOUND);
}

#[test]
fn source_scan_all_routes_are_protected_or_allowed() {
    let source = include_str!("../src/server.rs");
    const PUBLIC_ALLOW_LIST: &[&str] = &["/", "/static/{*path}"];

    let mut found_routes = Vec::new();
    for (idx, _) in source.match_indices(".route(\"") {
        let rest = &source[idx + ".route(\"".len()..];
        if let Some(end) = rest.find('"') {
            found_routes.push(&rest[..end]);
        }
    }

    assert!(!found_routes.is_empty(), "expected to find registered routes in server.rs");

    for route in &found_routes {
        if PUBLIC_ALLOW_LIST.contains(route) {
            continue;
        }
        assert!(
            route.starts_with("/api/"),
            "route {route} does not start with /api/ and is not in the public allow-list"
        );
        assert!(
            API_ROUTES.iter().any(|(_, template, _, _)| template == route),
            "route {route} starts with /api/ but is missing as a template in API_ROUTES"
        );
    }

    for (_, template, _, _) in API_ROUTES {
        assert!(
            found_routes.contains(template),
            "template {template} in API_ROUTES was not found in server.rs"
        );
    }
}

#[tokio::test]
async fn csrf_cross_origin_rejected_on_state_changing_methods() {
    let app = router();
    // Valid token + foreign Origin on POST -> 403
    let (s, _, _) = call(&app, "POST", "/api/positions", "localhost:5000", Some(order(json!({}))), &[("Origin", "https://evil.example.com")]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _, _) = call(&app, "POST", "/api/positions", "localhost:5000", Some(order(json!({}))), &[("Origin", "http://evil.example.com:5000")]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _, _) = call(&app, "POST", "/api/positions", "localhost:5000", Some(order(json!({}))), &[("Origin", "null")]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // PATCH and reset on accounts with a foreign Origin -> 403, and nothing changes
    let (s, _, _) = call(&app, "PATCH", "/api/accounts/demo-1", "localhost:5000", Some(json!({"label":"x"})), &[("Origin", "https://evil.example.com")]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(&app, "POST", "/api/accounts/demo-1/reset", "localhost:5000", None, &[("Origin", "https://evil.example.com")]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // DELETE on journal with foreign Origin -> 403
    let (s, _, _) = call(&app, "DELETE", "/api/journal/1", "localhost:5000", None, &[("Origin", "http://evil.com")]).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Same-origin Origin on state-changing request -> OK (not 403)
    let (s, _, _) = call(&app, "POST", "/api/positions", "localhost:5000", Some(order(json!({}))), &[("Origin", "http://localhost:5000")]).await;
    assert_ne!(s, StatusCode::FORBIDDEN);

    // Missing Origin on state-changing request -> OK (not 403)
    let (s, _, _) = call(&app, "POST", "/api/positions", "localhost:5000", Some(order(json!({}))), &[]).await;
    assert_ne!(s, StatusCode::FORBIDDEN);

    // Non-state-changing method GET with foreign Origin -> OK (Origin check only applies to state-changing methods)
    let (s, _, _) = call(&app, "GET", "/api/config", "localhost:5000", None, &[("Origin", "https://evil.example.com")]).await;
    assert_eq!(s, StatusCode::OK);
}

#[test]
fn api_token_differs_per_instance_and_constant_time_comparison() {
    let s1 = AppState::new(true, ":memory:").unwrap();
    let s2 = AppState::new(true, ":memory:").unwrap();

    let t1 = s1.api_token();
    let t2 = s2.api_token();

    assert_ne!(t1, t2, "tokens must differ across server instances");
    assert_eq!(t1.len(), 64, "token must be 256 bits (64 hex characters)");
    assert_eq!(t2.len(), 64);
    assert!(t1.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    assert!(t2.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));

    // Constant-time comparison (subtle)
    assert!(zoneboard::server::constant_time_compare(t1, t1));
    assert!(!zoneboard::server::constant_time_compare(t1, t2));
    assert!(!zoneboard::server::constant_time_compare(t1, ""));
    assert!(!zoneboard::server::constant_time_compare(t1, &t1[..32]));
}

#[tokio::test]
async fn index_serves_token_in_meta_tag_with_no_store_and_no_token_leak_in_headers() {
    let app = router();
    let (s, headers, body) = call_without_token(&app, "GET", "/", "127.0.0.1:5000", None, &[]).await;
    assert_eq!(s, StatusCode::OK);

    // Cache-Control must be no-store
    let cache_control = headers.get(header::CACHE_CONTROL).expect("Cache-Control header missing").to_str().unwrap();
    assert!(cache_control.contains("no-store"), "Cache-Control must contain no-store: {cache_control}");

    // No ETag on GET /
    assert!(headers.get(header::ETAG).is_none(), "GET / must not include ETag");

    // No header name or header value may contain the token
    for (name, val) in &headers {
        let val_str = val.to_str().unwrap_or("");
        assert!(
            !val_str.contains(&app.token),
            "Header {name} leaked token in value: {val_str}"
        );
        assert!(
            !name.as_str().contains(&app.token),
            "Header name {name} leaked token"
        );
    }

    // Body contains the token in <meta name="zb-token" content="...">
    let body_str = String::from_utf8(body).unwrap();
    let expected_tag = format!(r#"<meta name="zb-token" content="{}">"#, app.token);
    assert!(body_str.contains(&expected_tag), "HTML body missing expected meta tag: {expected_tag}");

    // Test that 401 error response does not leak token
    let (_, err_headers, err_body) = call_without_token(&app, "GET", "/api/config", "localhost", None, &[]).await;
    for (name, val) in &err_headers {
        assert!(!val.to_str().unwrap_or("").contains(&app.token));
        assert!(!name.as_str().contains(&app.token));
    }
    let err_body_str = String::from_utf8(err_body).unwrap();
    assert!(!err_body_str.contains(&app.token), "401 body leaked token: {err_body_str}");
}

#[test]
fn logging_redacts_api_token_header() {
    // Note 7: add a test that logging::is_secret_field("X-ZB-Token") is true
    assert!(zoneboard::logging::is_secret_field("X-ZB-Token"));
    assert!(zoneboard::logging::is_secret_field("x-zb-token"));
    assert!(zoneboard::logging::is_secret_field("X_ZB_TOKEN"));
}

#[tokio::test]
async fn audit_log_records_app_start_and_paginates_with_cursor() {
    let r = router();

    // AppState::new records an app_start entry on startup
    let (s, res) = get_json(&r, "/api/audit").await;
    assert_eq!(s, StatusCode::OK);
    let entries = res["entries"].as_array().unwrap();
    // newest first: app_start, then migration 4's account_created for the default demo account
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["action"], "app_start");
    assert_eq!(entries[0]["actor"], "system");
    assert_eq!(entries[0]["entity"], "system");
    assert_eq!(entries[1]["action"], "account_created");
    assert_eq!(entries[1]["entity"], "account:demo-1");
    assert_eq!(res["next_before"], Value::Null);

    // Query validation tests: invalid limits and cursors return 400 JSON
    assert_eq!(get_json(&r, "/api/audit?limit=0").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get_json(&r, "/api/audit?limit=abc").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get_json(&r, "/api/audit?before=abc").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get_json(&r, "/api/audit?before=0").await.0, StatusCode::BAD_REQUEST);

    // Generate 3 settings updates to test cursor pagination
    let timezones = ["Asia/Kolkata", "America/New_York", "Europe/London"];
    for tz in timezones {
        let (st, _, _) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({ "timezone": tz })), &[]).await;
        assert_eq!(st, StatusCode::OK);
    }

    // Total audit entries: 1 account_created + 1 app_start + 3 settings_update = 5 entries
    let (s, res) = get_json(&r, "/api/audit?limit=2").await;
    assert_eq!(s, StatusCode::OK);
    let page1 = res["entries"].as_array().unwrap();
    assert_eq!(page1.len(), 2);
    let next_cursor = res["next_before"].as_i64().unwrap();
    assert_eq!(next_cursor, page1[1]["id"].as_i64().unwrap());

    // Page 2
    let (s, res) = get_json(&r, &format!("/api/audit?limit=2&before={next_cursor}")).await;
    assert_eq!(s, StatusCode::OK);
    let page2 = res["entries"].as_array().unwrap();
    assert_eq!(page2.len(), 2);
    let next_cursor = res["next_before"].as_i64().unwrap();

    // Page 3: the one remaining entry, then no cursor
    let (s, res) = get_json(&r, &format!("/api/audit?limit=2&before={next_cursor}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(res["entries"].as_array().unwrap().len(), 1);
    assert_eq!(res["next_before"], Value::Null);
}

#[tokio::test]
async fn settings_get_and_put_lifecycle() {
    let r = router();

    // Default settings match spec
    let (s, res) = get_json(&r, "/api/settings").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(res["timezone"], "UTC");
    assert_eq!(res["notifications.desktop"], false);
    assert_eq!(res["preferences.default_account"], Value::Null);

    // Update settings
    let update_payload = json!({
        "timezone": "Asia/Kolkata",
        "notifications.desktop": true,
        "preferences.default_account": "demo"
    });
    let (s, _, b) = call(&r, "PUT", "/api/settings", "localhost", Some(update_payload), &[]).await;
    assert_eq!(s, StatusCode::OK);
    let updated: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(updated["timezone"], "Asia/Kolkata");
    assert_eq!(updated["notifications.desktop"], true);
    assert_eq!(updated["preferences.default_account"], "demo");

    // GET verifies persisted changes
    let (s, res) = get_json(&r, "/api/settings").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(res["timezone"], "Asia/Kolkata");
    assert_eq!(res["notifications.desktop"], true);
    assert_eq!(res["preferences.default_account"], "demo");

    // Verify audit entry contains old and new values
    let (s, audit) = get_json(&r, "/api/audit?limit=1").await;
    assert_eq!(s, StatusCode::OK);
    let latest = &audit["entries"].as_array().unwrap()[0];
    assert_eq!(latest["action"], "settings_update");
    assert_eq!(latest["actor"], "user");
    assert_eq!(latest["entity"], "settings");
    assert_eq!(latest["detail"]["old"]["timezone"], "UTC");
    assert_eq!(latest["detail"]["new"]["timezone"], "Asia/Kolkata");
    assert_eq!(latest["detail"]["old"]["notifications.desktop"], false);
    assert_eq!(latest["detail"]["new"]["notifications.desktop"], true);
    assert_eq!(latest["detail"]["old"]["preferences.default_account"], Value::Null);
    assert_eq!(latest["detail"]["new"]["preferences.default_account"], "demo");
}

#[tokio::test]
async fn settings_put_validation_and_all_or_nothing() {
    let r = router();

    // Invalid timezone -> 400
    let (s, _, b) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({ "timezone": "Mars/Olympus" })), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&b).contains("invalid timezone"));

    // Case matters for timezone -> 400 (asia/kolkata must still be a 400)
    let (s, _, b) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({ "timezone": "asia/kolkata" })), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&b).contains("invalid timezone"));

    // live_armed -> 400 with runtime-only message
    let (s, _, b) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({ "live_armed": true })), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let msg = String::from_utf8_lossy(&b);
    assert!(msg.contains("live_armed") && msg.contains("runtime-only"), "msg: {msg}");

    // Unknown setting key -> 400
    let (s, _, b) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({ "some_unknown_key": "val" })), &[]).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&b).contains("unknown setting key"));

    // All-or-nothing test: valid setting combined with invalid key must NOT update anything
    let (s, _, _) = call(
        &r,
        "PUT",
        "/api/settings",
        "localhost",
        Some(json!({ "timezone": "America/New_York", "invalid_key": 42 })),
        &[],
    ).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Setting remains at default UTC
    let (s, res) = get_json(&r, "/api/settings").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(res["timezone"], "UTC");
}

#[tokio::test]
async fn settings_unchanged_values_write_nothing() {
    let r = router();

    // 1. Initial PUT sets timezone to Asia/Kolkata
    let (s, _, _) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({ "timezone": "Asia/Kolkata" })), &[]).await;
    assert_eq!(s, StatusCode::OK);

    // Verify audit log has exactly 1 settings_update entry
    let (s, audit) = get_json(&r, "/api/audit?limit=10").await;
    assert_eq!(s, StatusCode::OK);
    let entries = audit["entries"].as_array().unwrap();
    let updates_count = entries.iter().filter(|e| e["action"] == "settings_update").count();
    assert_eq!(updates_count, 1);

    // 2. PUT timezone to Asia/Kolkata again -> unchanged, writes nothing
    let (s, _, _) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({ "timezone": "Asia/Kolkata" })), &[]).await;
    assert_eq!(s, StatusCode::OK);

    // 3. PUT of an empty object {} adds none
    let (s, _, _) = call(&r, "PUT", "/api/settings", "localhost", Some(json!({})), &[]).await;
    assert_eq!(s, StatusCode::OK);

    // Still exactly 1 settings_update row
    let (s, audit) = get_json(&r, "/api/audit?limit=10").await;
    assert_eq!(s, StatusCode::OK);
    let entries = audit["entries"].as_array().unwrap();
    let updates_count = entries.iter().filter(|e| e["action"] == "settings_update").count();
    assert_eq!(updates_count, 1);
}


// ------------------------------------------------------------------ accounts and virtual funds (T005)

async fn api(app: &TestApp, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (s, _, b) = call(app, method, uri, "localhost", body, &[]).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

/// Audit actions, newest first.
async fn audit_actions(app: &TestApp) -> Vec<String> {
    let (_, v) = api(app, "GET", "/api/audit?limit=500", None).await;
    v["entries"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_string()).collect()
}

fn wallet<'a>(account: &'a Value, cur: &str) -> &'a Value {
    account["wallets"].as_array().unwrap().iter().find(|w| w["currency"] == cur).unwrap()
}

/// Every object that carries a balance or credits also says it is virtual, and money is always a string.
fn assert_virtual(v: &Value) {
    match v {
        Value::Object(m) => {
            if m.contains_key("balance") || m.contains_key("credits") {
                assert_eq!(m.get("virtual"), Some(&Value::Bool(true)), "money object without virtual:true: {v}");
                for k in ["balance", "credits", "in_positions", "available"] {
                    if let Some(x) = m.get(k) {
                        assert!(x.is_string(), "{k} must be a decimal string: {v}");
                    }
                }
            }
            m.values().for_each(assert_virtual);
        }
        Value::Array(a) => a.iter().for_each(assert_virtual),
        _ => {}
    }
}

fn buy(symbol: &str, qty: f64, price: f64, side: &str) -> Value {
    order(json!({"symbol": symbol, "qty": qty, "price": price, "side": side}))
}

#[tokio::test]
async fn fresh_database_has_demo_1_with_virtual_credits() {
    let app = router();
    let (s, v) = api(&app, "GET", "/api/accounts", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_virtual(&v);
    assert_eq!(v["notice"], "Virtual practice credits. This app holds no money.");
    let accounts = v["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 1);
    let a = &accounts[0];
    assert_eq!((a["id"].as_str(), a["kind"].as_str(), a["label"].as_str(), a["environment"].as_str()), (Some("demo-1"), Some("demo"), Some("Demo"), Some("paper")));
    assert_eq!(a["virtual"], true);
    let (usdt, inr) = (wallet(a, "USDT"), wallet(a, "INR"));
    assert_eq!((usdt["credits"].as_str(), usdt["balance"].as_str(), usdt["available"].as_str()), (Some("10000"), Some("10000"), Some("10000")));
    assert_eq!((inr["credits"].as_str(), inr["available"].as_str()), (Some("1000000"), Some("1000000")));
    assert_eq!(audit_actions(&app).await.iter().filter(|a| *a == "account_created").count(), 1);
}

#[tokio::test]
async fn orders_are_checked_against_virtual_funds_and_closing_releases_them() {
    let app = router();
    // F1: 780 USDT on 10000 is fine and is reserved
    let (s, _) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 0.01, 78000.0, "long"))).await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, v) = api(&app, "GET", "/api/accounts", None).await;
    let usdt = wallet(&v["accounts"][0], "USDT").clone();
    assert_eq!((usdt["in_positions"].as_str(), usdt["available"].as_str(), usdt["balance"].as_str()), (Some("780"), Some("9220"), Some("10000")));
    assert_eq!(v["accounts"][0]["open_positions"], 1);

    // F2: exactly the available amount is accepted, one smallest unit more is rejected and inserts nothing
    let (s, _) = api(&app, "POST", "/api/positions", Some(buy("ETHUSDT", 9220.0, 1.0, "long"))).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, e) = api(&app, "POST", "/api/positions", Some(buy("ETHUSDT", 0.00000001, 1.0, "long"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().starts_with("insufficient virtual funds: this order needs 0.00000001 USDT but only 0 USDT of virtual credit"), "{e}");
    let (_, p) = api(&app, "GET", "/api/positions?market=crypto&mode=demo", None).await;
    assert_eq!(p["positions"].as_array().unwrap().len(), 2);

    // INR is a separate wallet: an NSE order is unaffected by a full USDT wallet
    let (s, _) = api(&app, "POST", "/api/positions", Some(order(json!({"market":"india","symbol":"SBIN","qty":10,"price":500})))).await;
    assert_eq!(s, StatusCode::CREATED);

    // F5: close the BTC position at a profit: P&L less fees lands in the wallet exactly
    let id = p["positions"].as_array().unwrap().iter().find(|x| x["symbol"] == "BTCUSDT").unwrap()["id"].as_i64().unwrap();
    let (s, c) = api(&app, "POST", &format!("/api/positions/{id}/close"), Some(json!({"price": 79000}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["trade"]["account_id"], "demo-1");
    let fees = 0.1 / 100.0 * 0.01 * (78000.0 + 79000.0); // 1.57, the unchanged fee formula
    let (_, v) = api(&app, "GET", "/api/accounts", None).await;
    let usdt = wallet(&v["accounts"][0], "USDT");
    let balance: f64 = usdt["balance"].as_str().unwrap().parse().unwrap();
    assert!((balance - (10000.0 + 10.0 - fees)).abs() < 1e-9, "{usdt}");
    assert!((c["trade"]["pnl"].as_f64().unwrap() - (10.0 - fees)).abs() < 0.005);
    assert_eq!(usdt["in_positions"], "9220"); // the 780 was released, the ETH position is still reserved
}

#[tokio::test]
async fn a_short_reserves_its_full_notional() {
    let app = router();
    assert_eq!(api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1.0, 6000.0, "short"))).await.0, StatusCode::CREATED);
    let (s, e) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1.0, 5000.0, "long"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("virtual"), "{e}");
}

#[tokio::test]
async fn a_losing_short_can_take_the_balance_negative_and_blocks_new_orders() {
    let app = router();
    let (_, p) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1.0, 100.0, "short"))).await;
    let id = p["position"]["id"].as_i64().unwrap();
    let (s, _) = api(&app, "POST", &format!("/api/positions/{id}/close"), Some(json!({"price": 20000}))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = api(&app, "GET", "/api/accounts", None).await;
    let usdt = wallet(&v["accounts"][0], "USDT");
    assert!(usdt["balance"].as_str().unwrap().starts_with('-'), "a loss is shown, not hidden: {usdt}");
    assert!(usdt["available"].as_str().unwrap().starts_with('-'));
    assert_eq!(api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 0.0001, 100.0, "long"))).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_crypto_quote_and_overflow_are_clean_400s_and_the_server_survives() {
    let app = router();
    let (s, e) = api(&app, "POST", "/api/positions", Some(buy("ETHBTC", 1.0, 1.0, "long"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().starts_with("no virtual wallet for ETHBTC"), "{e}");
    // an order above the 1e15 limit is refused for that reason, not merely as "insufficient funds"
    let (s, e) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1e10, 1e6, "long"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("above the 1000000000000000 limit"), "{e}");
    // F9: 1e20 * 1e20 overflows Decimal; must be a 400, not a panic that poisons the journal mutex
    let (s, _) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1e20, 1e20, "long"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1e300, 1.0, "long"))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 0.01, 78000.0, "long"))).await;
    assert_eq!(s, StatusCode::CREATED, "the next request must still succeed");
    // a silly close price is a 400 too and leaves the position open
    let (_, p) = api(&app, "GET", "/api/positions?market=crypto&mode=demo", None).await;
    let id = p["positions"][0]["id"].as_i64().unwrap();
    assert_eq!(api(&app, "POST", &format!("/api/positions/{id}/close"), Some(json!({"price": 1e300}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(api(&app, "GET", "/api/positions?market=crypto&mode=demo", None).await.1["positions"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn orders_name_their_account_and_live_is_still_refused_first() {
    let app = router();
    let (s, e) = api(&app, "POST", "/api/positions", Some(order(json!({"account_id": "nope"})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"], "invalid order: unknown or archived demo account");
    assert_eq!(api(&app, "POST", "/api/positions", Some(order(json!({"account_id": 7})))).await.0, StatusCode::BAD_REQUEST);
    // A8: a live order is refused with the existing message before any account lookup
    let (s, e) = api(&app, "POST", "/api/positions", Some(order(json!({"mode": "live", "account_id": "nope"})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("live orders aren't supported"), "{e}");

    // reads default to demo-1 and can name an account
    let (_, second) = api(&app, "POST", "/api/accounts", Some(json!({"label": "Second"}))).await;
    assert_eq!(second["id"], "demo-2");
    assert_eq!(api(&app, "POST", "/api/positions", Some(order(json!({"account_id": "demo-2"})))).await.0, StatusCode::CREATED);
    let count = |v: &Value| v["positions"].as_array().unwrap().len();
    assert_eq!(count(&api(&app, "GET", "/api/positions?market=crypto&mode=demo", None).await.1), 0);
    assert_eq!(count(&api(&app, "GET", "/api/positions?market=crypto&mode=demo&account_id=demo-2", None).await.1), 1);
    // F7: demo-2 funds are independent of demo-1
    let (_, v) = api(&app, "GET", "/api/accounts", None).await;
    assert_eq!(wallet(&v["accounts"][0], "USDT")["available"], "10000");
    assert_eq!(wallet(&v["accounts"][1], "USDT")["available"], "9220");
}

#[tokio::test]
async fn create_validates_everything_and_audits_once() {
    let app = router();
    let bad = |body: Value| {
        let app = &app;
        async move { api(app, "POST", "/api/accounts", Some(body)).await }
    };
    for body in [
        json!({"kind": "binance", "label": "x"}),
        json!({"kind": "fyers", "label": "x"}),
        json!({"kind": "demo", "label": "x", "environment": "live"}),
        json!({"label": ""}),
        json!({"label": "x".repeat(61)}),
        json!({"label": "ok", "credits": {"USDT": -1}}),
        json!({"label": "ok", "credits": {"USDT": "abc"}}),
        json!({"label": "ok", "credits": {"USDT": 1e40}}),
        json!({"label": "ok", "credits": {"USDT": "0.123456789"}}),
        json!({"label": "ok", "credits": {"INR": "1.001"}}),
        json!({"label": "ok", "credits": {"BTC": 1}}),
        json!({"label": "ok", "balance": "999999"}),
        json!([1, 2]),
    ] {
        let (s, e) = bad(body.clone()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert!(e["error"].is_string());
    }
    let (_, v) = api(&app, "GET", "/api/accounts", None).await;
    assert_eq!(v["accounts"].as_array().unwrap().len(), 1, "no failed request may create anything");
    assert_eq!(audit_actions(&app).await.iter().filter(|a| *a == "account_created").count(), 1);

    // success: defaults, 201, exactly one audit row, credits in clear in the detail
    let (s, a) = api(&app, "POST", "/api/accounts", Some(json!({"kind": "demo", "label": "Scalping", "credits": {"USDT": "500"}}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_virtual(&a);
    assert_eq!((a["id"].as_str(), wallet(&a, "USDT")["credits"].as_str(), wallet(&a, "INR")["credits"].as_str()), (Some("demo-2"), Some("500"), Some("1000000")));
    assert_eq!(audit_actions(&app).await.iter().filter(|a| *a == "account_created").count(), 2);
    let (_, log) = api(&app, "GET", "/api/audit?limit=1", None).await;
    assert_eq!(log["entries"][0]["detail"]["credits"]["USDT"], "500");

    // the cap: 10 demo accounts in total
    for i in 3..=10 {
        assert_eq!(api(&app, "POST", "/api/accounts", Some(json!({"label": format!("a{i}")}))).await.0, StatusCode::CREATED);
    }
    assert_eq!(api(&app, "POST", "/api/accounts", Some(json!({"label": "eleventh"}))).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_changes_label_and_credits_keeps_pnl_and_audits_only_real_changes() {
    let app = router();
    // realise a P&L first, so the shift can be told apart from a reset
    let (_, p) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1.0, 100.0, "long"))).await;
    let id = p["position"]["id"].as_i64().unwrap();
    api(&app, "POST", &format!("/api/positions/{id}/close"), Some(json!({"price": 110}))).await;
    let (_, before) = api(&app, "GET", "/api/accounts", None).await;
    let bal0: f64 = wallet(&before["accounts"][0], "USDT")["balance"].as_str().unwrap().parse().unwrap();

    let n0 = audit_actions(&app).await.len();
    let (s, a) = api(&app, "PATCH", "/api/accounts/demo-1", Some(json!({"credits": {"USDT": "12000"}}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_virtual(&a);
    let usdt = wallet(&a, "USDT");
    assert_eq!(usdt["credits"], "12000");
    let bal1: f64 = usdt["balance"].as_str().unwrap().parse().unwrap();
    assert!((bal1 - (bal0 + 2000.0)).abs() < 1e-9, "balance must shift by the credit delta: {bal0} -> {bal1}");
    assert_eq!(audit_actions(&app).await.len(), n0 + 1);
    let (_, log) = api(&app, "GET", "/api/audit?limit=1", None).await;
    let e = &log["entries"][0];
    assert_eq!((e["action"].as_str(), e["entity"].as_str()), (Some("account_updated"), Some("account:demo-1")));
    assert_eq!((e["detail"]["old"]["credits"]["USDT"].as_str(), e["detail"]["new"]["credits"]["USDT"].as_str()), (Some("10000"), Some("12000")));
    assert!(e["detail"]["new"]["credits"].get("INR").is_none(), "only changed fields are logged");

    // a label alone leaves wallets alone; a no-op writes nothing
    let (_, a) = api(&app, "PATCH", "/api/accounts/demo-1", Some(json!({"label": "Practice"}))).await;
    assert_eq!((a["label"].as_str(), wallet(&a, "USDT")["credits"].as_str()), (Some("Practice"), Some("12000")));
    let n1 = audit_actions(&app).await.len();
    assert_eq!(api(&app, "PATCH", "/api/accounts/demo-1", Some(json!({"label": "Practice", "credits": {"USDT": "12000"}}))).await.0, StatusCode::OK);
    assert_eq!(audit_actions(&app).await.len(), n1, "a no-op PATCH must not audit");

    // all or nothing: a bad INR value must leave USDT unchanged and write nothing
    let (s, _) = api(&app, "PATCH", "/api/accounts/demo-1", Some(json!({"credits": {"USDT": "5", "INR": "abc"}}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, a) = api(&app, "GET", "/api/accounts", None).await;
    assert_eq!(wallet(&a["accounts"][0], "USDT")["credits"], "12000");
    assert_eq!(audit_actions(&app).await.len(), n1);
    // the audit detail keeps `credits` and `balance` readable (they are not secret-shaped fields)
    assert_eq!(log["entries"][0]["detail"]["new"]["credits"]["USDT"], "12000");
}

#[tokio::test]
async fn patch_rejects_everything_but_label_and_credits() {
    let app = router();
    for body in [json!({"balance": "1"}), json!({"virtual": false}), json!({"kind": "fyers"}), json!({"id": "x"}), json!({"status": "closed"}), json!({"archived": true}), json!({"label": "ok", "balance": "9"}), json!({"label": 5}), json!({"credits": "lots"})] {
        let (s, e) = api(&app, "PATCH", "/api/accounts/demo-1", Some(body.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert!(e["error"].is_string());
    }
    assert_eq!(api(&app, "PATCH", "/api/accounts/demo-1", Some(json!([1]))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(api(&app, "PATCH", "/api/accounts/nope", Some(json!({"label": "x"}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(api(&app, "POST", "/api/accounts/nope/reset", None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(audit_actions(&app).await.iter().filter(|a| *a == "account_updated").count(), 0);
}

#[tokio::test]
async fn reset_restores_credits_removes_positions_keeps_trades_and_other_accounts() {
    let app = router();
    api(&app, "POST", "/api/accounts", Some(json!({"label": "Second"}))).await;
    // demo-1: one closed trade (P&L), one open position
    let (_, p) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1.0, 100.0, "long"))).await;
    api(&app, "POST", &format!("/api/positions/{}/close", p["position"]["id"]), Some(json!({"price": 150}))).await;
    api(&app, "POST", "/api/positions", Some(buy("ETHUSDT", 2.0, 100.0, "long"))).await;
    // demo-2 has its own open position
    api(&app, "POST", "/api/positions", Some(order(json!({"account_id": "demo-2", "qty": 0.01})))).await;

    let (s, a) = api(&app, "POST", "/api/accounts/demo-1/reset", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_virtual(&a);
    let usdt = wallet(&a, "USDT");
    assert_eq!((usdt["balance"].as_str(), usdt["in_positions"].as_str(), a["open_positions"].as_i64()), (Some("10000"), Some("0"), Some(0)));
    let (_, j) = api(&app, "GET", "/api/journal?market=crypto&mode=demo", None).await;
    assert_eq!(j["trades"].as_array().unwrap().len(), 1, "reset keeps trade history");
    let (_, all) = api(&app, "GET", "/api/accounts", None).await;
    assert_eq!(all["accounts"][1]["open_positions"], 1, "another account is untouched");

    let (_, log) = api(&app, "GET", "/api/audit?limit=1", None).await;
    let e = &log["entries"][0];
    assert_eq!(e["action"], "account_reset");
    assert_eq!(e["detail"]["positions_removed"], 1);
    assert_eq!(e["detail"]["positions"][0]["symbol"], "ETHUSDT");
    assert_eq!(e["detail"]["balance_after"]["USDT"], "10000");
    assert_ne!(e["detail"]["balance_before"]["USDT"], "10000");
}

#[tokio::test]
async fn deleting_a_paper_trade_does_not_conjure_funds() {
    let app = router();
    let (_, p) = api(&app, "POST", "/api/positions", Some(buy("BTCUSDT", 1.0, 100.0, "long"))).await;
    let (_, c) = api(&app, "POST", &format!("/api/positions/{}/close", p["position"]["id"]), Some(json!({"price": 50}))).await;
    let (_, before) = api(&app, "GET", "/api/accounts", None).await;
    api(&app, "DELETE", &format!("/api/journal/{}", c["trade"]["id"]), None).await;
    let (_, after) = api(&app, "GET", "/api/accounts", None).await;
    assert_eq!(before["accounts"][0]["wallets"], after["accounts"][0]["wallets"]);
}

#[tokio::test]
async fn account_responses_and_audit_rows_never_carry_secret_shaped_data() {
    let app = router();
    api(&app, "POST", "/api/accounts", Some(json!({"label": "Second"}))).await;
    api(&app, "PATCH", "/api/accounts/demo-1", Some(json!({"credits": {"INR": "5"}}))).await;
    api(&app, "POST", "/api/accounts/demo-2/reset", None).await;
    let mut text = api(&app, "GET", "/api/accounts", None).await.1.to_string();
    text += &api(&app, "GET", "/api/audit?limit=500", None).await.1.to_string();
    assert!(!text.contains(&app.token), "the API token must not appear in accounts or audit output");
    assert!(!text.contains("[REDACTED]"), "no demo field is secret-shaped, so nothing should be masked: {text}");
    for word in ["secret", "api_key", "apikey", "password"] {
        assert!(!text.to_lowercase().contains(word), "{word}");
    }
}

async fn send(app: &TestApp, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (s, _, b) = call(app, method, uri, "localhost", body, &[]).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

/// The Bot Studio flow over HTTP, on sample data: library, create, backtest (saved), optimise, the live
/// refusals, limits that bind manual orders, the kill switch, edit / duplicate / delete.
#[tokio::test]
async fn bot_studio_flow() {
    let r = router();
    let (s, lib) = get_json(&r, "/api/strategies/library").await;
    assert_eq!(s, StatusCode::OK);
    assert!(lib["indicators"].as_array().unwrap().len() >= 19);
    let spec = lib["templates"].as_array().unwrap().iter().find(|t| t["id"] == "ema_cross").unwrap()["spec"].clone();

    let (s, bot) = send(&r, "POST", "/api/bots", Some(json!({ "name": "EMA", "market": "crypto", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "1000", "source": "freqtrade" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{bot}");
    assert_eq!(bot["mode"], "demo");
    assert_eq!(bot["virtual"], true);
    let id = bot["id"].as_i64().unwrap();
    let (s, e) = send(&r, "POST", "/api/bots", Some(json!({ "name": "L", "symbol": "BTCUSDT", "spec": spec, "allocation": "1", "mode": "live" }))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");

    // backtest on sample data, saved against the bot
    let (s, bt) = send(&r, "POST", "/api/backtest", Some(json!({ "bot_id": id, "bars": 500, "save": true }))).await;
    assert_eq!(s, StatusCode::OK, "{bt}");
    assert_eq!(bt["data"]["sample"], true);
    assert!(bt["data"]["source"].as_str().unwrap().contains("SAMPLE"));
    assert!(bt["metrics"]["trades"].is_u64() && bt["candles"].as_array().unwrap().len() > 100);
    let saved = bt["saved_id"].as_i64().unwrap();
    assert_eq!(bt["reproducibility"]["engine_version"], "botengine-2");
    assert_eq!(bt["reproducibility"]["dataset_hash"].as_str().unwrap().len(), 64);
    assert!(bt["regimes"].is_array() && bt["monte_carlo"].is_object());
    // same request, same answer (reproducible)
    let (_, bt2) = send(&r, "POST", "/api/backtest", Some(json!({ "bot_id": id, "bars": 500 }))).await;
    assert_eq!(bt2["metrics"], bt["metrics"]);
    assert_eq!(bt2["reproducibility"], bt["reproducibility"]);
    let (s, _, csv) = call(&r, "GET", &format!("/api/backtests/{saved}/trades.csv"), "localhost", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8_lossy(&csv).starts_with("leg,side,tag,qty"));
    // the bot is pinned to an exact strategy version; export / import round-trips it
    let vid = bot["strategy_version"]["id"].as_i64().unwrap();
    let (s, doc) = send(&r, "GET", &format!("/api/strategy-versions/{vid}/export"), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, imp) = send(&r, "POST", "/api/strategies/import", Some(doc)).await;
    assert_eq!(s, StatusCode::CREATED, "{imp}");
    assert_eq!(imp["version_id"].as_i64(), Some(vid), "same content, same version");
    let (s, wf) = send(&r, "POST", "/api/backtest/walkforward", Some(json!({ "bot_id": id, "bars": 800, "folds": 3, "objective": "return", "params": [{ "path": "/indicators/0/params/period", "values": [5, 9] }] }))).await;
    assert_eq!(s, StatusCode::OK, "{wf}");
    assert_eq!(wf["folds"].as_array().unwrap().len(), 3);
    assert_eq!(get_json(&r, &format!("/api/backtests/{saved}")).await.0, StatusCode::OK);
    assert_eq!(get_json(&r, &format!("/api/bots/{id}/backtests")).await.1["backtests"].as_array().unwrap().len(), 1);

    let (s, opt) = send(&r, "POST", "/api/backtest/optimize", Some(json!({ "bot_id": id, "bars": 600, "objective": "return", "params": [{ "path": "/indicators/0/params/period", "values": [5, 9] }] }))).await;
    assert_eq!(s, StatusCode::OK, "{opt}");
    assert_eq!(opt["results"].as_array().unwrap().len(), 2);

    // bots never trade made-up prices
    let (s, e) = send(&r, "POST", &format!("/api/bots/{id}/start"), None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(e["error"].as_str().unwrap().contains("sample"));
    let (s, d) = send(&r, "POST", &format!("/api/bots/{id}/deploy-live"), None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(d["readiness"]["can_trade_live"], false);
    let (s, a) = send(&r, "POST", "/api/live/arm", Some(json!({ "confirm": "ARM LIVE TRADING", "account_id": "demo-1" }))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{a}");

    // account limits bind manual ticket orders too
    let (s, _) = send(&r, "PUT", "/api/risk/limits/demo-1", Some(json!({ "USDT": { "max_order_notional": "100" } }))).await;
    assert_eq!(s, StatusCode::OK);
    let order = json!({"market":"crypto","mode":"demo","symbol":"BTCUSDT","side":"long","qty":0.01,"price":78000});
    let (s, e) = send(&r, "POST", "/api/positions", Some(order.clone())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("max order size"), "{e}");
    send(&r, "PUT", "/api/risk/limits/demo-1", Some(json!({ "USDT": { "max_order_notional": null } }))).await;
    assert_eq!(send(&r, "POST", "/api/positions", Some(order.clone())).await.0, StatusCode::CREATED);

    // kill switch blocks new orders until released
    let (s, k) = send(&r, "POST", "/api/risk/kill", Some(json!({ "engaged": true }))).await;
    assert_eq!((s, k["kill_switch"].clone()), (StatusCode::OK, json!(true)));
    assert!(send(&r, "POST", "/api/positions", Some(order.clone())).await.1["error"].as_str().unwrap().contains("kill switch"));
    send(&r, "POST", "/api/risk/kill", Some(json!({ "engaged": false }))).await;
    assert_eq!(send(&r, "POST", "/api/positions", Some(order)).await.0, StatusCode::CREATED);

    // edit, duplicate, logs, delete
    let (s, b2) = send(&r, "PUT", &format!("/api/bots/{id}"), Some(json!({ "name": "EMA v2" }))).await;
    assert_eq!((s, b2["name"].clone()), (StatusCode::OK, json!("EMA v2")));
    let (s, dup) = send(&r, "POST", &format!("/api/bots/{id}/duplicate"), None).await;
    assert_eq!(s, StatusCode::CREATED);
    assert!(get_json(&r, &format!("/api/bots/{id}/logs")).await.1["logs"].as_array().unwrap().len() >= 2);
    assert_eq!(send(&r, "DELETE", &format!("/api/bots/{}", dup["id"]), None).await.0, StatusCode::OK);
    assert_eq!(get_json(&r, "/api/bots").await.1["bots"].as_array().unwrap().len(), 1);
    // audit trail
    let (_, audit) = get_json(&r, "/api/audit?limit=100").await;
    let actions: Vec<&str> = audit["entries"].as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();
    for a in ["bot_created", "bot_updated", "bot_deleted", "risk_limits_updated", "kill_switch_on", "kill_switch_off"] {
        assert!(actions.contains(&a), "missing audit {a}: {actions:?}");
    }
}

#[tokio::test]
async fn scanner_runs_on_sample_data_with_source_and_formula() {
    let r = router();
    let (s, cat) = get_json(&r, "/api/scanner").await;
    assert_eq!(s, StatusCode::OK);
    assert!(cat["unsupported"].as_array().unwrap().len() >= 3);
    let (s, out) = send(&r, "POST", "/api/scanner/run", Some(json!({ "template": "movers", "market": "india", "interval": "1d", "params": { "lookback": 5, "threshold_pct": 0.1 } }))).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert!(out["source"].as_str().unwrap().contains("SAMPLE"));
    assert!(out["formula"].as_str().unwrap().contains("threshold_pct"));
    assert!(out["scanned"].as_u64().unwrap() > 50);
    assert!(!out["results"].as_array().unwrap().is_empty());
    assert_eq!(send(&r, "POST", "/api/scanner/run", Some(json!({ "template": "gap", "market": "crypto" }))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get_json(&r, "/api/scanner/history").await.1["runs"].as_array().unwrap().len(), 1);
}
