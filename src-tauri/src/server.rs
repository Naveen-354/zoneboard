//! The local HTTP server: the web UI (embedded in the binary) plus the same `/api/*` the Flask app had.
//! It only answers on localhost: there is no login, so it must never be reachable from the network.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use chrono::{FixedOffset, Utc};
use rust_embed::RustEmbed;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::data::{self, DataStore};
use crate::journal::{self, Journal};
use crate::zones::{self, Label, Summary};

#[derive(RustEmbed)]
#[folder = "../static"]
struct Assets;

/// The page loads its own scripts only, and may talk to Binance's public data hosts and nobody else.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
    connect-src 'self' https://data-api.binance.vision wss://data-stream.binance.vision; \
    object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

pub struct AppState {
    data: DataStore,
    journal: Journal,
    scan: Mutex<Option<(Instant, Value)>>,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(demo: bool, journal_path: &str) -> Result<Shared, String> {
        let journal = Journal::open(journal_path).map_err(|e| format!("can't open journal {journal_path}: {e}"))?;
        Ok(Arc::new(Self { data: DataStore::new(demo), journal, scan: Mutex::new(None) }))
    }
}

pub fn demo_from_env() -> bool {
    std::env::var("DEMO").as_deref() == Ok("1")
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

pub fn app(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/{*path}", get(static_file))
        .route("/api/config", get(config))
        .route("/api/scan", get(scan))
        .route("/api/chart/{symbol}", get(chart))
        .route("/api/journal", get(journal_get).post(journal_add))
        .route("/api/journal/{id}", delete(journal_delete))
        .layer(middleware::from_fn(guard))
        .with_state(state)
}

/// Rejects foreign Host headers (stops DNS rebinding: a web page can't reach this no-login app through a
/// hostname it controls) and adds the security headers.
async fn guard(req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    let name = if host.ends_with(']') { host.as_str() } else { host.rsplit_once(':').map_or(host.as_str(), |(h, _)| h) };
    if !matches!(name, "127.0.0.1" | "localhost" | "[::1]") {
        return err(StatusCode::FORBIDDEN, "this app only answers on localhost");
    }
    let mut resp = next.run(req).await;
    let is_html = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("text/html"));
    let h = resp.headers_mut();
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    if is_html {
        h.insert("content-security-policy", HeaderValue::from_static(CSP));
    }
    resp
}

// ------------------------------------------------------------------ static files
fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn asset(path: &str, req: &HeaderMap) -> Response {
    let Some(file) = Assets::get(path) else {
        return err(StatusCode::NOT_FOUND, "not found");
    };
    let etag = format!("\"{}\"", file.metadata.sha256_hash().iter().map(|b| format!("{b:02x}")).collect::<String>());
    let etag_value = HeaderValue::from_str(&etag).unwrap();
    if req.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag_value)]).into_response();
    }
    // vendor/ files have the version in their name, so they can be cached for a year
    let cache = if path.starts_with("vendor/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static(mime_for(path))), (header::ETAG, etag_value), (header::CACHE_CONTROL, HeaderValue::from_static(cache))],
        file.data.into_owned(),
    )
        .into_response()
}

async fn index(headers: HeaderMap) -> Response {
    asset("index.html", &headers)
}

async fn static_file(Path(path): Path<String>, headers: HeaderMap) -> Response {
    asset(&path, &headers)
}

// ------------------------------------------------------------------ api
async fn config(State(s): State<Shared>) -> Json<Value> {
    Json(json!({
        "demo": s.data.demo,
        "crypto": { "symbols": data::load_crypto_symbols() },
        "india": { "count": data::load_symbols().len() },
    }))
}

#[derive(Serialize)]
struct Row<'a> {
    symbol: &'a str,
    #[serde(flatten)]
    summary: &'a Summary,
}

async fn run_scan(s: &AppState) -> Value {
    let frames = s.data.daily_all().await;
    let mut rows: Vec<(String, Summary)> = frames.iter().filter(|(_, f)| f.len() >= 40).map(|(sym, f)| (sym.clone(), zones::summary_of(f))).collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let count = |l: Label| rows.iter().filter(|r| r.1.strength.label == l).count();
    let score = |r: &&(String, Summary)| r.1.strength.score;
    let mut bull: Vec<&(String, Summary)> = rows.iter().filter(|r| r.1.strength.label == Label::Bullish).collect();
    bull.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap());
    let mut bear: Vec<&(String, Summary)> = rows.iter().filter(|r| r.1.strength.label == Label::Bearish).collect();
    bear.sort_by(|a, b| score(a).partial_cmp(&score(b)).unwrap());
    let mut all: Vec<&(String, Summary)> = rows.iter().collect();
    all.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap());

    let full = |v: &[&(String, Summary)]| v.iter().take(20).map(|r| serde_json::to_value(Row { symbol: &r.0, summary: &r.1 }).unwrap()).collect::<Vec<_>>();
    let slim: Vec<Value> = all
        .iter()
        .map(|r| json!({ "symbol": r.0, "price": r.1.price, "change_pct": r.1.strength.change_pct, "score": r.1.strength.score, "label": r.1.strength.label }))
        .collect();
    let ist = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
    json!({
        "updated": Utc::now().with_timezone(&ist).format("%Y-%m-%d %H:%M:%S IST").to_string(),
        "demo": s.data.demo,
        "counts": {
            "total": rows.len(),
            "bullish": count(Label::Bullish),
            "bearish": count(Label::Bearish),
            "choppy": count(Label::Choppy),
            "advancing": rows.iter().filter(|r| r.1.strength.change_pct > 0.0).count(),
            "declining": rows.iter().filter(|r| r.1.strength.change_pct < 0.0).count(),
        },
        "bullish": full(&bull),
        "bearish": full(&bear),
        "all": slim,
    })
}

async fn scan(State(s): State<Shared>) -> Json<Value> {
    let mut slot = s.scan.lock().await; // one scan at a time; others wait and share the result
    if slot.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(60)) {
        let v = run_scan(&s).await;
        *slot = Some((Instant::now(), v));
    }
    Json(slot.as_ref().unwrap().1.clone())
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

async fn chart(State(s): State<Shared>, Path(symbol): Path<String>, Query(q): Query<HashMap<String, String>>) -> Response {
    let symbol = symbol.to_uppercase();
    if !data::valid_nse_symbol(&symbol) {
        return err(StatusCode::BAD_REQUEST, "invalid symbol");
    }
    let interval = q.get("interval").map_or("1d", String::as_str);
    if !data::INTERVALS.iter().any(|x| x.0 == interval) {
        let names: Vec<&str> = data::INTERVALS.iter().map(|x| x.0).collect();
        return err(StatusCode::BAD_REQUEST, &format!("interval must be one of {}", names.join(", ")));
    }
    let Some(f) = s.data.ohlcv(&symbol, interval).await.filter(|f| f.len() >= 40) else {
        return err(StatusCode::NOT_FOUND, &format!("no data for {symbol} (check the symbol or your connection)"));
    };
    let an = zones::analyze(&f);
    let (zone_out, level_out) = zones::chart_overlays(&f, &an.obs, &an.liq);
    let summary = zones::summarize(&f, &an.obs, &an.liq, &an.atr);
    let from = f.len().saturating_sub(300);
    let candles: Vec<Value> = (from..f.len())
        .map(|i| json!({ "time": f.t[i], "open": r2(f.o[i]), "high": r2(f.h[i]), "low": r2(f.l[i]), "close": r2(f.c[i]), "volume": f.v[i] }))
        .collect();
    Json(json!({ "symbol": symbol, "interval": interval, "candles": candles, "zones": zone_out, "levels": level_out, "summary": summary })).into_response()
}

async fn journal_get(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let market = q.get("market").map_or("india", String::as_str);
    if !journal::MARKETS.contains(&market) {
        return err(StatusCode::BAD_REQUEST, &format!("market must be one of {}", journal::MARKETS.join(", ")));
    }
    Json(s.journal.payload(market)).into_response()
}

async fn journal_add(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    // JSON only, like Flask's get_json(): a cross-site form post can't send this content type without a CORS preflight
    let is_json = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"));
    let parsed: Value = if is_json { serde_json::from_slice(&body).unwrap_or(Value::Null) } else { Value::Null };
    match s.journal.add(&parsed) {
        Ok(()) => (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

async fn journal_delete(State(s): State<Shared>, Path(id): Path<i64>) -> Json<Value> {
    s.journal.delete(id);
    Json(json!({ "ok": true }))
}

// ------------------------------------------------------------------ serving
/// Loopback listeners, never reachable from the network. Port from `PORT`, else 5000; if 5000 is taken
/// (and none was asked for) any free port. The first listener is 127.0.0.1; `[::1]` is added on the same
/// port when IPv6 is available, because Windows tries `localhost` -> ::1 first and every new connection
/// would otherwise wait ~200 ms for the IPv4 fallback.
pub fn bind() -> std::io::Result<Vec<TcpListener>> {
    let v4 = |port: u16| TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    let first = match std::env::var("PORT") {
        Ok(p) => v4(p.parse().map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("PORT={p} is not a port number")))?)?,
        Err(_) => v4(5000).or_else(|_| v4(0))?,
    };
    let port = first.local_addr()?.port();
    let mut out = vec![first];
    if let Ok(v6) = TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))) {
        out.push(v6);
    }
    Ok(out)
}

pub fn port_of(listeners: &[TcpListener]) -> u16 {
    listeners[0].local_addr().map(|a| a.port()).unwrap_or(0)
}

pub async fn serve(listeners: Vec<TcpListener>, state: Shared) -> std::io::Result<()> {
    let router = app(state);
    let mut tasks = tokio::task::JoinSet::new();
    for l in listeners {
        l.set_nonblocking(true)?;
        let l = tokio::net::TcpListener::from_std(l)?;
        let r = router.clone();
        tasks.spawn(async move { axum::serve(l, r).await });
    }
    while let Some(done) = tasks.join_next().await {
        done.map_err(std::io::Error::other)??;
    }
    Ok(())
}

/// Start the server on its own thread and runtime. Returns the port. For the desktop window.
pub fn spawn() -> Result<u16, String> {
    let listeners = bind().map_err(|e| format!("can't start the local server: {e}"))?;
    let port = port_of(&listeners);
    let state = AppState::new(demo_from_env(), &journal::default_path())?;
    std::thread::Builder::new()
        .name("zoneboard-server".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
            if let Err(e) = rt.block_on(serve(listeners, state)) {
                eprintln!("server stopped: {e}");
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(port)
}
