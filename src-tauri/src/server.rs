//! The local HTTP server: the web UI (embedded in the binary) plus the same `/api/*` the Flask app had.
//! It only answers on localhost: there is no login, so it must never be reachable from the network.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use chrono::{FixedOffset, Utc};
use rand::rngs::OsRng;
use rand::RngCore;
use rust_embed::RustEmbed;
use serde::Serialize;
use serde_json::{json, Value};
use subtle::ConstantTimeEq;
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

/// Generates a cryptographically secure 256-bit random API token formatted as lowercase hex (64 chars).
fn generate_api_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time string equality comparison to prevent timing side-channel attacks on token verification.
#[doc(hidden)]
pub fn constant_time_compare(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Checks if an `Origin` header matches the server's own origin (`http://{host}`).
fn is_same_origin(origin: &str, host: &str) -> bool {
    origin.eq_ignore_ascii_case(&format!("http://{host}"))
}

/// Extracts the API token from an incoming request.
/// Today it reads the `X-ZB-Token` header.
///
/// # Extension Point
/// Future Server-Sent Events (SSE) endpoints using browser `EventSource` cannot set
/// custom HTTP headers; an `auth` or `token` query parameter (e.g. `?token=...`) can
/// be accepted here specifically for designated SSE routes.
fn token_from_request(req: &Request) -> Option<&str> {
    req.headers().get("X-ZB-Token").and_then(|v| v.to_str().ok())
}

/// Replaces the placeholder `<meta name="zb-token" content="">` with the server's launch token.
#[doc(hidden)]
pub fn render_index_html(template: &str, token: &str) -> String {
    const PLACEHOLDER: &str = r#"<meta name="zb-token" content="">"#;
    let replacement = format!(r#"<meta name="zb-token" content="{token}">"#);
    template.replace(PLACEHOLDER, &replacement)
}

pub struct AppState {
    data: DataStore,
    journal: Journal,
    scan: [Mutex<Option<(Instant, Value)>>; 2], // indexed by `sample as usize`: sample and real scans never share a cache
    api_token: String,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(sample: bool, journal_path: &str) -> Result<Shared, String> {
        let journal = Journal::open(journal_path).map_err(|e| format!("can't open journal {journal_path}: {e}"))?;
        journal.with_conn(|conn| {
            crate::audit::record(
                conn,
                "system",
                "app_start",
                "system",
                &json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "sample": sample,
                }),
            )
        }).map_err(|e| format!("can't record app_start audit entry: {e}"))?;
        let api_token = generate_api_token();
        Ok(Arc::new(Self {
            data: DataStore::new(sample),
            journal,
            scan: [Mutex::new(None), Mutex::new(None)],
            api_token,
        }))
    }

    /// Read-only accessor for the in-memory API token.
    #[doc(hidden)]
    pub fn api_token(&self) -> &str {
        &self.api_token
    }

    /// `?sample=1` / `?sample=0` on a request; without it the `SAMPLE_DATA=1` environment decides.
    fn want_sample(&self, q: &HashMap<String, String>) -> bool {
        match q.get("sample").map(String::as_str) {
            Some("1" | "true") => true,
            Some(_) => false,
            None => self.data.sample,
        }
    }
}

pub fn sample_from_env() -> bool {
    std::env::var("SAMPLE_DATA").as_deref() == Ok("1")
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
        .route("/api/positions", get(positions_get).post(positions_open))
        .route("/api/positions/{id}/close", post(positions_close))
        .route("/api/audit", get(audit_get))
        .route("/api/settings", get(settings_get).put(settings_put))
        .route("/api/accounts", get(accounts_get).post(accounts_create))
        .route("/api/accounts/{id}", patch(accounts_patch))
        .route("/api/accounts/{id}/reset", post(accounts_reset))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// Rejects foreign Host headers (stops DNS rebinding: a web page can't reach this no-login app through a
/// hostname it controls) and adds the security headers.
///
/// # Threat Model
/// Protects against: other websites (Cross-Site Request Forgery / CSRF, DNS rebinding) and
/// browser extensions on other origins attempting to use this local API.
/// Does **NOT** protect against malware or malicious processes already running as this
/// Windows user, which could read the page, local database, or keychain anyway.
async fn guard(State(state): State<Shared>, req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
    let name = if host.ends_with(']') { host.as_str() } else { host.rsplit_once(':').map_or(host.as_str(), |(h, _)| h) };
    if !matches!(name, "127.0.0.1" | "localhost" | "[::1]") {
        return err(StatusCode::FORBIDDEN, "this app only answers on localhost");
    }
    // CSRF origin check on state-changing methods
    let method = req.method();
    if matches!(method, &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE) {
        if let Some(origin_val) = req.headers().get(header::ORIGIN) {
            let Ok(origin_str) = origin_val.to_str() else {
                return err(StatusCode::FORBIDDEN, "invalid origin");
            };
            if !is_same_origin(origin_str, &host) {
                return err(StatusCode::FORBIDDEN, "cross-origin requests are forbidden");
            }
        }
    }

    // Route protection by path prefix: all /api and /api/* endpoints require the launch token
    let path = req.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        let provided = token_from_request(&req);
        let valid = match provided {
            Some(tok) => constant_time_compare(tok, state.api_token()),
            None => false,
        };
        if !valid {
            return err(StatusCode::UNAUTHORIZED, "missing or invalid API token");
        }
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

async fn index(State(s): State<Shared>) -> Response {
    let Some(file) = Assets::get("index.html") else {
        return err(StatusCode::NOT_FOUND, "not found");
    };
    let Ok(html) = std::str::from_utf8(&file.data) else {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "invalid html");
    };
    let rendered = render_index_html(html, s.api_token());
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        rendered,
    )
        .into_response()
}

async fn static_file(Path(path): Path<String>, headers: HeaderMap) -> Response {
    asset(&path, &headers)
}

// ------------------------------------------------------------------ api
async fn config(State(s): State<Shared>) -> Json<Value> {
    Json(json!({
        "sample": s.data.sample,
        "crypto": { "symbols": data::load_crypto_symbols() },
        "india": { "count": data::load_symbols().len() },
        "fees": { "crypto": journal::fee_pct("crypto"), "india": journal::fee_pct("india") },
    }))
}

#[derive(Serialize)]
struct Row<'a> {
    symbol: &'a str,
    #[serde(flatten)]
    summary: &'a Summary,
}

async fn run_scan(s: &AppState, sample: bool) -> Value {
    let frames = s.data.daily_all(sample).await;
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
        "sample": sample,
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

async fn scan(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let sample = s.want_sample(&q);
    let mut slot = s.scan[sample as usize].lock().await; // one scan at a time per mode; others wait and share the result
    if slot.as_ref().is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(60)) {
        let v = run_scan(&s, sample).await;
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
    let Some(f) = s.data.ohlcv(&symbol, interval, s.want_sample(&q)).await.filter(|f| f.len() >= 40) else {
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

/// `?market=` (default india) and `?mode=` (default live), validated.
fn market_mode(q: &HashMap<String, String>) -> Result<(&str, &str), Response> {
    let market = q.get("market").map_or("india", String::as_str);
    let mode = q.get("mode").map_or("live", String::as_str);
    journal::check_market_mode(market, mode).map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
    Ok((market, mode))
}

/// A JSON request body, or Null. JSON only, like Flask's get_json(): a cross-site form post can't send
/// this content type without a CORS preflight.
fn json_body(headers: &HeaderMap, body: &[u8]) -> Value {
    let is_json = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        serde_json::from_slice(body).unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

async fn journal_get(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    match market_mode(&q) {
        Ok((market, mode)) => Json(s.journal.payload_for(market, mode, q.get("account_id").map(String::as_str))).into_response(),
        Err(r) => r,
    }
}

async fn journal_add(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    match s.journal.add(&json_body(&headers, &body)) {
        Ok(()) => (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

async fn journal_delete(State(s): State<Shared>, Path(id): Path<i64>) -> Json<Value> {
    s.journal.delete(id);
    Json(json!({ "ok": true }))
}

// Paper trading: simulated orders at the price the page sends. See journal.rs.
async fn positions_get(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    match market_mode(&q) {
        Ok((market, mode)) => Json(json!({ "positions": s.journal.positions_for(market, mode, q.get("account_id").map(String::as_str)) })).into_response(),
        Err(r) => r,
    }
}

async fn positions_open(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    match s.journal.open_position(&json_body(&headers, &body)) {
        Ok(p) => (StatusCode::CREATED, Json(json!({ "ok": true, "position": p }))).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, &e),
    }
}

async fn positions_close(State(s): State<Shared>, Path(id): Path<i64>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    match s.journal.close_position(id, &json_body(&headers, &body)) {
        Ok(t) => Json(json!({ "ok": true, "trade": t })).into_response(),
        Err(journal::CloseError::NotFound) => err(StatusCode::NOT_FOUND, "no such open position"),
        Err(journal::CloseError::Invalid(m)) => err(StatusCode::BAD_REQUEST, &m),
    }
}

fn parse_audit_query(q: &HashMap<String, String>) -> Result<(usize, Option<i64>), String> {
    let limit = match q.get("limit") {
        Some(s) => match s.parse::<usize>() {
            Ok(0) | Err(_) => return Err("limit must be a positive integer".to_string()),
            Ok(n) => n.min(500),
        },
        None => 50,
    };

    let before = match q.get("before") {
        Some(s) => match s.parse::<i64>() {
            Ok(id) if id > 0 => Some(id),
            _ => return Err("before must be a positive integer".to_string()),
        },
        None => None,
    };

    Ok((limit, before))
}

async fn audit_get(State(s): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let (limit, before) = match parse_audit_query(&q) {
        Ok(vals) => vals,
        Err(e) => return err(StatusCode::BAD_REQUEST, &e),
    };

    let result = s.journal.with_conn(|conn| crate::audit::list(conn, limit, before));
    match result {
        Ok((entries, next_before)) => {
            Json(json!({ "entries": entries, "next_before": next_before })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("audit query failed: {e}")),
    }
}

async fn settings_get(State(s): State<Shared>) -> Response {
    match s.journal.with_conn(|conn| crate::settings::get_settings(conn)) {
        Ok(settings) => Json(settings).into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("failed to get settings: {e}")),
    }
}

async fn settings_put(
    State(s): State<Shared>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let body_json = json_body(&headers, &body);
    let body_obj = match body_json {
        Value::Object(map) => map,
        _ => return err(StatusCode::BAD_REQUEST, "request body must be a JSON object"),
    };

    let result = s.journal.with_conn(|conn| crate::settings::update_settings(conn, &body_obj));
    match result {
        Ok(updated) => Json(updated).into_response(),
        Err(crate::settings::SettingsError::RuntimeOnlyKey(msg)) => err(StatusCode::BAD_REQUEST, &msg),
        Err(crate::settings::SettingsError::Validation(msg)) => err(StatusCode::BAD_REQUEST, &msg),
        Err(crate::settings::SettingsError::Database(e)) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("database error: {e}")),
    }
}

// Accounts and their virtual funds (T005). Every balance in these responses carries "virtual": true.
fn account_response(r: Result<Value, crate::accounts::AccountError>, ok: StatusCode) -> Response {
    use crate::accounts::AccountError;
    match r {
        Ok(v) => (ok, Json(v)).into_response(),
        Err(AccountError::NotFound) => err(StatusCode::NOT_FOUND, "no such account"),
        Err(AccountError::Invalid(m)) => err(StatusCode::BAD_REQUEST, &m),
        Err(AccountError::Db(m)) => err(StatusCode::INTERNAL_SERVER_ERROR, &format!("database error: {m}")),
    }
}

async fn accounts_get(State(s): State<Shared>) -> Response {
    account_response(s.journal.with_conn(|c| crate::accounts::list(c)), StatusCode::OK)
}

async fn accounts_create(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    account_response(s.journal.with_conn(|c| crate::accounts::create(c, &b)), StatusCode::CREATED)
}

async fn accounts_patch(State(s): State<Shared>, Path(id): Path<String>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    let b = json_body(&headers, &body);
    account_response(s.journal.with_conn(|c| crate::accounts::update(c, &id, &b)), StatusCode::OK)
}

async fn accounts_reset(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    account_response(s.journal.with_conn(|c| crate::accounts::reset(c, &id)), StatusCode::OK)
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
    let journal_path = journal::default_path();
    tracing::info!(port, journal_path = %journal_path, version = env!("CARGO_PKG_VERSION"), "ZoneBoard server started");
    let state = AppState::new(sample_from_env(), &journal_path)?;
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_audit_query_limits_and_before() {
        let mut q = HashMap::new();

        // no limit -> 50, no before -> None
        assert_eq!(parse_audit_query(&q), Ok((50, None)));

        // 500 -> 500
        q.insert("limit".to_string(), "500".to_string());
        assert_eq!(parse_audit_query(&q), Ok((500, None)));

        // 501 -> 500
        q.insert("limit".to_string(), "501".to_string());
        assert_eq!(parse_audit_query(&q), Ok((500, None)));

        // 100000 -> 500
        q.insert("limit".to_string(), "100000".to_string());
        assert_eq!(parse_audit_query(&q), Ok((500, None)));

        // 0, -1, abc -> error
        q.insert("limit".to_string(), "0".to_string());
        assert!(parse_audit_query(&q).is_err());

        q.insert("limit".to_string(), "-1".to_string());
        assert!(parse_audit_query(&q).is_err());

        q.insert("limit".to_string(), "abc".to_string());
        assert!(parse_audit_query(&q).is_err());

        // reset valid limit
        q.insert("limit".to_string(), "20".to_string());

        // before 0, -5, abc -> error
        q.insert("before".to_string(), "0".to_string());
        assert!(parse_audit_query(&q).is_err());

        q.insert("before".to_string(), "-5".to_string());
        assert!(parse_audit_query(&q).is_err());

        q.insert("before".to_string(), "abc".to_string());
        assert!(parse_audit_query(&q).is_err());

        // valid before passes through
        q.insert("before".to_string(), "42".to_string());
        assert_eq!(parse_audit_query(&q), Ok((20, Some(42))));
    }
}
