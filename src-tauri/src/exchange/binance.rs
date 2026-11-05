//! Binance Spot REST client (signed endpoints, HMAC-SHA256 keys).
//!
//! Checked against Binance's official Spot API docs (developers.binance.com), recorded in
//! `docs/platform/FINDINGS-binance.md`: signature = hex HMAC-SHA256 of the full query string with the
//! secret key (the documented example vector is a unit test here), `timestamp` + `recvWindow`, the
//! `X-MBX-APIKEY` header, `X-MBX-USED-WEIGHT-1M` usage header, 429 = rate limit, 418 = IP ban, both with
//! `Retry-After`. Live base `https://api.binance.com`, Spot Testnet `https://testnet.binance.vision` (no
//! `/sapi` there). Self-generated Ed25519 keys are not supported yet: HMAC (system-generated) keys only.
//!
//! Safety:
//!  * The secret never leaves this struct except into the HMAC; the API key goes only in the header.
//!    Errors never contain either, nor the signed URL.
//!  * 429 / 418 set a hard pause; every call during it fails fast. Nothing here retries in a loop.
//!  * An order call that times out has an UNKNOWN outcome ([`ExchangeError::Network`]): callers must look the
//!    order up by its client order id before deciding anything ([`BinanceClient::get_order`]).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use reqwest::{Client, Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use sha2::Sha256;

use super::{Balance, Capabilities, ExchangeError};

pub const LIVE_URL: &str = "https://api.binance.com";
pub const TESTNET_URL: &str = "https://testnet.binance.vision";
const RECV_WINDOW: u64 = 5000;

/// Hex HMAC-SHA256 of `query` with `secret`, as Binance's SIGNED endpoints require.
pub fn sign(secret: &str, query: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(query.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

pub struct BinanceClient {
    http: Client,
    base: String,
    environment: String,
    api_key: SecretString,
    secret: SecretString,
    /// server time minus local time, ms
    offset_ms: AtomicI64,
    used_weight: AtomicU64,
    paused_until: Mutex<Option<Instant>>,
}

/// What `GET /api/v3/account` says, reduced to what we use.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub can_trade: bool,
    pub account_type: String,
    pub permissions: Vec<String>,
    pub balances: Vec<Balance>,
}

/// One order as Binance reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderReport {
    pub order_id: i64,
    pub client_order_id: String,
    pub status: String,
    pub executed_qty: String,
    pub quote_qty: String,
    /// Volume-weighted fill price, from the quote and base quantities (the broker's own numbers).
    pub avg_price: Option<String>,
    pub raw: Value,
}

/// Lot and notional rules for one symbol (from `exchangeInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct SymbolRules {
    pub status: String,
    pub step_size: String,
    pub min_qty: String,
    pub min_notional: Option<String>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl BinanceClient {
    pub fn new(http: Client, base: &str, environment: &str, api_key: SecretString, secret: SecretString) -> Self {
        Self { http, base: base.trim_end_matches('/').to_string(), environment: environment.to_string(), api_key, secret, offset_ms: AtomicI64::new(0), used_weight: AtomicU64::new(0), paused_until: Mutex::new(None) }
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            provider: "binance",
            environment: self.environment.clone(),
            markets: vec!["crypto spot"],
            read_balances: true,
            place_orders: true,
            order_types: vec!["MARKET"],
            market_data: "Binance public market data (candles); account data through signed REST",
            notes: vec![
                "Spot only (decision D1): no margin or futures.",
                "Orders go only through ZoneBoard's risk engine, and to a live account only when it is armed and the bot is approved.",
                "HMAC keys only; Ed25519 keys are not supported yet.",
            ],
        }
    }

    pub fn used_weight(&self) -> u64 {
        self.used_weight.load(Ordering::Relaxed)
    }

    fn check_pause(&self) -> Result<(), ExchangeError> {
        let p = self.paused_until.lock().unwrap();
        if let Some(t) = *p {
            let now = Instant::now();
            if t > now {
                return Err(ExchangeError::RateLimited { retry_after_secs: (t - now).as_secs().max(1), banned: false });
            }
        }
        Ok(())
    }

    /// `GET /api/v3/time`, and remember the clock offset used for signed requests.
    pub async fn sync_time(&self) -> Result<i64, ExchangeError> {
        let v = self.send(Method::GET, "/api/v3/time", None, false).await?;
        let server = v.get("serverTime").and_then(Value::as_i64).ok_or_else(|| ExchangeError::Network("no serverTime in the reply".into()))?;
        let offset = server - now_ms();
        self.offset_ms.store(offset, Ordering::Relaxed);
        Ok(offset)
    }

    async fn send(&self, method: Method, path: &str, params: Option<Vec<(String, String)>>, signed: bool) -> Result<Value, ExchangeError> {
        self.check_pause()?;
        let mut query: Vec<(String, String)> = params.unwrap_or_default();
        if signed {
            query.push(("recvWindow".into(), RECV_WINDOW.to_string()));
            query.push(("timestamp".into(), (now_ms() + self.offset_ms.load(Ordering::Relaxed)).to_string()));
        }
        let mut qs = query.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect::<Vec<_>>().join("&");
        if signed {
            let sig = sign(self.secret.expose_secret(), &qs);
            qs = if qs.is_empty() { format!("signature={sig}") } else { format!("{qs}&signature={sig}") };
        }
        let url = if qs.is_empty() { format!("{}{path}", self.base) } else { format!("{}{path}?{qs}", self.base) };
        let mut req = self.http.request(method, &url).timeout(Duration::from_secs(10));
        if signed {
            req = req.header("X-MBX-APIKEY", self.api_key.expose_secret());
        }
        // never put the URL (it carries the signature) or a key into an error
        let resp = req.send().await.map_err(|e| ExchangeError::Network(if e.is_timeout() { "timed out".into() } else if e.is_connect() { "could not connect".into() } else { "request failed".into() }))?;
        if let Some(w) = resp.headers().get("x-mbx-used-weight-1m").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()) {
            self.used_weight.store(w, Ordering::Relaxed);
        }
        let status = resp.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.as_u16() == 418 {
            let secs = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(60);
            *self.paused_until.lock().unwrap() = Some(Instant::now() + Duration::from_secs(secs));
            tracing::warn!(status = status.as_u16(), retry_after = secs, "binance rate limit: paused");
            return Err(ExchangeError::RateLimited { retry_after_secs: secs, banned: status.as_u16() == 418 });
        }
        let body: Value = resp.json().await.map_err(|_| ExchangeError::Network("unreadable reply".into()))?;
        if !status.is_success() {
            let code = body.get("code").and_then(Value::as_i64).unwrap_or(0);
            let msg: String = body.get("msg").and_then(Value::as_str).unwrap_or("").chars().take(200).collect();
            // -2014 / -2015 bad key, -1022 bad signature, 401 / 403: the credentials, not the request
            if matches!(code, -2014 | -2015 | -1022) || status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Err(ExchangeError::Auth(msg));
            }
            return Err(ExchangeError::Api { status: status.as_u16(), code, msg });
        }
        Ok(body)
    }

    /// A signed read. A timestamp rejection (-1021) re-syncs the clock and is tried once more: reads are safe
    /// to repeat. Orders never use this.
    async fn signed_read(&self, path: &str, params: Vec<(String, String)>) -> Result<Value, ExchangeError> {
        match self.send(Method::GET, path, Some(params.clone()), true).await {
            Err(ExchangeError::Api { code: -1021, .. }) => {
                self.sync_time().await?;
                self.send(Method::GET, path, Some(params), true).await
            }
            r => r,
        }
    }

    pub async fn account(&self) -> Result<Account, ExchangeError> {
        let v = self.signed_read("/api/v3/account", vec![("omitZeroBalances".into(), "true".into())]).await?;
        let s = |x: &Value| x.as_str().unwrap_or("0").to_string();
        let balances = v
            .get("balances")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|b| Balance { asset: b.get("asset").map(s).unwrap_or_default(), free: b.get("free").map(s).unwrap_or_default(), locked: b.get("locked").map(s).unwrap_or_default() }).collect())
            .unwrap_or_default();
        Ok(Account {
            can_trade: v.get("canTrade").and_then(Value::as_bool).unwrap_or(false),
            account_type: v.get("accountType").and_then(Value::as_str).unwrap_or("").to_string(),
            permissions: v.get("permissions").and_then(Value::as_array).map(|a| a.iter().filter_map(|p| p.as_str().map(String::from)).collect()).unwrap_or_default(),
            balances,
        })
    }

    /// `GET /sapi/v1/account/apiRestrictions`: does this key allow withdrawals? Live only (the testnet has no
    /// `/sapi`, and no withdrawals at all).
    pub async fn withdrawals_enabled(&self) -> Result<bool, ExchangeError> {
        let v = self.signed_read("/sapi/v1/account/apiRestrictions", vec![]).await?;
        v.get("enableWithdrawals").and_then(Value::as_bool).ok_or_else(|| ExchangeError::Network("apiRestrictions reply without enableWithdrawals".into()))
    }

    pub async fn symbol_rules(&self, symbol: &str) -> Result<SymbolRules, ExchangeError> {
        let v = self.send(Method::GET, "/api/v3/exchangeInfo", Some(vec![("symbol".into(), symbol.into())]), false).await?;
        let sym = v.get("symbols").and_then(Value::as_array).and_then(|a| a.first()).ok_or_else(|| ExchangeError::Refused(format!("{symbol} is not listed on this Binance environment")))?;
        let filters = sym.get("filters").and_then(Value::as_array).cloned().unwrap_or_default();
        let f = |t: &str, k: &str| filters.iter().find(|x| x["filterType"] == t).and_then(|x| x.get(k)).and_then(Value::as_str).map(String::from);
        Ok(SymbolRules {
            status: sym.get("status").and_then(Value::as_str).unwrap_or("").to_string(),
            step_size: f("LOT_SIZE", "stepSize").unwrap_or_else(|| "0.00000001".into()),
            min_qty: f("LOT_SIZE", "minQty").unwrap_or_else(|| "0".into()),
            min_notional: f("NOTIONAL", "minNotional").or_else(|| f("MIN_NOTIONAL", "minNotional")),
        })
    }

    fn report(v: Value) -> OrderReport {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("0").to_string();
        let (exec, quote) = (s("executedQty"), s("cummulativeQuoteQty"));
        let avg = match (exec.parse::<rust_decimal::Decimal>(), quote.parse::<rust_decimal::Decimal>()) {
            (Ok(e), Ok(q)) if !e.is_zero() => Some((q / e).round_dp(12).normalize().to_string()),
            _ => None,
        };
        OrderReport { order_id: v.get("orderId").and_then(Value::as_i64).unwrap_or(0), client_order_id: s("clientOrderId"), status: s("status"), executed_qty: exec, quote_qty: quote, avg_price: avg, raw: v }
    }

    /// `POST /api/v3/order`, MARKET. `client_order_id` is the idempotency key: Binance refuses a duplicate
    /// id for an open order, and [`Self::get_order`] finds the order by it after an unknown outcome.
    pub async fn place_market_order(&self, symbol: &str, side: &str, qty: &str, client_order_id: &str) -> Result<OrderReport, ExchangeError> {
        let params = vec![
            ("symbol".into(), symbol.into()),
            ("side".into(), side.into()),
            ("type".into(), "MARKET".into()),
            ("quantity".into(), qty.into()),
            ("newClientOrderId".into(), client_order_id.into()),
            ("newOrderRespType".into(), "FULL".into()),
        ];
        self.send(Method::POST, "/api/v3/order", Some(params), true).await.map(Self::report)
    }

    /// `GET /api/v3/order` by client order id. `Ok(None)` when Binance says the order does not exist (-2013).
    pub async fn get_order(&self, symbol: &str, client_order_id: &str) -> Result<Option<OrderReport>, ExchangeError> {
        match self.signed_read("/api/v3/order", vec![("symbol".into(), symbol.into()), ("origClientOrderId".into(), client_order_id.into())]).await {
            Ok(v) => Ok(Some(Self::report(v))),
            Err(ExchangeError::Api { code: -2013, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Quantity rounded DOWN to the symbol's step, as decimal text. `None` if it falls below the minimum.
pub fn normalize_qty(qty: rust_decimal::Decimal, rules: &SymbolRules) -> Option<String> {
    use std::str::FromStr;
    let step = rust_decimal::Decimal::from_str(&rules.step_size).ok()?;
    let min = rust_decimal::Decimal::from_str(&rules.min_qty).unwrap_or_default();
    let q = if step.is_zero() { qty } else { (qty / step).floor() * step };
    (q > rust_decimal::Decimal::ZERO && q >= min).then(|| q.normalize().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_the_official_documentation_example() {
        // Binance Spot API docs, "SIGNED endpoint examples for POST /api/v3/order" (HMAC keys)
        let secret = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
        let q = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
        assert_eq!(sign(secret, q), "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71");
    }

    #[test]
    fn quantity_is_rounded_down_to_the_step_and_checked_against_the_minimum() {
        use std::str::FromStr;
        let r = SymbolRules { status: "TRADING".into(), step_size: "0.00100000".into(), min_qty: "0.00100000".into(), min_notional: None };
        let d = |s: &str| rust_decimal::Decimal::from_str(s).unwrap();
        assert_eq!(normalize_qty(d("0.12399"), &r), Some("0.123".into()));
        assert_eq!(normalize_qty(d("0.0009"), &r), None);
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }
}
