//! Broker adapters. One module per provider; each exposes what it really supports through [`Capabilities`],
//! so the UI and the risk engine never assume an operation a provider or account does not have.
//!
//! Adapter contract (the methods each provider client offers, adapted to its real API):
//! `verify` (permissions and key safety), `account` (balances), `server_time`, and for providers whose
//! order path is built, `place_order` with a client order id used as the idempotency key, `get_order` and
//! `cancel_order`. Strategies never call an adapter: orders reach it only through the order manager and
//! the risk engine.

pub mod binance;

use serde::Serialize;

/// What a connected provider can do for this account. Shown in the UI; checked before orders.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Capabilities {
    pub provider: &'static str,
    pub environment: String,
    pub markets: Vec<&'static str>,
    pub read_balances: bool,
    pub place_orders: bool,
    pub order_types: Vec<&'static str>,
    pub market_data: &'static str,
    pub notes: Vec<&'static str>,
}

/// A balance line as the broker reports it. Amounts are the broker's own decimal strings, untouched.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Balance {
    pub asset: String,
    pub free: String,
    pub locked: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExchangeError {
    /// Bad, expired or revoked key, or a signature the provider rejected.
    Auth(String),
    /// HTTP 429 or 418: we stop until this many seconds have passed. Never retried in a loop.
    RateLimited { retry_after_secs: u64, banned: bool },
    /// The provider answered with an error code.
    Api { status: u16, code: i64, msg: String },
    /// No answer, or an answer we could not read. For an order this means the outcome is UNKNOWN.
    Network(String),
    /// Refused by our own safety rules before anything was sent.
    Refused(String),
}

impl std::fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExchangeError::Auth(m) => write!(f, "authentication failed: {m}"),
            ExchangeError::RateLimited { retry_after_secs, banned } => {
                write!(f, "{} by the exchange; paused for {retry_after_secs} s", if *banned { "IP temporarily banned (418)" } else { "rate limited (429)" })
            }
            ExchangeError::Api { status, code, msg } => write!(f, "exchange error {code} (HTTP {status}): {msg}"),
            ExchangeError::Network(m) => write!(f, "network: {m}"),
            ExchangeError::Refused(m) => write!(f, "{m}"),
        }
    }
}
