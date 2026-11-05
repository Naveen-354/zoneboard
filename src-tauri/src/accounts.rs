//! Virtual (paper) accounts and their funds. T005.
//!
//! Everything here is practice bookkeeping: the demo account holds *virtual* credits, every balance the API
//! returns carries `"virtual": true`, and the app holds no money. Fills are still priced by the page (a
//! client can send any price), so the funds check is not a security boundary.
//!
//! Money is `Decimal` with checked arithmetic. The legacy `REAL` columns are converted once, through their
//! shortest decimal text ([`Money::from_legacy_f64`]); an overflow is a clean error, never a panic (a panic
//! would poison the journal's mutex).
//!
//! A wallet keeps two numbers: `credits` (what the owner configured) and `balance` (credits plus realised P&L
//! since the last reset). Available funds are `balance - notional of open positions`, which is the spec's
//! "credits + realised P&L - open notional". Reset restores `balance` to `credits` and keeps the trade history.

use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Map, Value};

use crate::money::Money;

pub const DEFAULT_ACCOUNT: &str = "demo-1";
/// Wallet currencies, in the order the API lists them.
pub const CURRENCIES: [&str; 2] = ["USDT", "INR"];
const MAX_DEMO_ACCOUNTS: i64 = 10;
const MAX_LABEL_CHARS: usize = 60;
const NOTICE: &str = "Virtual practice credits. This app holds no money.";

/// Largest credit, wallet or order notional we accept (1e15). Far below `Decimal`'s range, so sums of
/// capped values cannot overflow in practice; the checked operations cover the rest.
fn cap() -> Decimal {
    Decimal::from(1_000_000_000_000_000i64)
}

fn default_credits(currency: &str) -> Decimal {
    if currency == "USDT" {
        Decimal::from(10_000)
    } else {
        Decimal::from(1_000_000)
    }
}

/// Decimal places a credit value may carry: crypto to the satoshi-ish 8, rupees to the paisa.
fn input_scale(currency: &str) -> u32 {
    if currency == "USDT" {
        8
    } else {
        2
    }
}

#[derive(Debug, PartialEq)]
pub enum AccountError {
    NotFound,
    Invalid(String),
    Db(String),
}

impl From<rusqlite::Error> for AccountError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e.to_string())
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T, AccountError> {
    Err(AccountError::Invalid(m.into()))
}

// ------------------------------------------------------------------ pure maths

/// Which wallet an order draws on: NSE is INR, USDT-quoted crypto is USDT, anything else has no wallet.
pub fn currency_for(market: &str, symbol: &str) -> Result<&'static str, String> {
    match market {
        "india" => Ok("INR"),
        "crypto" if symbol.ends_with("USDT") => Ok("USDT"),
        "crypto" => Err(format!("no virtual wallet for {symbol}: only USDT-quoted crypto and NSE (INR) can be traded on the demo account yet")),
        other => Err(format!("no virtual wallet for market {other}")),
    }
}

fn to_dec(x: f64) -> Option<Decimal> {
    Money::from_legacy_f64(x).map(|m| m.0)
}

fn round8(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(8, RoundingStrategy::MidpointNearestEven)
}

/// Decimal text for storage and the API: no exponent, no trailing zeros.
pub fn dec_str(d: Decimal) -> String {
    d.normalize().to_string()
}

fn parse_stored(s: &str) -> Result<Decimal, AccountError> {
    Decimal::from_str(s).map_err(|_| AccountError::Db(format!("corrupt decimal in wallet: {s}")))
}

/// Notional of a position or order, `qty * price`. `None` on overflow or a non-finite input.
pub fn notional(qty: f64, price: f64) -> Option<Decimal> {
    to_dec(qty)?.checked_mul(to_dec(price)?)
}

/// Realised P&L of a closed position, net of fees, rounded half-even to 8 places. One function for the
/// close path and the migration seed, so they cannot disagree. `None` on overflow.
pub fn realised(side: &str, qty: f64, entry: f64, exit: f64, fees: f64) -> Option<Decimal> {
    let gross = to_dec(exit)?.checked_sub(to_dec(entry)?)?.checked_mul(to_dec(qty)?)?;
    let gross = if side == "long" { gross } else { -gross };
    Some(round8(gross.checked_sub(to_dec(fees)?)?))
}

/// Sum of the notional of an account's open positions in one currency. Legacy rows whose currency cannot
/// be derived (an odd crypto quote) or whose notional overflows are skipped with a warning.
pub fn open_notional(conn: &Connection, account_id: &str, currency: &str) -> Result<Decimal, AccountError> {
    let mut st = conn.prepare("SELECT market, symbol, qty, entry FROM positions WHERE account_id = ?1")?;
    let rows: Vec<(String, String, f64, f64)> = st
        .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .filter_map(Result::ok)
        .collect();
    let mut total = Decimal::ZERO;
    for (market, symbol, qty, entry) in rows {
        let Ok(cur) = currency_for(&market, &symbol) else {
            tracing::warn!(account = account_id, symbol = %symbol, "open position has no virtual wallet; not reserved");
            continue;
        };
        if cur != currency {
            continue;
        }
        let Some(n) = notional(qty, entry) else {
            tracing::warn!(account = account_id, symbol = %symbol, "open position notional overflows; not reserved");
            continue;
        };
        total = total.checked_add(n).ok_or_else(|| AccountError::Invalid("open positions too large to total".into()))?;
    }
    Ok(total)
}

/// `(credits, balance)` of one wallet.
fn wallet(conn: &Connection, account_id: &str, currency: &str) -> Result<Option<(Decimal, Decimal)>, AccountError> {
    let row: Option<(String, String)> = conn
        .query_row("SELECT credits, balance FROM demo_wallets WHERE account_id = ?1 AND currency = ?2", params![account_id, currency], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    row.map(|(c, b)| Ok((parse_stored(&c)?, parse_stored(&b)?))).transpose()
}

/// The order path's account gate: must exist, be a demo account and not be archived.
pub fn require_demo(conn: &Connection, account_id: &str) -> Result<(), String> {
    let row: Option<(String, i64)> = conn
        .query_row("SELECT kind, archived FROM accounts WHERE id = ?1", [account_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    match row {
        Some((kind, 0)) if kind == "demo" => Ok(()),
        _ => Err("invalid order: unknown or archived demo account".to_string()),
    }
}

/// Reject an order that needs more than the wallet has free. `notional <= available` is accepted.
/// Shorts reserve their full notional like longs; the entry fee is not reserved (charged at close).
pub fn check_funds(conn: &Connection, account_id: &str, market: &str, symbol: &str, qty: f64, price: f64) -> Result<(), String> {
    let currency = currency_for(market, symbol)?;
    let need = notional(qty, price).ok_or("invalid order: order size is too large")?;
    if need > cap() {
        return Err(format!("invalid order: order notional is above the {} limit", dec_str(cap())));
    }
    let (_, balance) = wallet(conn, account_id, currency).map_err(|e| format!("{e:?}"))?.ok_or_else(|| format!("no virtual {currency} wallet on this account"))?;
    let reserved = open_notional(conn, account_id, currency).map_err(|e| format!("{e:?}"))?;
    let available = balance.checked_sub(reserved).ok_or("invalid order: wallet arithmetic overflow")?;
    if need > available {
        return Err(format!(
            "insufficient virtual funds: this order needs {} {currency} but only {} {currency} of virtual credit is available",
            dec_str(need),
            dec_str(available)
        ));
    }
    Ok(())
}

/// Add realised P&L to a wallet, in the caller's transaction. A missing wallet (non-demo account) is a no-op.
pub fn apply_pnl(conn: &Connection, account_id: &str, currency: &str, net: Decimal) -> Result<(), String> {
    let Some((_, balance)) = wallet(conn, account_id, currency).map_err(|e| format!("{e:?}"))? else {
        return Ok(());
    };
    let next = round8(balance.checked_add(net).ok_or("wallet arithmetic overflow")?);
    conn.execute("UPDATE demo_wallets SET balance = ?1 WHERE account_id = ?2 AND currency = ?3", params![dec_str(next), account_id, currency])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ------------------------------------------------------------------ migration seed

/// Called by migration 4, inside its transaction, after `demo-1`, the wallets and the `account_id` backfill
/// exist. Replays the paper engine's closed trades into the wallet and writes the one audit row.
/// Hand-entered demo trades are linked to `demo-1` but do not move the wallet.
pub fn seed_from_legacy(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut st = conn.prepare("SELECT market, symbol, side, qty, entry, exit, fees FROM trades WHERE account_id = ?1 AND notes = 'Paper trade'")?;
    let rows: Vec<(String, String, String, f64, f64, f64, f64)> =
        st.query_map([DEFAULT_ACCOUNT], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?.filter_map(Result::ok).collect();
    drop(st);
    let mut sums = [Decimal::ZERO; 2];
    let mut skipped = 0;
    for (market, symbol, side, qty, entry, exit, fees) in rows {
        let slot = currency_for(&market, &symbol).ok().and_then(|c| CURRENCIES.iter().position(|x| *x == c));
        let net = realised(&side, qty, entry, exit, fees);
        match (slot, net) {
            (Some(i), Some(n)) => match sums[i].checked_add(n) {
                Some(s) => sums[i] = s,
                None => skipped += 1,
            },
            _ => skipped += 1,
        }
    }
    if skipped > 0 {
        tracing::warn!(skipped, "paper trades could not be replayed into the demo wallet");
    }
    let mut credits = Map::new();
    let mut seeded = Map::new();
    for (i, cur) in CURRENCIES.iter().enumerate() {
        let (c, _) = wallet(conn, DEFAULT_ACCOUNT, cur).ok().flatten().unwrap_or((default_credits(cur), default_credits(cur)));
        let balance = round8(c + sums[i]);
        conn.execute("UPDATE demo_wallets SET balance = ?1 WHERE account_id = ?2 AND currency = ?3", params![dec_str(balance), DEFAULT_ACCOUNT, cur])?;
        credits.insert(cur.to_string(), json!(dec_str(c)));
        seeded.insert(cur.to_string(), json!(dec_str(sums[i])));
    }
    let count = |table: &str| -> Result<i64, rusqlite::Error> { conn.query_row(&format!("SELECT count(*) FROM {table} WHERE account_id = ?1"), [DEFAULT_ACCOUNT], |r| r.get(0)) };
    let detail = json!({
        "account_id": DEFAULT_ACCOUNT, "kind": "demo", "label": "Demo", "environment": "paper",
        "credits": credits, "seeded_realised": seeded,
        "linked_trades": count("trades")?, "linked_positions": count("positions")?, "skipped_trades": skipped,
    });
    crate::audit::record(conn, "system", "account_created", &format!("account:{DEFAULT_ACCOUNT}"), &detail)?;
    Ok(())
}

// ------------------------------------------------------------------ reads

/// One account as the API shows it. `"virtual": true` is set here, in the one place wallets are built, so
/// no endpoint can return a demo balance without it.
pub fn account_json(conn: &Connection, id: &str) -> Result<Value, AccountError> {
    let row: Option<(String, String, String, String, String, i64, Option<String>, Option<String>, Option<String>)> = conn
        .query_row("SELECT kind, label, environment, status, created_utc, archived, key_hint, last_sync_utc, last_error FROM accounts WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        })
        .optional()?;
    let Some((kind, label, environment, status, created_utc, archived, key_hint, last_sync, last_error)) = row else {
        return Err(AccountError::NotFound);
    };
    let mut wallets = Vec::new();
    for cur in CURRENCIES {
        if let Some((credits, balance)) = wallet(conn, id, cur)? {
            let reserved = open_notional(conn, id, cur)?;
            let available = balance.checked_sub(reserved).unwrap_or(Decimal::ZERO);
            wallets.push(json!({
                "currency": cur, "credits": dec_str(credits), "balance": dec_str(balance),
                "in_positions": dec_str(reserved), "available": dec_str(available), "virtual": true,
            }));
        }
    }
    let open_positions: i64 = conn.query_row("SELECT count(*) FROM positions WHERE account_id = ?1", [id], |r| r.get(0))?;
    // Only demo accounts hold virtual credits. A broker account's balances come from the broker
    // (`/api/accounts/{id}/balances`), never from here.
    let is_demo = kind == "demo";
    Ok(json!({
        "id": id, "kind": kind, "label": label, "environment": environment, "status": status,
        "created_utc": created_utc, "archived": archived != 0, "virtual": is_demo,
        "open_positions": open_positions, "wallets": wallets,
        "key_hint": key_hint, "last_sync_utc": last_sync, "last_error": last_error,
    }))
}

pub fn list(conn: &Connection) -> Result<Value, AccountError> {
    let ids: Vec<String> = {
        let mut st = conn.prepare("SELECT id FROM accounts ORDER BY rowid")?;
        let mapped = st.query_map([], |r| r.get(0))?;
        mapped.filter_map(Result::ok).collect()
    };
    let mut accounts = Vec::new();
    for id in ids {
        accounts.push(account_json(conn, &id)?);
    }
    Ok(json!({ "accounts": accounts, "notice": NOTICE }))
}

// ------------------------------------------------------------------ writes (each one transaction + one audit row)

fn parse_label(v: &Value) -> Result<String, AccountError> {
    let Some(s) = v.as_str() else {
        return invalid("label must be text");
    };
    let s = s.trim();
    let n = s.chars().count();
    if n == 0 || n > MAX_LABEL_CHARS {
        return invalid(format!("label must be 1 to {MAX_LABEL_CHARS} characters"));
    }
    Ok(s.to_string())
}

/// A credit amount: a decimal number or numeric string, finite, not negative, at most 1e15, and no more
/// decimal places than the currency allows. Rejected, never silently rounded.
fn parse_credit(cur: &str, v: &Value) -> Result<Decimal, AccountError> {
    let text = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return invalid(format!("credits.{cur} must be a number")),
    };
    let Ok(d) = Decimal::from_str(&text) else {
        return invalid(format!("credits.{cur} must be a decimal number"));
    };
    if d.is_sign_negative() && !d.is_zero() {
        return invalid(format!("credits.{cur} cannot be negative"));
    }
    if d > cap() {
        return invalid(format!("credits.{cur} is above the {} limit", dec_str(cap())));
    }
    if d.normalize().scale() > input_scale(cur) {
        return invalid(format!("credits.{cur} allows at most {} decimal places", input_scale(cur)));
    }
    Ok(d)
}

/// The `credits` object of a request: only known currencies, each valid. Returns the ones that were given.
fn parse_credits(v: &Value) -> Result<Vec<(&'static str, Decimal)>, AccountError> {
    let Some(map) = v.as_object() else {
        return invalid("credits must be an object like {\"USDT\": \"10000\", \"INR\": \"1000000\"}");
    };
    if let Some(k) = map.keys().find(|k| !CURRENCIES.contains(&k.as_str())) {
        return invalid(format!("unknown currency '{k}': credits can be set for {}", CURRENCIES.join(", ")));
    }
    let mut out = Vec::new();
    for cur in CURRENCIES {
        if let Some(x) = map.get(cur) {
            out.push((cur, parse_credit(cur, x)?));
        }
    }
    Ok(out)
}

fn object(body: &Value) -> Result<&Map<String, Value>, AccountError> {
    body.as_object().ok_or_else(|| AccountError::Invalid("request body must be a JSON object".into()))
}

/// `POST /api/accounts`: a new demo account. Broker accounts arrive with T007.
pub fn create(conn: &mut Connection, body: &Value) -> Result<Value, AccountError> {
    let obj = object(body)?;
    if let Some(k) = obj.keys().find(|k| !["kind", "label", "credits", "environment"].contains(&k.as_str())) {
        return invalid(format!("unknown field '{k}'"));
    }
    match obj.get("kind").and_then(Value::as_str).unwrap_or("demo") {
        "demo" => {}
        "fyers" => return invalid("FYERS connections are not built yet (see docs/fyers/SPEC.md)"),
        "binance" => return invalid("Binance accounts are connected with api_key and api_secret (handled by connections.rs)"),
        _ => return invalid("kind must be demo"),
    }
    if let Some(e) = obj.get("environment") {
        if e.as_str() != Some("paper") {
            return invalid("a demo account's environment is always paper");
        }
    }
    let label = parse_label(obj.get("label").unwrap_or(&Value::Null))?;
    let given = match obj.get("credits") {
        Some(c) => parse_credits(c)?,
        None => Vec::new(),
    };
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let count: i64 = tx.query_row("SELECT count(*) FROM accounts WHERE kind = 'demo'", [], |r| r.get(0))?;
    if count >= MAX_DEMO_ACCOUNTS {
        return invalid(format!("at most {MAX_DEMO_ACCOUNTS} demo accounts"));
    }
    let id = (1..=1000)
        .map(|n| format!("demo-{n}"))
        .find(|id| tx.query_row("SELECT 1 FROM accounts WHERE id = ?1", [id], |_| Ok(())).optional().ok().flatten().is_none())
        .ok_or_else(|| AccountError::Invalid("no free demo account id".into()))?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    tx.execute("INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived) VALUES (?1,'demo',?2,'paper','active',?3,0)", params![id, label, now])?;
    let mut credits = Map::new();
    for cur in CURRENCIES {
        let c = given.iter().find(|(k, _)| *k == cur).map(|(_, d)| *d).unwrap_or_else(|| default_credits(cur));
        tx.execute("INSERT INTO demo_wallets (account_id, currency, credits, balance) VALUES (?1,?2,?3,?3)", params![id, cur, dec_str(c)])?;
        credits.insert(cur.to_string(), json!(dec_str(c)));
    }
    let detail = json!({ "account_id": id, "kind": "demo", "label": label, "environment": "paper", "credits": credits });
    crate::audit::record(&tx, "user", "account_created", &format!("account:{id}"), &detail)?;
    tx.commit()?;
    account_json(conn, &id)
}

/// `PATCH /api/accounts/{id}`: `label` and `credits` only. All or nothing; a request that changes nothing
/// writes nothing and audits nothing. Changing credits shifts `balance` by the same amount, so realised P&L
/// survives.
pub fn update(conn: &mut Connection, id: &str, body: &Value) -> Result<Value, AccountError> {
    let obj = object(body)?;
    if let Some(k) = obj.keys().find(|k| !["label", "credits"].contains(&k.as_str())) {
        return invalid(format!("unknown field '{k}': only label and credits can be changed"));
    }
    let label = obj.get("label").map(parse_label).transpose()?;
    let credits = obj.get("credits").map(parse_credits).transpose()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current: Option<String> = tx.query_row("SELECT label FROM accounts WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    let Some(old_label) = current else {
        return Err(AccountError::NotFound);
    };
    let (mut old, mut new) = (Map::new(), Map::new());
    if let Some(l) = label.filter(|l| *l != old_label) {
        tx.execute("UPDATE accounts SET label = ?1 WHERE id = ?2", params![l, id])?;
        old.insert("label".into(), json!(old_label));
        new.insert("label".into(), json!(l));
    }
    let (mut old_c, mut new_c) = (Map::new(), Map::new());
    for (cur, value) in credits.unwrap_or_default() {
        let Some((was, balance)) = wallet(&tx, id, cur)? else {
            return invalid(format!("this account has no {cur} wallet"));
        };
        if was == value {
            continue;
        }
        let shifted = value.checked_sub(was).and_then(|d| balance.checked_add(d)).map(round8);
        let Some(shifted) = shifted else {
            return invalid("credit change overflows the wallet");
        };
        tx.execute(
            "UPDATE demo_wallets SET credits = ?1, balance = ?2 WHERE account_id = ?3 AND currency = ?4",
            params![dec_str(value), dec_str(shifted), id, cur],
        )?;
        old_c.insert(cur.to_string(), json!(dec_str(was)));
        new_c.insert(cur.to_string(), json!(dec_str(value)));
    }
    if !new_c.is_empty() {
        old.insert("credits".into(), Value::Object(old_c));
        new.insert("credits".into(), Value::Object(new_c));
    }
    if new.is_empty() {
        drop(tx); // nothing changed: roll back, write no audit row
        return account_json(conn, id);
    }
    let detail = json!({ "account_id": id, "old": old, "new": new });
    crate::audit::record(&tx, "user", "account_updated", &format!("account:{id}"), &detail)?;
    tx.commit()?;
    account_json(conn, id)
}

/// `POST /api/accounts/{id}/reset`: open positions removed (no trade is written), every wallet back to its
/// credits, trade history kept.
pub fn reset(conn: &mut Connection, id: &str) -> Result<Value, AccountError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let exists: Option<i64> = tx.query_row("SELECT 1 FROM accounts WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    if exists.is_none() {
        return Err(AccountError::NotFound);
    }
    let removed: Vec<Value> = {
        let mut st = tx.prepare("SELECT symbol, side, qty, entry FROM positions WHERE account_id = ?1 ORDER BY id LIMIT 50")?;
        let mapped = st.query_map([id], |r| Ok(json!({ "symbol": r.get::<_, String>(0)?, "side": r.get::<_, String>(1)?, "qty": r.get::<_, f64>(2)?, "entry": r.get::<_, f64>(3)? })))?;
        mapped.filter_map(Result::ok).collect()
    };
    let removed_total = tx.execute("DELETE FROM positions WHERE account_id = ?1", [id])?;
    let (mut before, mut after) = (Map::new(), Map::new());
    for cur in CURRENCIES {
        if let Some((credits, balance)) = wallet(&tx, id, cur)? {
            tx.execute("UPDATE demo_wallets SET balance = credits WHERE account_id = ?1 AND currency = ?2", params![id, cur])?;
            before.insert(cur.to_string(), json!(dec_str(balance)));
            after.insert(cur.to_string(), json!(dec_str(credits)));
        }
    }
    let detail = json!({ "account_id": id, "positions_removed": removed_total, "positions": removed, "balance_before": before, "balance_after": after });
    crate::audit::record(&tx, "user", "account_reset", &format!("account:{id}"), &detail)?;
    tx.commit()?;
    account_json(conn, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn currency_rules() {
        assert_eq!(currency_for("india", "SBIN"), Ok("INR"));
        assert_eq!(currency_for("crypto", "BTCUSDT"), Ok("USDT"));
        let e = currency_for("crypto", "ETHBTC").unwrap_err();
        assert!(e.starts_with("no virtual wallet for ETHBTC"), "{e}");
        assert!(currency_for("forex", "X").is_err());
    }

    #[test]
    fn realised_is_exact_and_matches_the_float_fee_formula() {
        // 0.01 BTC from 78000 to 79000, 0.1% per side on each side's notional: no binary float noise survives
        let fees = 0.1 / 100.0 * 0.01 * (78000.0 + 79000.0);
        let net = realised("long", 0.01, 78000.0, 79000.0, fees).unwrap();
        assert_eq!(net, round8(d("10") - to_dec(fees).unwrap()));
        assert_eq!(realised("short", 10.0, 100.0, 90.0, 0.0).unwrap(), d("100"));
        assert_eq!(realised("short", 10.0, 100.0, 110.0, 0.0).unwrap(), d("-100"));
        assert_eq!(realised("long", 1.0, 100.0, 90.0, 1.0).unwrap(), d("-11"));
    }

    #[test]
    fn rounding_is_half_even_at_the_eighth_decimal() {
        // exact ties at the 9th place: half-even goes to the even digit, half-away would not
        let net = |fee: f64| realised("long", 1.0, 100.0, 100.0, fee).unwrap();
        assert_eq!(net(0.000000005), d("0"), "-0.000000005 ties to 0 (even), not -0.00000001");
        assert_eq!(net(0.000000015), d("-0.00000002"), "1.5 ties to 2 (even)");
        assert_eq!(net(0.000000025), d("-0.00000002"), "2.5 ties to 2 (even), not 3");
        assert_eq!(net(0.000000035), d("-0.00000004"), "3.5 ties to 4 (even)");
    }

    #[test]
    fn overflow_is_none_not_a_panic() {
        assert!(notional(1e20, 1e20).is_none());
        assert!(notional(1e300, 1.0).is_none()); // too long to be a Decimal at all
        assert!(realised("long", 1e20, 1.0, 1e20, 0.0).is_none());
        assert!(notional(f64::NAN, 1.0).is_none());
    }

    #[test]
    fn credit_parsing_rejects_instead_of_rounding() {
        let ok = |c: &str, v: Value| parse_credit(c, &v).map(dec_str);
        assert_eq!(ok("USDT", json!("12.5")), Ok("12.5".into()));
        assert_eq!(ok("USDT", json!(0)), Ok("0".into()));
        assert_eq!(ok("INR", json!(100.25)), Ok("100.25".into()));
        for (c, v) in [("USDT", json!(-1)), ("USDT", json!("abc")), ("USDT", json!(1e40)), ("USDT", json!("0.123456789")), ("INR", json!("1.001")), ("INR", json!(null)), ("USDT", json!("1e-9")), ("USDT", json!(1.0e15 + 1e9))] {
            assert!(parse_credit(c, &v).is_err(), "{c} {v}");
        }
        assert_eq!(ok("USDT", json!("1e5")), Ok("100000".into())); // an exponent is a valid way to write 100000
        assert_eq!(ok("USDT", json!("1000000000000000")), Ok("1000000000000000".into())); // exactly the cap
    }
}
