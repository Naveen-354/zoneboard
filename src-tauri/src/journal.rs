//! Trade journal on SQLite. Same table, same rules and same JSON as the Python app, so an existing
//! `journal.db` opens as-is. Journals made before crypto existed get a `market` column (all NSE).

use std::sync::Mutex;

use chrono::{FixedOffset, NaiveDate, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

pub const MARKETS: [&str; 2] = ["crypto", "india"];
/// The account. "demo" = paper trading on real prices, the only account that can place orders here.
/// "live" = real orders, which this app does not place: no broker is connected.
pub const MODES: [&str; 2] = ["live", "demo"];

/// Simulated fee per side, percent of notional. Round trip 0.2% / 0.1%: the same costs the backtest assumes.
pub fn fee_pct(market: &str) -> f64 {
    if market == "crypto" {
        0.1
    } else {
        0.05
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Trade {
    pub id: i64,
    pub date: String,
    pub symbol: String,
    pub side: String,
    pub qty: f64,
    pub entry: f64,
    pub exit: f64,
    pub fees: f64,
    pub notes: String,
    pub market: String,
    pub mode: String,
    pub pnl: f64,
}

/// An open paper position. Closing it writes a [`Trade`] and removes it.
#[derive(Serialize, Clone, Debug)]
pub struct Position {
    pub id: i64,
    pub market: String,
    pub mode: String,
    pub symbol: String,
    pub side: String,
    pub qty: f64,
    pub entry: f64,
    pub fee_pct: f64,
    pub opened_at: String,
}

/// Why a close failed: no such position, or the request itself was bad.
#[derive(Debug, PartialEq)]
pub enum CloseError {
    NotFound,
    Invalid(String),
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Stats {
    pub trades: usize,
    pub net_pnl: f64,
    pub fees: f64,
    pub win_rate: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub profit_factor: Option<f64>,
}

#[derive(Serialize)]
pub struct Payload {
    pub trades: Vec<Trade>,
    pub stats: Stats,
}

pub struct Journal {
    conn: Mutex<Connection>,
}

fn round(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

/// Where the journal lives: `JOURNAL_DB`, else a `journal.db` in the folder you launched from (so an
/// existing journal keeps working), else the user's data folder.
pub fn default_path() -> String {
    if let Ok(p) = std::env::var("JOURNAL_DB") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            tracing::info!(path = %trimmed, source = "JOURNAL_DB", "resolved journal path");
            return trimmed.to_string();
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("journal.db");
        if p.exists() {
            let s = p.to_string_lossy().into_owned();
            tracing::info!(path = %s, source = "current_dir", "resolved journal path");
            return s;
        }
    }
    let p = crate::db::data_dir().join("journal.db");
    let s = p.to_string_lossy().into_owned();
    tracing::info!(path = %s, source = "data_dir", "resolved journal path");
    s
}

impl Journal {
    pub fn open(path: &str) -> Result<Self, crate::db::DbError> {
        let conn = crate::db::open(path)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Access the underlying SQLite connection inside a closure helper.
    pub fn with_conn<R>(&self, f: impl FnOnce(&mut Connection) -> R) -> R {
        let mut conn = self.conn.lock().unwrap();
        f(&mut conn)
    }

    const TRADE_COLS: &'static str = "id, date, symbol, side, qty, entry, exit, fees, notes, market, mode";

    fn trade_from_row(r: &rusqlite::Row) -> rusqlite::Result<Trade> {
        let (side, qty, entry, exit, fees): (String, f64, f64, f64, f64) = (r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?);
        let gross = (exit - entry) * qty * if side == "long" { 1.0 } else { -1.0 };
        Ok(Trade { id: r.get(0)?, date: r.get(1)?, symbol: r.get(2)?, side, qty, entry, exit, fees, notes: r.get(8)?, market: r.get(9)?, mode: r.get(10)?, pnl: round(gross - fees, 2) })
    }

    pub fn payload(&self, market: &str, mode: &str) -> Payload {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {} FROM trades WHERE market = ?1 AND mode = ?2 ORDER BY date DESC, id DESC", Self::TRADE_COLS);
        let mut st = conn.prepare(&sql).unwrap();
        let trades: Vec<Trade> = st.query_map([market, mode], Self::trade_from_row).unwrap().filter_map(Result::ok).collect();
        let stats = stats_of(&trades);
        Payload { trades, stats }
    }

    /// Validate and insert a trade from the JSON a form posts (numbers may arrive as strings).
    pub fn add(&self, body: &Value) -> Result<(), String> {
        let bad = |m: &str| Err(format!("invalid trade: {m}"));
        let text = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let date = text("date");
        if NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_err() {
            return bad(&format!("time data '{date}' does not match format '%Y-%m-%d'"));
        }
        let or_default = |k: &str, d: &str| match text(k).as_str() {
            "" => d.to_string(),
            v => v.to_string(),
        };
        let (market, mode) = (or_default("market", "india"), or_default("mode", "live"));
        let symbol = text("symbol").trim().to_uppercase();
        let side = text("side").to_lowercase();
        let (Some(qty), Some(entry), Some(exit)) = (number(body.get("qty")), number(body.get("entry")), number(body.get("exit"))) else {
            return bad("qty, entry and exit must be numbers");
        };
        let fees = match body.get("fees") {
            None | Some(Value::Null) => 0.0,
            Some(Value::String(s)) if s.trim().is_empty() => 0.0,
            v => match number(v) {
                Some(f) => f,
                None => return bad("fees must be a number"),
            },
        };
        let notes: String = text("notes").chars().take(500).collect();
        if let Err(e) = check_market_mode(&market, &mode) {
            return bad(&e);
        }
        if !crate::data::valid_nse_symbol(&symbol) || !matches!(side.as_str(), "long" | "short") {
            return bad("symbol or side invalid");
        }
        if ![qty, entry, exit, fees].iter().all(|x| x.is_finite()) {
            return bad("numbers must be finite");
        }
        if qty <= 0.0 || entry <= 0.0 || exit <= 0.0 || fees < 0.0 {
            return bad("qty, prices must be positive; fees cannot be negative");
        }
        if market == "india" && qty.fract() != 0.0 {
            return bad("NSE quantity must be a whole number");
        }
        self.conn
            .lock()
            .unwrap()
            .execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![date, symbol, side, qty, entry, exit, fees, notes, market, mode])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete(&self, id: i64) {
        let _ = self.conn.lock().unwrap().execute("DELETE FROM trades WHERE id = ?1", [id]);
    }

    // ---------------------------------------------------------------- paper positions
    // Orders are simulated: a market order fills at the price the page sends (the real price on screen).
    // Closing a position records it as a trade. No broker is involved and no real money moves.

    const POSITION_COLS: &'static str = "id, market, mode, symbol, side, qty, entry, fee_pct, opened_at";

    fn position_from_row(r: &rusqlite::Row) -> rusqlite::Result<Position> {
        Ok(Position { id: r.get(0)?, market: r.get(1)?, mode: r.get(2)?, symbol: r.get(3)?, side: r.get(4)?, qty: r.get(5)?, entry: r.get(6)?, fee_pct: r.get(7)?, opened_at: r.get(8)? })
    }

    pub fn positions(&self, market: &str, mode: &str) -> Vec<Position> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {} FROM positions WHERE market = ?1 AND mode = ?2 ORDER BY id DESC", Self::POSITION_COLS);
        let mut st = conn.prepare(&sql).unwrap();
        st.query_map([market, mode], Self::position_from_row).unwrap().filter_map(Result::ok).collect()
    }

    /// Open a position from `{market, mode, symbol, side, qty, price}`. Fills at `price`.
    pub fn open_position(&self, body: &Value) -> Result<Position, String> {
        let bad = |m: &str| Err(format!("invalid order: {m}"));
        let text = |k: &str, d: &str| match body.get(k).and_then(Value::as_str).unwrap_or("") {
            "" => d.to_string(),
            v => v.to_string(),
        };
        let (market, mode) = (text("market", "india"), text("mode", "demo"));
        if let Err(e) = check_market_mode(&market, &mode) {
            return bad(&e);
        }
        if mode != "demo" {
            // a "live" order would claim a real trade that never happened
            return bad("live orders aren't supported: no broker is connected, so only the demo account can trade");
        }
        let symbol = text("symbol", "").trim().to_uppercase();
        let side = text("side", "").to_lowercase();
        if !crate::data::valid_nse_symbol(&symbol) || !matches!(side.as_str(), "long" | "short") {
            return bad("symbol or side invalid");
        }
        let qty = match positive(body.get("qty"), "qty") {
            Ok(v) => v,
            Err(e) => return bad(&e),
        };
        let price = match positive(body.get("price"), "price") {
            Ok(v) => v,
            Err(e) => return bad(&e),
        };
        if market == "india" && qty.fract() != 0.0 {
            return bad("NSE quantity must be a whole number");
        }
        let opened = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![market, mode, symbol, side, qty, price, fee_pct(&market), opened],
        )
        .map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();
        let sql = format!("SELECT {} FROM positions WHERE id = ?1", Self::POSITION_COLS);
        conn.query_row(&sql, [id], Self::position_from_row).map_err(|e| e.to_string())
    }

    /// Close position `id` at `body.price` (optionally `body.date`, YYYY-MM-DD). Records the trade, removes the
    /// position, in one transaction. Fees are the position's rate on each side's own notional.
    pub fn close_position(&self, id: i64, body: &Value) -> Result<Trade, CloseError> {
        let invalid = |m: String| CloseError::Invalid(format!("invalid close: {m}"));
        let price = positive(body.get("price"), "price").map_err(invalid)?;
        let date = match body.get("date") {
            None | Some(Value::Null) => None,
            Some(Value::String(d)) if NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok() => Some(d.clone()),
            Some(_) => return Err(invalid("date must be YYYY-MM-DD".into())),
        };
        let mut conn = self.conn.lock().unwrap();
        let db = |e: rusqlite::Error| CloseError::Invalid(e.to_string());
        let tx = conn.transaction().map_err(db)?;
        let sql = format!("SELECT {} FROM positions WHERE id = ?1", Self::POSITION_COLS);
        let pos = tx.query_row(&sql, [id], Self::position_from_row).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => CloseError::NotFound,
            e => db(e),
        })?;
        let day = date.unwrap_or_else(|| {
            let tz = if pos.market == "india" { FixedOffset::east_opt(5 * 3600 + 1800).unwrap() } else { FixedOffset::east_opt(0).unwrap() };
            Utc::now().with_timezone(&tz).format("%Y-%m-%d").to_string()
        });
        let fees = pos.fee_pct / 100.0 * pos.qty * (pos.entry + price); // both sides, at their own prices
        tx.execute(
            "INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![day, pos.symbol, pos.side, pos.qty, pos.entry, price, fees, "Paper trade", pos.market, pos.mode],
        )
        .map_err(db)?;
        let trade_id = tx.last_insert_rowid();
        tx.execute("DELETE FROM positions WHERE id = ?1", [id]).map_err(db)?;
        let sql = format!("SELECT {} FROM trades WHERE id = ?1", Self::TRADE_COLS);
        let trade = tx.query_row(&sql, [trade_id], Self::trade_from_row).map_err(db)?;
        tx.commit().map_err(db)?;
        Ok(trade)
    }
}

pub fn check_market_mode(market: &str, mode: &str) -> Result<(), String> {
    if !MARKETS.contains(&market) {
        return Err(format!("market must be one of {}", MARKETS.join(", ")));
    }
    if !MODES.contains(&mode) {
        return Err(format!("mode must be one of {}", MODES.join(", ")));
    }
    Ok(())
}

/// A finite number above zero, from a JSON number or numeric string.
fn positive(v: Option<&Value>, name: &str) -> Result<f64, String> {
    match number(v) {
        Some(x) if x.is_finite() && x > 0.0 => Ok(x),
        _ => Err(format!("{name} must be a positive number")),
    }
}

/// A JSON number, or a string holding one ("0.0153" from a form field). Not NaN-checked here.
fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn stats_of(trades: &[Trade]) -> Stats {
    let pnls: Vec<f64> = trades.iter().map(|t| t.pnl).collect();
    let wins: Vec<f64> = pnls.iter().copied().filter(|&p| p > 0.0).collect();
    let losses: Vec<f64> = pnls.iter().copied().filter(|&p| p <= 0.0).collect();
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let loss_sum: f64 = losses.iter().sum();
    Stats {
        trades: trades.len(),
        net_pnl: if trades.is_empty() { 0.0 } else { round(pnls.iter().sum(), 2) },
        fees: round(trades.iter().map(|t| t.fees).sum(), 2),
        win_rate: if trades.is_empty() { 0.0 } else { round(100.0 * wins.len() as f64 / trades.len() as f64, 1) },
        avg_win: round(mean(&wins), 2),
        avg_loss: round(mean(&losses), 2),
        profit_factor: (!losses.is_empty() && loss_sum < 0.0).then(|| round(wins.iter().sum::<f64>() / -loss_sum, 2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn j() -> Journal {
        Journal::open(":memory:").unwrap()
    }

    fn trade(market: &str, symbol: &str, qty: Value) -> Value {
        json!({"date":"2026-11-02","side":"long","entry":100,"exit":110,"fees":1,"market":market,"symbol":symbol,"qty":qty})
    }

    #[test]
    fn trades_are_kept_per_market_and_crypto_takes_fractional_qty() {
        let j = j();
        j.add(&trade("crypto", "btcusdt", json!("0.0153"))).unwrap(); // form fields arrive as strings
        j.add(&trade("india", "SBIN", json!(10))).unwrap();
        let c = j.payload("crypto", "live");
        assert_eq!(c.trades.len(), 1);
        assert_eq!(c.trades[0].symbol, "BTCUSDT");
        assert_eq!(c.trades[0].qty, 0.0153);
        assert!((c.stats.net_pnl - (0.0153 * 10.0 - 1.0)).abs() < 0.01); // reported to the cent
        assert_eq!(j.payload("india", "live").trades.len(), 1);
    }

    #[test]
    fn pnl_matches_the_python_app() {
        let j = j();
        j.add(&json!({"date":"2026-10-30","symbol":"sbin","side":"long","qty":10,"entry":100,"exit":110,"fees":20})).unwrap();
        j.add(&json!({"date":"2026-10-30","symbol":"INFY","side":"short","qty":5,"entry":100,"exit":110,"fees":10})).unwrap();
        let p = j.payload("india", "live");
        assert_eq!(p.stats.trades, 2);
        assert!((p.stats.net_pnl - (100.0 - 20.0 - 50.0 - 10.0)).abs() < 1e-9);
        assert_eq!(p.stats.win_rate, 50.0);
        assert_eq!(p.stats.profit_factor, Some(1.33)); // 80 / 60
    }

    #[test]
    fn validation() {
        let j = j();
        let ok = |mut v: Value, k: &str, x: Value| {
            v[k] = x;
            v
        };
        let base = trade("crypto", "X", json!(1));
        assert!(j.add(&ok(base.clone(), "market", json!("forex"))).is_err());
        assert!(j.add(&ok(base.clone(), "qty", json!(0))).is_err());
        assert!(j.add(&ok(base.clone(), "qty", json!("nan"))).is_err());
        assert!(j.add(&ok(base.clone(), "qty", json!("inf"))).is_err());
        assert!(j.add(&ok(base.clone(), "date", json!("nope"))).is_err());
        assert!(j.add(&ok(base.clone(), "side", json!("sideways"))).is_err());
        assert!(j.add(&ok(base.clone(), "fees", json!(-1))).is_err());
        assert!(j.add(&ok(trade("india", "X", json!(1.5)), "qty", json!(1.5))).is_err()); // NSE lots are whole
        assert!(j.add(&ok(base.clone(), "symbol", json!("../etc"))).is_err());
        assert_eq!(j.payload("crypto", "live").stats.trades, 0);
        assert!(j.add(&base).is_ok());
    }

    #[test]
    fn old_journal_gets_a_market_column_and_keeps_its_trades() {
        let path = std::env::temp_dir().join(format!("zb_old_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE trades (id INTEGER PRIMARY KEY AUTOINCREMENT, date TEXT NOT NULL, symbol TEXT NOT NULL,
                 side TEXT NOT NULL, qty INTEGER NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL,
                 fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '');
                 INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes) VALUES ('2026-10-01','SBIN','long',10,100,110,5,'old');",
            )
            .unwrap();
        }
        let j = Journal::open(path.to_str().unwrap()).unwrap();
        assert_eq!(j.payload("india", "live").trades.len(), 1); // old trades were all NSE
        assert_eq!(j.payload("india", "live").trades[0].notes, "old");
        assert_eq!(j.payload("india", "live").trades[0].mode, "live"); // made at real prices
        assert_eq!(j.payload("crypto", "live").trades.len(), 0);
        assert_eq!(j.payload("india", "demo").trades.len(), 0);
        assert!(j.positions("india", "live").is_empty());
        drop(j);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_removes_only_that_trade() {
        let j = j();
        j.add(&trade("crypto", "AAAUSDT", json!(1))).unwrap();
        j.add(&trade("crypto", "BBBUSDT", json!(1))).unwrap();
        let id = j.payload("crypto", "live").trades[0].id;
        j.delete(id);
        assert_eq!(j.payload("crypto", "live").trades.len(), 1);
    }

    fn order(market: &str, mode: &str, symbol: &str, side: &str, qty: Value, price: Value) -> Value {
        json!({"market": market, "mode": mode, "symbol": symbol, "side": side, "qty": qty, "price": price})
    }

    #[test]
    fn paper_position_lifecycle_long_and_short() {
        let j = j();
        let pos = j.open_position(&order("crypto", "demo", "btcusdt", "long", json!(0.01), json!(78000))).unwrap();
        assert_eq!((pos.symbol.as_str(), pos.entry, pos.fee_pct), ("BTCUSDT", 78000.0, 0.1));
        assert!(pos.opened_at.ends_with('Z'));
        assert_eq!(j.positions("crypto", "demo").len(), 1);

        let t = j.close_position(pos.id, &json!({"price": 79000, "date": "2026-11-02"})).unwrap();
        let fees = 0.1 / 100.0 * 0.01 * (78000.0 + 79000.0); // each side at its own price
        assert!((t.fees - fees).abs() < 1e-9);
        assert_eq!(t.pnl, round(0.01 * 1000.0 - fees, 2));
        assert_eq!((t.date.as_str(), t.mode.as_str(), t.notes.as_str()), ("2026-11-02", "demo", "Paper trade"));
        assert!(j.positions("crypto", "demo").is_empty());
        assert_eq!(j.payload("crypto", "demo").trades.len(), 1);
        assert!(matches!(j.close_position(pos.id, &json!({"price": 79000})), Err(CloseError::NotFound))); // already closed
    }

    #[test]
    fn a_short_wins_when_price_falls() {
        let j = j();
        let pos = j.open_position(&order("india", "demo", "SBIN", "short", json!(10), json!(100))).unwrap();
        assert_eq!(pos.fee_pct, 0.05);
        let t = j.close_position(pos.id, &json!({"price": 90})).unwrap();
        assert_eq!(t.pnl, round(100.0 - 0.05 / 100.0 * 10.0 * 190.0, 2));
        assert_eq!(t.side, "short");
    }

    #[test]
    fn live_account_refuses_orders_and_demo_trades_stay_out_of_it() {
        let j = j();
        // no broker is connected, so a "live" order would claim a real trade that never happened
        let err = j.open_position(&order("crypto", "live", "ETHUSDT", "long", json!(1), json!(2000))).unwrap_err();
        assert!(err.contains("live orders"), "{err}");
        assert!(j.positions("crypto", "live").is_empty());

        let pos = j.open_position(&order("crypto", "demo", "ETHUSDT", "long", json!(1), json!(2000))).unwrap();
        assert!(j.positions("crypto", "live").is_empty());
        assert_eq!(j.positions("crypto", "demo").len(), 1);
        j.close_position(pos.id, &json!({"price": 2100})).unwrap();
        assert_eq!(j.payload("crypto", "live").trades.len(), 0);
        let demo = j.payload("crypto", "demo");
        assert_eq!((demo.trades.len(), demo.trades[0].mode.as_str()), (1, "demo"));
    }

    #[test]
    fn earlier_paper_trades_recorded_as_live_move_to_the_demo_account() {
        // the previous build filed the ticket's paper trades under "live" (meaning real prices); they were never real orders
        let path = std::env::temp_dir().join(format!("zb_prev_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE trades (id INTEGER PRIMARY KEY AUTOINCREMENT, date TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL,
                 qty REAL NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL, fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '',
                 market TEXT NOT NULL DEFAULT 'india', mode TEXT NOT NULL DEFAULT 'live');
                 CREATE TABLE positions (id INTEGER PRIMARY KEY AUTOINCREMENT, market TEXT NOT NULL, mode TEXT NOT NULL DEFAULT 'live',
                 symbol TEXT NOT NULL, side TEXT NOT NULL, qty REAL NOT NULL, entry REAL NOT NULL, fee_pct REAL NOT NULL, opened_at TEXT NOT NULL);
                 INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES
                   ('2026-11-02','BTCUSDT','long',0.01,78000,79000,1.5,'Paper trade','crypto','live'),
                   ('2026-10-01','SBIN','long',10,100,110,5,'bought via my broker','india','live');
                 INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES ('crypto','live','ETHUSDT','long',1,2000,0.1,'2026-11-02T10:00:00Z');",
            )
            .unwrap();
        }
        let j = Journal::open(path.to_str().unwrap()).unwrap();
        let syms = |p: Payload| p.trades.into_iter().map(|t| t.symbol).collect::<Vec<_>>();
        assert_eq!(syms(j.payload("crypto", "live")), Vec::<String>::new());
        assert_eq!(syms(j.payload("crypto", "demo")), ["BTCUSDT"]); // the paper trade moved
        assert_eq!(syms(j.payload("india", "live")), ["SBIN"]); // the hand-entered broker trade stayed live
        assert!(j.positions("crypto", "live").is_empty());
        assert_eq!(j.positions("crypto", "demo").len(), 1);
        drop(j);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn order_validation() {
        let j = j();
        let with = |k: &str, v: Value| {
            let mut o = order("crypto", "demo", "BTCUSDT", "long", json!(1), json!(100));
            o[k] = v;
            o
        };
        for (k, v) in [
            ("price", json!(0)), ("price", json!(-5)), ("price", json!("nan")), ("price", json!("inf")), ("qty", json!(0)), ("qty", json!("abc")),
            ("side", json!("buy")), ("market", json!("forex")), ("mode", json!("paper")), ("symbol", json!("../etc")), ("symbol", json!("")),
        ] {
            assert!(j.open_position(&with(k, v.clone())).is_err(), "{k}={v}");
        }
        assert!(j.open_position(&order("india", "demo", "SBIN", "long", json!(1.5), json!(100))).is_err()); // NSE lots are whole
        assert!(j.positions("crypto", "demo").is_empty());

        let id = j.open_position(&order("crypto", "demo", "BTCUSDT", "long", json!(1), json!(100))).unwrap().id;
        assert!(matches!(j.close_position(id, &json!({"price": 0})), Err(CloseError::Invalid(_))));
        assert!(matches!(j.close_position(id, &json!({"price": 5, "date": "nope"})), Err(CloseError::Invalid(_))));
        assert!(matches!(j.close_position(id, &json!({})), Err(CloseError::Invalid(_))));
        assert_eq!(j.positions("crypto", "demo").len(), 1, "a rejected close must leave the position open");
        assert!(matches!(j.close_position(999_999, &json!({"price": 1})), Err(CloseError::NotFound)));
    }
}
