//! Trading ledger and tax report support.
//!
//! The ledger is append-only: the database refuses UPDATE and DELETE on `ledger_entries` (triggers in migration
//! 9), so a mistake is corrected by a reversal entry that points at the original. Entries come from closed
//! trades (`sync`: gross P&L and fees per trade, idempotent by (ref_type, ref_id, kind)) and from the owner's
//! own adjustments, which need a memo. Amounts are decimals stored as text.
//!
//! Reports: realised totals per account, currency and year (calendar or the Indian financial year, April to
//! March), and a CSV export of every entry. Statement import reconciles a broker CSV (date, symbol, side, qty,
//! price, fee, ref) against the recorded trades and reports matches and differences; it never writes ledger
//! entries by itself. This is record keeping for the owner and their accountant, not tax advice: holding
//! periods, lot matching rules and tax rates are not decided here.

use std::collections::BTreeMap;
use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::accounts::{dec_str, realised};

pub const KINDS: [&str; 4] = ["realised_pnl", "fee", "adjustment", "reversal"];

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Add entries for every closed trade on an account that has none yet. Returns how many were added.
pub fn sync(conn: &Connection) -> Result<usize, String> {
    let rows: Vec<(i64, String, String, String, String, String, f64, f64, f64, f64)> = conn
        .prepare("SELECT t.id, t.date, t.account_id, t.market, t.symbol, t.side, t.qty, t.entry, t.exit, t.fees FROM trades t WHERE t.account_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM ledger_entries l WHERE l.ref_type = 'trade' AND l.ref_id = CAST(t.id AS TEXT))")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?))).map(|m| m.filter_map(Result::ok).collect()))
        .map_err(|e| e.to_string())?;
    let mut n = 0;
    for (id, date, acct, market, sym, side, qty, entry, exit, fees) in rows {
        let cur = crate::accounts::currency_for(&market, &sym).unwrap_or("USDT");
        let gross = realised(&side, qty, entry, exit, 0.0).ok_or("a trade's P&L does not fit a decimal")?;
        let fee = Decimal::from_f64_retain(fees).map(|d| -d.round_dp(8)).unwrap_or_default();
        let ts = format!("{date}T00:00:00Z");
        for (kind, amt, memo) in [("realised_pnl", gross, format!("{side} {qty} {sym} {entry} -> {exit}")), ("fee", fee, format!("fees on trade #{id}"))] {
            n += conn
                .execute("INSERT OR IGNORE INTO ledger_entries (ts_utc, account_id, kind, currency, amount, ref_type, ref_id, memo, created_utc) VALUES (?1,?2,?3,?4,?5,'trade',?6,?7,?8)", params![ts, acct, kind, cur, dec_str(amt), id.to_string(), memo, now()])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(n)
}

pub fn adjust(conn: &Connection, account: &str, currency: &str, amount: &str, memo: &str) -> Result<Value, String> {
    let amt = Decimal::from_str(amount.trim()).map_err(|_| "amount must be a decimal number".to_string())?;
    if amt.is_zero() {
        return Err("amount must not be zero".into());
    }
    let memo = memo.trim();
    if memo.len() < 3 || memo.len() > 300 {
        return Err("a memo (3 to 300 chars) is required for an adjustment".into());
    }
    if !["USDT", "INR"].contains(&currency) {
        return Err("currency must be USDT or INR".into());
    }
    conn.query_row("SELECT 1 FROM accounts WHERE id = ?1", [account], |_| Ok(())).map_err(|_| format!("unknown account {account}"))?;
    let refid = format!("adj-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    conn.execute("INSERT INTO ledger_entries (ts_utc, account_id, kind, currency, amount, ref_type, ref_id, memo, created_utc) VALUES (?1,?2,'adjustment',?3,?4,'manual',?5,?6,?1)", params![now(), account, currency, dec_str(amt), refid, memo]).map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    let _ = crate::audit::record(conn, "user", "ledger_adjustment", &format!("ledger:{id}"), &json!({ "account": account, "currency": currency, "amount": dec_str(amt) }));
    entry(conn, id)
}

/// Reverse an entry (once). The original stays; the reversal carries the negated amount.
pub fn reverse(conn: &Connection, id: i64, memo: &str) -> Result<Value, String> {
    let (acct, kind, cur, amt): (String, String, String, String) = conn.query_row("SELECT account_id, kind, currency, amount FROM ledger_entries WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(|_| "no such ledger entry".to_string())?;
    if kind == "reversal" {
        return Err("a reversal cannot itself be reversed: add an adjustment".into());
    }
    if memo.trim().len() < 3 {
        return Err("say why (memo) when reversing an entry".into());
    }
    let neg = -Decimal::from_str(&amt).map_err(|e| e.to_string())?;
    let n = conn
        .execute("INSERT OR IGNORE INTO ledger_entries (ts_utc, account_id, kind, currency, amount, ref_type, ref_id, reverses_id, memo, created_utc) VALUES (?1,?2,'reversal',?3,?4,'ledger',?5,?6,?7,?1)", params![now(), acct, cur, dec_str(neg), id.to_string(), id, memo.trim()])
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("this entry was already reversed".into());
    }
    let rid = conn.last_insert_rowid();
    let _ = crate::audit::record(conn, "user", "ledger_reversal", &format!("ledger:{id}"), &json!({ "reversal_id": rid }));
    entry(conn, rid)
}

fn entry_json(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({ "id": r.get::<_, i64>(0)?, "ts_utc": r.get::<_, String>(1)?, "account_id": r.get::<_, String>(2)?, "kind": r.get::<_, String>(3)?, "currency": r.get::<_, String>(4)?, "amount": r.get::<_, String>(5)?, "ref_type": r.get::<_, String>(6)?, "ref_id": r.get::<_, String>(7)?, "reverses_id": r.get::<_, Option<i64>>(8)?, "memo": r.get::<_, String>(9)?, "reversed": r.get::<_, i64>(10)? != 0 }))
}

const COLS: &str = "id, ts_utc, account_id, kind, currency, amount, ref_type, ref_id, reverses_id, memo, EXISTS (SELECT 1 FROM ledger_entries x WHERE x.reverses_id = l.id)";

pub fn entry(conn: &Connection, id: i64) -> Result<Value, String> {
    conn.query_row(&format!("SELECT {COLS} FROM ledger_entries l WHERE id = ?1"), [id], entry_json).optional().map_err(|e| e.to_string())?.ok_or("no such ledger entry".into())
}

pub fn entries(conn: &Connection, account: Option<&str>, from: Option<&str>, to: Option<&str>, limit: usize) -> Result<Vec<Value>, String> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM ledger_entries l WHERE (?1 IS NULL OR account_id = ?1) AND (?2 IS NULL OR ts_utc >= ?2) AND (?3 IS NULL OR ts_utc < ?3) ORDER BY ts_utc, id LIMIT ?4")).map_err(|e| e.to_string())?;
    let v = st.query_map(params![account, from, to, limit as i64], entry_json).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    Ok(v)
}

/// The tax year an ISO timestamp falls in: "2026" (calendar) or "FY2026-27" (India, April to March).
pub fn tax_year(ts: &str, basis: &str) -> String {
    let y: i32 = ts.get(..4).and_then(|x| x.parse().ok()).unwrap_or(0);
    let m: u32 = ts.get(5..7).and_then(|x| x.parse().ok()).unwrap_or(1);
    if basis == "india_fy" {
        let start = if m >= 4 { y } else { y - 1 };
        format!("FY{start}-{:02}", (start + 1) % 100)
    } else {
        y.to_string()
    }
}

pub fn report(conn: &Connection, basis: &str, account: Option<&str>) -> Result<Value, String> {
    if !["calendar", "india_fy"].contains(&basis) {
        return Err("basis must be calendar or india_fy".into());
    }
    let mut agg: BTreeMap<(String, String, String), [Decimal; 5]> = BTreeMap::new();
    for e in entries(conn, account, None, None, 1_000_000)? {
        let amt = Decimal::from_str(e["amount"].as_str().unwrap_or("0")).unwrap_or_default();
        let key = (tax_year(e["ts_utc"].as_str().unwrap_or(""), basis), e["account_id"].as_str().unwrap_or("").to_string(), e["currency"].as_str().unwrap_or("").to_string());
        let a = agg.entry(key).or_insert([Decimal::ZERO; 5]);
        match e["kind"].as_str().unwrap_or("") {
            "realised_pnl" if amt > Decimal::ZERO => a[0] += amt,
            "realised_pnl" => a[1] += amt,
            "fee" => a[2] += amt,
            _ => a[3] += amt,
        }
        a[4] += amt;
    }
    let rows: Vec<Value> = agg.into_iter().map(|((y, acct, cur), a)| json!({ "year": y, "account_id": acct, "currency": cur, "gains": dec_str(a[0]), "losses": dec_str(a[1]), "fees": dec_str(a[2]), "adjustments_and_reversals": dec_str(a[3]), "net": dec_str(a[4]) })).collect();
    Ok(json!({ "basis": basis, "rows": rows, "note": "Totals of recorded entries for your own records. Not tax advice: holding periods, lot matching and rates are for you and your accountant." }))
}

fn csv_cell(s: &str) -> String {
    // quote, and neutralise spreadsheet formulas
    let s = if s.starts_with(['=', '+', '-', '@']) && s.parse::<f64>().is_err() { format!("'{s}") } else { s.to_string() };
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn export_csv(conn: &Connection, account: Option<&str>, from: Option<&str>, to: Option<&str>) -> Result<String, String> {
    let mut out = String::from("id,ts_utc,account_id,kind,currency,amount,ref_type,ref_id,reverses_id,memo\n");
    for e in entries(conn, account, from, to, 1_000_000)? {
        let cells: Vec<String> = ["id", "ts_utc", "account_id", "kind", "currency", "amount", "ref_type", "ref_id", "reverses_id", "memo"].iter().map(|k| match &e[*k] {
            Value::String(s) => csv_cell(s),
            Value::Null => String::new(),
            v => v.to_string(),
        }).collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    Ok(out)
}

/// Reconcile a broker statement CSV against recorded trades on `account`. A statement row matches a trade with
/// the same date, symbol and side, quantity within 1e-8 and price within 0.5% of the entry or exit.
pub fn reconcile(conn: &Connection, account: &str, csv: &str) -> Result<Value, String> {
    conn.query_row("SELECT 1 FROM accounts WHERE id = ?1", [account], |_| Ok(())).map_err(|_| format!("unknown account {account}"))?;
    let mut lines = csv.lines().filter(|l| !l.trim().is_empty());
    let head: Vec<String> = lines.next().ok_or("empty statement")?.split(',').map(|h| h.trim().to_lowercase()).collect();
    let col = |n: &str| head.iter().position(|h| h == n);
    let (Some(cd), Some(cs), Some(csd), Some(cq), Some(cp)) = (col("date"), col("symbol"), col("side"), col("qty"), col("price")) else {
        return Err("statement needs date, symbol, side, qty and price columns (fee and ref optional)".into());
    };
    let trades: Vec<(i64, String, String, String, f64, f64, f64)> = conn
        .prepare("SELECT id, date, symbol, side, qty, entry, exit FROM trades WHERE account_id = ?1")
        .and_then(|mut st| st.query_map([account], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map(|m| m.filter_map(Result::ok).collect()))
        .map_err(|e| e.to_string())?;
    let mut used = std::collections::BTreeSet::new();
    let (mut matched, mut unmatched, mut bad) = (vec![], vec![], vec![]);
    let mut rows = 0;
    for (i, l) in lines.enumerate().take(10_000) {
        rows += 1;
        let c: Vec<&str> = l.split(',').map(str::trim).collect();
        let g = |k: usize| c.get(k).copied().unwrap_or("");
        let (Ok(q), Ok(p)) = (g(cq).parse::<f64>(), g(cp).parse::<f64>()) else {
            bad.push(json!({ "line": i + 2, "error": "qty and price must be numbers" }));
            continue;
        };
        let (date, sym) = (g(cd).get(..10).unwrap_or(""), g(cs).to_uppercase());
        let side = match g(csd).to_lowercase().as_str() {
            "buy" | "long" => "long",
            "sell" | "short" => "short",
            _ => {
                bad.push(json!({ "line": i + 2, "error": "side must be buy/sell or long/short" }));
                continue;
            }
        };
        let near = |a: f64, b: f64| b > 0.0 && ((a - b) / b).abs() <= 0.005;
        let hit = trades.iter().find(|t| !used.contains(&t.0) && t.1 == date && t.2 == sym && t.3 == side && (t.4 - q).abs() < 1e-8 && (near(p, t.5) || near(p, t.6)));
        let row = json!({ "line": i + 2, "date": date, "symbol": sym, "side": side, "qty": q, "price": p, "ref": col("ref").map(|k| g(k)) });
        match hit {
            Some(t) => {
                used.insert(t.0);
                matched.push(json!({ "statement": row, "trade_id": t.0 }));
            }
            None => unmatched.push(row),
        }
    }
    let missing: Vec<i64> = trades.iter().filter(|t| !used.contains(&t.0)).map(|t| t.0).collect();
    let rep = json!({ "account_id": account, "rows": rows, "matched": matched, "unmatched_statement_rows": unmatched, "trades_not_on_statement": missing, "rejected_lines": bad, "note": "Reconciliation only: no ledger entries were written." });
    conn.execute("INSERT INTO ledger_imports (account_id, created_utc, rows, matched, unmatched, report) VALUES (?1,?2,?3,?4,?5,?6)", params![account, now(), rows, matched.len() as i64, unmatched.len() as i64, rep.to_string()]).map_err(|e| e.to_string())?;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let c = crate::db::open(":memory:").unwrap();
        crate::accounts::seed_from_legacy(&c).unwrap();
        for (d, e, x, f) in [("2026-03-20", 100.0, 110.0, 0.5), ("2026-04-02", 100.0, 95.0, 0.25)] {
            c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode, account_id) VALUES (?1,'BTCUSDT','long',2,?2,?3,?4,'','crypto','demo','demo-1')", params![d, e, x, f]).unwrap();
        }
        c
    }

    #[test]
    fn entries_are_immutable_idempotent_and_reversible() {
        let c = setup();
        assert_eq!(sync(&c).unwrap(), 4);
        assert_eq!(sync(&c).unwrap(), 0, "idempotent");
        let first = entries(&c, None, None, None, 10).unwrap()[0]["id"].as_i64().unwrap();
        assert!(c.execute("UPDATE ledger_entries SET amount = '0' WHERE id = ?1", [first]).is_err());
        assert!(c.execute("DELETE FROM ledger_entries WHERE id = ?1", [first]).is_err());
        let r = reverse(&c, first, "booked twice in error").unwrap();
        assert_eq!(r["amount"], "-20");
        assert!(reverse(&c, first, "again").is_err(), "once only");
        assert!(reverse(&c, r["id"].as_i64().unwrap(), "no").is_err());
        assert!(adjust(&c, "demo-1", "USDT", "1.5", "").is_err());
        adjust(&c, "demo-1", "USDT", "1.5", "rounding difference").unwrap();
        // calendar: both in 2026; India FY splits at April
        let cal = report(&c, "calendar", None).unwrap();
        assert_eq!(cal["rows"].as_array().unwrap().len(), 1);
        let fy = report(&c, "india_fy", None).unwrap();
        let years: Vec<&str> = fy["rows"].as_array().unwrap().iter().map(|r| r["year"].as_str().unwrap()).collect();
        assert_eq!(years, vec!["FY2025-26", "FY2026-27"]);
        // gains 20, losses -10, fees -0.75, reversal -20, adjustment 1.5 => net -9.25
        assert_eq!(cal["rows"][0]["net"], "-9.25", "{cal}");
        let csv = export_csv(&c, None, None, None).unwrap();
        assert_eq!(csv.lines().count(), 7);
        assert_eq!(csv_cell("=HYPERLINK(1)"), "\"'=HYPERLINK(1)\"");
    }

    #[test]
    fn statements_reconcile_without_writing_entries() {
        let c = setup();
        let r = reconcile(&c, "demo-1", "date,symbol,side,qty,price,fee,ref\n2026-03-20,BTCUSDT,BUY,2,100.2,0.5,T1\n2026-05-01,ETHUSDT,SELL,1,50,0,T9\nx,y,z,a,b\n").unwrap();
        assert_eq!(r["matched"].as_array().unwrap().len(), 1);
        assert_eq!(r["unmatched_statement_rows"].as_array().unwrap().len(), 1);
        assert_eq!(r["trades_not_on_statement"].as_array().unwrap().len(), 1);
        assert_eq!(r["rejected_lines"].as_array().unwrap().len(), 1);
        assert_eq!(c.query_row("SELECT count(*) FROM ledger_entries", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        assert!(reconcile(&c, "demo-1", "a,b\n1,2").is_err());
    }
}
