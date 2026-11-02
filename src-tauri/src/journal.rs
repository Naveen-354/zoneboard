//! Trade journal on SQLite. Same table, same rules and same JSON as the Python app, so an existing
//! `journal.db` opens as-is. Journals made before crypto existed get a `market` column (all NSE).

use std::path::PathBuf;
use std::sync::Mutex;

use chrono::NaiveDate;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

pub const MARKETS: [&str; 2] = ["crypto", "india"];

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
    pub pnl: f64,
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
        return p;
    }
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join("journal.db");
        if p.exists() {
            return p.to_string_lossy().into_owned();
        }
    }
    let dir: PathBuf = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("ZoneBoard");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("journal.db").to_string_lossy().into_owned()
}

impl Journal {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                date TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL,
                qty REAL NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL,
                fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '',
                market TEXT NOT NULL DEFAULT 'india'
            );",
        )?;
        let has_market = conn.prepare("PRAGMA table_info(trades)")?.query_map([], |r| r.get::<_, String>(1))?.filter_map(Result::ok).any(|c| c == "market");
        if !has_market {
            conn.execute("ALTER TABLE trades ADD COLUMN market TEXT NOT NULL DEFAULT 'india'", [])?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn payload(&self, market: &str) -> Payload {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare("SELECT id, date, symbol, side, qty, entry, exit, fees, notes, market FROM trades WHERE market = ?1 ORDER BY date DESC, id DESC").unwrap();
        let trades: Vec<Trade> = st
            .query_map([market], |r| {
                let (side, qty, entry, exit, fees): (String, f64, f64, f64, f64) = (r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?);
                let gross = (exit - entry) * qty * if side == "long" { 1.0 } else { -1.0 };
                Ok(Trade { id: r.get(0)?, date: r.get(1)?, symbol: r.get(2)?, side, qty, entry, exit, fees, notes: r.get(8)?, market: r.get(9)?, pnl: round(gross - fees, 2) })
            })
            .unwrap()
            .filter_map(Result::ok)
            .collect();
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
        let market = match text("market").as_str() {
            "" => "india".to_string(),
            m => m.to_string(),
        };
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
        if !MARKETS.contains(&market.as_str()) {
            return bad(&format!("market must be one of {}", MARKETS.join(", ")));
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
            .execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![date, symbol, side, qty, entry, exit, fees, notes, market])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete(&self, id: i64) {
        let _ = self.conn.lock().unwrap().execute("DELETE FROM trades WHERE id = ?1", [id]);
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
        let c = j.payload("crypto");
        assert_eq!(c.trades.len(), 1);
        assert_eq!(c.trades[0].symbol, "BTCUSDT");
        assert_eq!(c.trades[0].qty, 0.0153);
        assert!((c.stats.net_pnl - (0.0153 * 10.0 - 1.0)).abs() < 0.01); // reported to the cent
        assert_eq!(j.payload("india").trades.len(), 1);
    }

    #[test]
    fn pnl_matches_the_python_app() {
        let j = j();
        j.add(&json!({"date":"2026-10-30","symbol":"sbin","side":"long","qty":10,"entry":100,"exit":110,"fees":20})).unwrap();
        j.add(&json!({"date":"2026-10-30","symbol":"INFY","side":"short","qty":5,"entry":100,"exit":110,"fees":10})).unwrap();
        let p = j.payload("india");
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
        assert_eq!(j.payload("crypto").stats.trades, 0);
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
        assert_eq!(j.payload("india").trades.len(), 1); // old trades were all NSE
        assert_eq!(j.payload("india").trades[0].notes, "old");
        assert_eq!(j.payload("crypto").trades.len(), 0);
        drop(j);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_removes_only_that_trade() {
        let j = j();
        j.add(&trade("crypto", "AAAUSDT", json!(1))).unwrap();
        j.add(&trade("crypto", "BBBUSDT", json!(1))).unwrap();
        let id = j.payload("crypto").trades[0].id;
        j.delete(id);
        assert_eq!(j.payload("crypto").trades.len(), 1);
    }
}
