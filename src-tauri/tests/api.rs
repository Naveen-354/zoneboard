//! HTTP-level tests of the local server: the same contract the Flask app had (see test_app.py).
//! Runs against the in-process router with synthetic data, so no network is needed.

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;
use zoneboard::server::{app, AppState};

fn router() -> Router {
    app(AppState::new(true, ":memory:").unwrap())
}

async fn call(r: &Router, method: &str, uri: &str, host: &str, json_body: Option<Value>, extra: &[(&str, &str)]) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri).header(header::HOST, host);
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    let req = match json_body {
        Some(v) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = r.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    (parts.status, parts.headers, to_bytes(body, 10_000_000).await.unwrap().to_vec())
}

async fn get_json(r: &Router, uri: &str) -> (StatusCode, Value) {
    let (s, _, b) = call(r, "GET", uri, "localhost", None, &[]).await;
    (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
}

#[tokio::test]
async fn config_lists_crypto_symbols() {
    let (s, c) = get_json(&router(), "/api/config").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["demo"], true);
    let syms = c["crypto"]["symbols"].as_array().unwrap();
    assert!(syms.iter().any(|x| x == "BTCUSDT") && syms.len() >= 20);
    assert!(c["india"]["count"].as_u64().unwrap() > 10);
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
    assert_eq!(scan["demo"], true);
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
    let req = Request::builder().method("POST").uri("/api/journal").header(header::HOST, "localhost").header(header::CONTENT_TYPE, "text/plain").body(Body::from(post(json!({"market":"crypto","symbol":"X","qty":1})).to_string())).unwrap();
    assert_eq!(r.clone().oneshot(req).await.unwrap().status(), StatusCode::BAD_REQUEST);

    let id = crypto["trades"][0]["id"].as_i64().unwrap();
    assert_eq!(call(&r, "DELETE", &format!("/api/journal/{id}"), "localhost", None, &[]).await.0, StatusCode::OK);
    assert_eq!(get_json(&r, "/api/journal?market=crypto").await.1["trades"].as_array().unwrap().len(), 0);
}
