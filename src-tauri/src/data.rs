//! Market data for the server and the backtest.
//!
//! * NSE stocks: Yahoo Finance chart endpoint. Unofficial and delayed (~15 min), personal use only.
//!   Live NSE data needs a broker feed (Fyers or Zerodha Kite); it would slot in next to `yahoo_frame`.
//! * Crypto candles for the backtest: Binance public market data (no key). The live crypto board does
//!   not come through here: the browser talks to Binance directly.
//! * `DEMO=1`: deterministic synthetic bars, no network.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, FixedOffset, TimeZone, Utc, Weekday};
use futures_util::stream::{self, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::zones::Frame;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";
const BINANCE_REST: &str = "https://data-api.binance.vision";

/// interval -> (Yahoo range, seconds to cache)
pub const INTERVALS: [(&str, &str, u64); 3] = [("1d", "1y", 300), ("1h", "6mo", 300), ("15m", "1mo", 120)];

pub fn http_client() -> Client {
    Client::builder().user_agent(UA).timeout(Duration::from_secs(20)).gzip(true).build().expect("http client")
}

// ------------------------------------------------------------------ symbol lists
const DEFAULT_NSE: &str = include_str!("../../symbols.txt");
const DEFAULT_CRYPTO: &str = include_str!("../../symbols_crypto.txt");

/// A symbols file next to where the app runs wins (so you can edit the list); otherwise the built-in copy.
fn read_list(name: &str, default: &str) -> String {
    let mut dirs: Vec<PathBuf> = vec![std::env::current_dir().unwrap_or_default()];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    dirs.iter().find_map(|d| std::fs::read_to_string(d.join(name)).ok()).unwrap_or_else(|| default.to_string())
}

fn parse_symbols(text: &str, valid: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in text.lines() {
        let s = line.trim().to_uppercase();
        if !s.is_empty() && !s.starts_with('#') && valid(&s) && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

pub fn load_symbols() -> Vec<String> {
    parse_symbols(&read_list("symbols.txt", DEFAULT_NSE), |s| valid_nse_symbol(s))
}

pub fn load_crypto_symbols() -> Vec<String> {
    parse_symbols(&read_list("symbols_crypto.txt", DEFAULT_CRYPTO), |s| (5..=20).contains(&s.len()) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
}

/// 1-20 chars of A-Z 0-9 & -  (same rule as the Python app).
pub fn valid_nse_symbol(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '&' || c == '-')
}

// ------------------------------------------------------------------ Yahoo
#[derive(Deserialize)]
struct YReply {
    chart: YChart,
}
#[derive(Deserialize)]
struct YChart {
    result: Option<Vec<YResult>>,
}
#[derive(Deserialize)]
struct YResult {
    timestamp: Option<Vec<i64>>,
    indicators: YIndicators,
}
#[derive(Deserialize)]
struct YIndicators {
    quote: Vec<YQuote>,
}
#[derive(Deserialize)]
struct YQuote {
    open: Vec<Option<f64>>,
    high: Vec<Option<f64>>,
    low: Vec<Option<f64>>,
    close: Vec<Option<f64>>,
    volume: Vec<Option<f64>>,
}

/// Bars for one NSE symbol. Rows with gaps or impossible values are dropped (as the Python `_clean`).
pub async fn yahoo_frame(client: &Client, sym: &str, range: &str, interval: &str) -> Option<Frame> {
    let mut url = reqwest::Url::parse("https://query1.finance.yahoo.com/v8/finance/chart/").ok()?;
    url.path_segments_mut().ok()?.pop_if_empty().push(&format!("{sym}.NS"));
    url.query_pairs_mut().append_pair("range", range).append_pair("interval", interval).append_pair("includePrePost", "false");
    let reply: YReply = client.get(url).send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
    let r = reply.chart.result?.into_iter().next()?;
    let ts = r.timestamp?;
    let q = r.indicators.quote.into_iter().next()?;
    let mut f = Frame::default();
    for (i, &t) in ts.iter().enumerate() {
        let get = |v: &Vec<Option<f64>>| v.get(i).copied().flatten();
        if let (Some(o), Some(h), Some(l), Some(c), Some(v)) = (get(&q.open), get(&q.high), get(&q.low), get(&q.close), get(&q.volume)) {
            if h >= l && c > 0.0 {
                f.push(t, o, h, l, c, v);
            }
        }
    }
    (!f.is_empty()).then_some(f)
}

// ------------------------------------------------------------------ Binance (backtest history)
/// Up to `bars` candles ending now, paging backwards 1000 at a time. Oldest first.
pub async fn binance_frame(client: &Client, sym: &str, interval: &str, bars: usize) -> Result<Frame, String> {
    let mut rows: Vec<Vec<serde_json::Value>> = vec![];
    let mut end: Option<i64> = None;
    while rows.len() < bars {
        let mut url = format!("{BINANCE_REST}/api/v3/klines?symbol={sym}&interval={interval}&limit=1000");
        if let Some(e) = end {
            url.push_str(&format!("&endTime={e}"));
        }
        let page: Vec<Vec<serde_json::Value>> =
            client.get(&url).send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| format!("{sym}: {e}"))?.json().await.map_err(|e| e.to_string())?;
        if page.is_empty() {
            break;
        }
        end = page[0][0].as_i64().map(|t| t - 1);
        let full = page.len() == 1000;
        rows.splice(0..0, page);
        if !full {
            break;
        }
    }
    let skip = rows.len().saturating_sub(bars);
    let num = |v: &serde_json::Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(f64::NAN);
    let mut f = Frame::default();
    for r in rows.iter().skip(skip) {
        f.push(r[0].as_i64().unwrap_or(0) / 1000, num(&r[1]), num(&r[2]), num(&r[3]), num(&r[4]), num(&r[5]));
    }
    Ok(f)
}

// ------------------------------------------------------------------ demo data
/// Deterministic random-walk bars for `DEMO=1`. Prices are made up and mean nothing.
pub fn synthetic(sym: &str, interval: &str) -> Frame {
    let mut seed = fnv(&format!("{sym}{interval}")) | 1;
    let mut rnd = move || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        (seed.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut normal = move |sd: f64| sd * (-2.0 * (1.0 - rnd()).ln()).sqrt() * (2.0 * std::f64::consts::PI * rnd()).cos();

    let ist = FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
    let today = Utc::now().with_timezone(&ist).date_naive();
    let sessions = |n: usize| {
        let mut days = vec![];
        let mut d = today;
        while days.len() < n {
            if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
                days.push(d);
            }
            d = d.pred_opt().unwrap();
        }
        days.reverse();
        days
    };
    let at = |d: chrono::NaiveDate, minutes: i64| ist.from_local_datetime(&d.and_hms_opt(9, 15, 0).unwrap()).unwrap().timestamp() + minutes * 60;
    let (times, vol_scale): (Vec<i64>, f64) = match interval {
        "1d" => (sessions(260).into_iter().map(|d| at(d, 0)).collect(), 0.015),
        "15m" => (sessions(22).into_iter().flat_map(|d| (0..25).map(move |i| (d, i * 15))).map(|(d, m)| at(d, m)).collect(), 0.004),
        _ => (sessions(120).into_iter().flat_map(|d| (0..7).map(move |i| (d, i * 60))).map(|(d, m)| at(d, m)).collect(), 0.007),
    };

    let n = times.len();
    let base = 100.0 + (fnv(sym) % 3000) as f64;
    let mut close = Vec::with_capacity(n);
    let mut level = base.ln();
    for _ in 0..n {
        level += normal(vol_scale);
        close.push(level.exp());
    }
    let mut f = Frame::default();
    for i in 0..n {
        let prev = if i == 0 { close[0] } else { close[i - 1] };
        let o = prev * (1.0 + normal(vol_scale / 4.0));
        let span = normal(vol_scale / 2.0).abs() * close[i];
        let v = (13.0 + normal(0.4)).exp();
        f.push(times[i], o, o.max(close[i]) + span, o.min(close[i]) - span, close[i], v);
    }
    f
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

// ------------------------------------------------------------------ cached store
type OneCache = HashMap<(String, String), (Instant, Option<Arc<Frame>>)>;

pub struct DataStore {
    client: Client,
    pub demo: bool,
    one: Mutex<OneCache>,
    all: Mutex<Option<(Instant, Arc<HashMap<String, Arc<Frame>>>)>>,
}

impl DataStore {
    pub fn new(demo: bool) -> Self {
        Self { client: http_client(), demo, one: Mutex::new(HashMap::new()), all: Mutex::new(None) }
    }

    /// Bars for one NSE symbol at 1d / 1h / 15m, cached for a few minutes.
    pub async fn ohlcv(&self, sym: &str, interval: &str) -> Option<Arc<Frame>> {
        let &(_, range, ttl) = INTERVALS.iter().find(|x| x.0 == interval)?;
        let key = (sym.to_string(), interval.to_string());
        if let Some((t, v)) = self.one.lock().await.get(&key) {
            if t.elapsed() < Duration::from_secs(ttl) {
                return v.clone();
            }
        }
        let frame = if self.demo { Some(synthetic(sym, interval)) } else { yahoo_frame(&self.client, sym, range, interval).await };
        let frame = frame.map(Arc::new);
        self.one.lock().await.insert(key, (Instant::now(), frame.clone()));
        frame
    }

    /// Daily bars for the whole scan list, fetched 8 at a time and cached for 5 minutes.
    pub async fn daily_all(&self) -> Arc<HashMap<String, Arc<Frame>>> {
        let mut slot = self.all.lock().await; // held across the fetch so concurrent callers share one scan
        if let Some((t, v)) = slot.as_ref() {
            if t.elapsed() < Duration::from_secs(300) {
                return v.clone();
            }
        }
        let out: HashMap<String, Arc<Frame>> = if self.demo {
            load_symbols().into_iter().map(|s| (s.clone(), Arc::new(synthetic(&s, "1d")))).collect()
        } else {
            stream::iter(load_symbols())
                .map(|s| async move { (s.clone(), yahoo_frame(&self.client, &s, "1y", "1d").await) })
                .buffer_unordered(8)
                .filter_map(|(s, f)| async move { f.filter(|f| f.len() > 60).map(|f| (s, Arc::new(f))) })
                .collect()
                .await
        };
        let out = Arc::new(out);
        *slot = Some((Instant::now(), out.clone()));
        out
    }

    pub fn client(&self) -> &Client {
        &self.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_rules_match_the_python_app() {
        assert!(valid_nse_symbol("M&M") && valid_nse_symbol("BAJAJ-AUTO"));
        assert!(!valid_nse_symbol("") && !valid_nse_symbol("bad") && !valid_nse_symbol("../ETC") && !valid_nse_symbol(&"A".repeat(21)));
    }

    #[test]
    fn built_in_symbol_lists_load() {
        let nse = parse_symbols(DEFAULT_NSE, valid_nse_symbol);
        assert!(nse.len() > 60 && nse.contains(&"RELIANCE".to_string()) && nse.contains(&"M&M".to_string()));
        let crypto = parse_symbols(DEFAULT_CRYPTO, |s| s.ends_with("USDT"));
        assert!(crypto.len() >= 20 && crypto.contains(&"BTCUSDT".to_string()));
    }

    #[test]
    fn synthetic_bars_are_deterministic_and_sane() {
        let a = synthetic("AAA", "1d");
        let b = synthetic("AAA", "1d");
        assert_eq!(a.c, b.c);
        assert_eq!(a.len(), 260);
        assert!(a.t.windows(2).all(|w| w[1] > w[0]));
        assert!((0..a.len()).all(|i| a.h[i] >= a.l[i] && a.c[i] > 0.0));
        assert_eq!(synthetic("AAA", "15m").len(), 22 * 25);
        assert_eq!(synthetic("AAA", "1h").len(), 120 * 7);
    }
}
