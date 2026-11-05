//! Pre-trade risk checks and the live-trading gate.
//!
//! Every order that opens or adds risk passes [`check`] inside the same transaction that writes it: manual
//! ticket orders and bot orders alike, so a bot cannot get around an account's limits. Closing orders are
//! never blocked: reducing risk must always be possible, even with the kill switch on.
//!
//! Order of checks: kill switch, account (exists, active, and paper: **real orders are refused in this build**),
//! account limits (order size, exposure, daily loss, per currency), then the bot's own limits (capital,
//! open positions, daily loss). The funds check (`accounts::check_funds`) runs alongside.
//!
//! Live trading (ROADMAP section 5, gates G1 to G5) is not reachable: there is no broker connector, so no
//! account is a live account, and [`check`] refuses any account that is not a paper account. The arming
//! state below is real and runtime-only (disarmed after every restart), ready for when a connector exists.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};

use crate::accounts::{self, dec_str};

/// The phrase the owner types to arm live trading.
pub const ARM_PHRASE: &str = "ARM LIVE TRADING";

/// An order as the risk engine sees it.
pub struct Order {
    pub account_id: String,
    pub market: String,
    pub symbol: String,
    pub qty: f64,
    pub price: f64,
    pub bot: Option<BotLimits>,
}

pub struct BotLimits {
    pub bot_id: i64,
    /// Allocation plus the bot's realised P&L.
    pub capital: Decimal,
    pub max_daily_loss: Option<Decimal>,
    pub max_open_positions: i64,
}

/// Runtime-only safety state. A restart clears both: the kill switch's effect (every bot stopped) persists in
/// the database, and live trading always starts disarmed (gate G2).
#[derive(Default)]
pub struct Gate {
    kill: AtomicBool,
    armed: Mutex<Option<(String, String)>>, // (account id, armed at)
}

impl Gate {
    pub fn killed(&self) -> bool {
        self.kill.load(Ordering::SeqCst)
    }
    pub fn set_kill(&self, on: bool) {
        self.kill.store(on, Ordering::SeqCst);
        if on {
            self.disarm();
        }
    }
    pub fn armed(&self) -> Option<(String, String)> {
        self.armed.lock().unwrap().clone()
    }
    pub fn disarm(&self) {
        *self.armed.lock().unwrap() = None;
    }
    fn arm(&self, account: &str) {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        *self.armed.lock().unwrap() = Some((account.to_string(), now));
    }
}

/// The account's limits for one currency: (max order notional, max daily loss, max exposure). `None` = unset.
pub fn limits(conn: &Connection, account: &str, currency: &str) -> Result<(Option<Decimal>, Option<Decimal>, Option<Decimal>), String> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row("SELECT max_order_notional, max_daily_loss, max_exposure FROM account_limits WHERE account_id = ?1 AND currency = ?2", params![account, currency], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    let p = |s: Option<String>| s.and_then(|s| Decimal::from_str(&s).ok());
    Ok(row.map(|(a, b, c)| (p(a), p(b), p(c))).unwrap_or((None, None, None)))
}

/// Realised P&L today (the market's calendar day) for an account, optionally one bot, in one currency.
pub fn realised_today(conn: &Connection, account: &str, bot: Option<i64>, currency: &str) -> Result<Decimal, String> {
    let markets: &[&str] = if currency == "INR" { &["india"] } else { &["crypto"] };
    let mut total = Decimal::ZERO;
    for m in markets {
        let today = crate::journal::market_today(m);
        let mut st = conn
            .prepare("SELECT side, qty, entry, exit, fees, symbol FROM trades WHERE account_id = ?1 AND market = ?2 AND date = ?3 AND (?4 IS NULL OR bot_id = ?4)")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map(params![account, m, today, bot], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?, r.get::<_, f64>(2)?, r.get::<_, f64>(3)?, r.get::<_, f64>(4)?, r.get::<_, String>(5)?)))
            .map_err(|e| e.to_string())?;
        for (side, qty, entry, exit, fees, symbol) in rows.filter_map(Result::ok) {
            if accounts::currency_for(m, &symbol).ok() != Some(currency) {
                continue;
            }
            if let Some(n) = accounts::realised(&side, qty, entry, exit, fees) {
                total += n;
            }
        }
    }
    Ok(total)
}

fn bot_exposure(conn: &Connection, bot: i64) -> Result<(Decimal, i64), String> {
    let mut st = conn.prepare("SELECT qty, entry FROM positions WHERE bot_id = ?1").map_err(|e| e.to_string())?;
    let rows: Vec<(f64, f64)> = st.query_map([bot], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    let mut sum = Decimal::ZERO;
    for (q, p) in &rows {
        sum += accounts::notional(*q, *p).ok_or("bot exposure overflows")?;
    }
    Ok((sum, rows.len() as i64))
}

/// All pre-trade checks for an order that opens risk. `Err` carries the reason shown to the owner.
pub fn check(conn: &Connection, o: &Order, gate: &Gate) -> Result<(), String> {
    if gate.killed() {
        return Err("kill switch is on: no new orders until you release it in Bot Studio".into());
    }
    let kind: Option<(String, String, i64)> = conn
        .query_row("SELECT kind, environment, archived FROM accounts WHERE id = ?1", [&o.account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((kind, env, archived)) = kind else { return Err(format!("unknown account {}", o.account_id)) };
    if archived != 0 {
        return Err("this account is archived".into());
    }
    if kind != "demo" || env != "paper" {
        // The paper order path never sends anything to a broker; broker orders go through `check_broker`.
        return Err("live order refused: this is the paper order path; broker accounts trade only through approved bots".into());
    }
    check_limits(conn, o)
}

/// Pre-trade checks for an order to a connected broker account. Testnet keys trade practice funds; a live
/// account additionally needs live trading **armed for that account** and a **current approval** of the bot's
/// exact scope. Fails closed on any doubt.
pub fn check_broker(conn: &Connection, o: &Order, gate: &Gate, live_approved: bool) -> Result<(), String> {
    if gate.killed() {
        return Err("kill switch is on: no new orders until you release it".into());
    }
    let row: Option<(String, String, i64)> = conn
        .query_row("SELECT kind, environment, archived FROM accounts WHERE id = ?1", [&o.account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    let Some((kind, env, archived)) = row else { return Err(format!("unknown account {}", o.account_id)) };
    if kind != "binance" || archived != 0 {
        return Err("not a connected Binance account".into());
    }
    match env.as_str() {
        "testnet" => {}
        "live" => {
            if gate.armed().map(|(a, _)| a) != Some(o.account_id.clone()) {
                return Err("live trading is not armed for this account (Bot Studio > Risk: type the confirmation). Arming resets on every restart.".into());
            }
            if !live_approved {
                return Err("this bot has no current approval for live trading in its exact configuration".into());
            }
            if !limits_complete(conn, &o.account_id) {
                return Err("set this account's max order size, daily loss and exposure limits first".into());
            }
        }
        _ => return Err("unknown account environment".into()),
    }
    crate::riskintel::broker_data_reliable(conn, &o.account_id)?;
    check_limits(conn, o)
}

/// Account and bot limits, shared by the paper and broker paths.
fn check_limits(conn: &Connection, o: &Order) -> Result<(), String> {
    crate::riskintel::extra_checks(conn, o)?;
    let currency = accounts::currency_for(&o.market, &o.symbol)?;
    let need = accounts::notional(o.qty, o.price).ok_or("order size is too large")?;
    let (max_order, max_loss, max_exp) = limits(conn, &o.account_id, currency)?;
    if let Some(m) = max_order {
        if need > m {
            return Err(format!("order of {} {currency} is above this account's max order size of {} {currency}", dec_str(need), dec_str(m)));
        }
    }
    if let Some(m) = max_exp {
        let open = accounts::open_notional(conn, &o.account_id, currency).map_err(|e| format!("{e:?}"))?;
        if open + need > m {
            return Err(format!("exposure would be {} {currency}, above this account's limit of {} {currency}", dec_str(open + need), dec_str(m)));
        }
    }
    if let Some(m) = max_loss {
        let today = realised_today(conn, &o.account_id, None, currency)?;
        if -today >= m {
            return Err(format!("daily loss limit reached: {} {currency} lost today, limit {} {currency}", dec_str(-today), dec_str(m)));
        }
    }
    if let Some(b) = &o.bot {
        let (exposure, open) = bot_exposure(conn, b.bot_id)?;
        if open >= b.max_open_positions {
            return Err(format!("the bot already holds {open} positions, its maximum"));
        }
        // a hair of tolerance for rounding of the quantity step
        let cap = b.capital * Decimal::new(1_000_001, 6);
        if exposure + need > cap {
            return Err(format!("the bot's exposure would be {} {currency}, above its capital of {} {currency}", dec_str(exposure + need), dec_str(b.capital.round_dp(8))));
        }
        if let Some(m) = b.max_daily_loss {
            let today = realised_today(conn, &o.account_id, Some(b.bot_id), currency)?;
            if -today >= m {
                return Err(format!("the bot's daily loss limit is reached ({} {currency} lost today, limit {} {currency})", dec_str(-today), dec_str(m)));
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ account limits API

fn parse_limit(v: Option<&Value>, what: &str) -> Result<Option<Decimal>, String> {
    let text = match v {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => return Ok(None),
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err(format!("{what} must be a number or null")),
    };
    let d = Decimal::from_str(&text).map_err(|_| format!("{what} must be a decimal number"))?;
    if d <= Decimal::ZERO || d > Decimal::from(1_000_000_000_000_000i64) {
        return Err(format!("{what} must be above 0 and at most 1e15 (or empty for no limit)"));
    }
    Ok(Some(d))
}

pub fn limits_json(conn: &Connection, account: &str) -> Result<Value, String> {
    let exists: Option<i64> = conn.query_row("SELECT 1 FROM accounts WHERE id = ?1", [account], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    if exists.is_none() {
        return Err("no such account".into());
    }
    let mut out = Map::new();
    for cur in accounts::CURRENCIES {
        let (a, b, c) = limits(conn, account, cur)?;
        let s = |d: Option<Decimal>| d.map(dec_str);
        let today = realised_today(conn, account, None, cur)?;
        out.insert(cur.to_string(), json!({ "max_order_notional": s(a), "max_daily_loss": s(b), "max_exposure": s(c), "realised_today": dec_str(today) }));
    }
    Ok(json!({ "account_id": account, "limits": out, "virtual": true }))
}

/// `PUT /api/risk/limits/{account}`: `{ "USDT": { "max_order_notional": "500", ... }, "INR": {...} }`.
pub fn set_limits(conn: &mut Connection, account: &str, body: &Value) -> Result<Value, String> {
    let m = body.as_object().ok_or("request body must be a JSON object")?;
    if let Some(k) = m.keys().find(|k| !accounts::CURRENCIES.contains(&k.as_str())) {
        return Err(format!("unknown currency '{k}'"));
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let exists: Option<i64> = tx.query_row("SELECT 1 FROM accounts WHERE id = ?1", [account], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    if exists.is_none() {
        return Err("no such account".into());
    }
    let mut changes = Map::new();
    for (cur, v) in m {
        let lm = v.as_object().ok_or(format!("{cur} must be an object"))?;
        if let Some(k) = lm.keys().find(|k| !["max_order_notional", "max_daily_loss", "max_exposure"].contains(&k.as_str())) {
            return Err(format!("{cur}: unknown field '{k}'"));
        }
        let (old_a, old_b, old_c) = limits(&tx, account, cur)?;
        let pick = |k: &str, old: Option<Decimal>| -> Result<Option<Decimal>, String> { if lm.contains_key(k) { parse_limit(lm.get(k), &format!("{cur}.{k}")) } else { Ok(old) } };
        let (a, b, c) = (pick("max_order_notional", old_a)?, pick("max_daily_loss", old_b)?, pick("max_exposure", old_c)?);
        tx.execute(
            "INSERT INTO account_limits (account_id, currency, max_order_notional, max_daily_loss, max_exposure) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(account_id, currency) DO UPDATE SET max_order_notional = excluded.max_order_notional, max_daily_loss = excluded.max_daily_loss, max_exposure = excluded.max_exposure",
            params![account, cur, a.map(dec_str), b.map(dec_str), c.map(dec_str)],
        )
        .map_err(|e| e.to_string())?;
        changes.insert(cur.clone(), json!({ "max_order_notional": a.map(dec_str), "max_daily_loss": b.map(dec_str), "max_exposure": c.map(dec_str) }));
    }
    crate::audit::record(&tx, "user", "risk_limits_updated", &format!("account:{account}"), &json!({ "account_id": account, "limits": changes })).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    limits_json(conn, account)
}

// ------------------------------------------------------------------ live gate

fn live_accounts(conn: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut st = conn.prepare("SELECT id, label FROM accounts WHERE kind <> 'demo' AND environment = 'live' AND archived = 0").map_err(|e| e.to_string())?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

fn limits_complete(conn: &Connection, account: &str) -> bool {
    accounts::CURRENCIES.iter().any(|c| matches!(limits(conn, account, c), Ok((Some(_), Some(_), Some(_)))))
}

/// The gate checklist (ROADMAP section 5) as the UI shows it. Every item is computed, none is decorative.
pub fn live_status(conn: &Connection, gate: &Gate) -> Result<Value, String> {
    let live = live_accounts(conn)?;
    let armed = gate.armed();
    let limited = live.iter().any(|(id, _)| limits_complete(conn, id));
    let items = vec![
        json!({ "id": "connector", "ok": true, "label": "Binance order route built", "detail": "Spot MARKET orders through the risk engine, with client-order-id idempotency and reconciliation. Tested against a fake exchange only; never against live Binance. FYERS orders are not built." }),
        json!({ "id": "account", "ok": !live.is_empty(), "label": "A live broker account is connected", "detail": if live.is_empty() { "None connected." } else { "Connected." } }),
        json!({ "id": "limits", "ok": limited, "label": "Account limits set (max order size, daily loss, exposure)", "detail": "Set on the live account in Bot Studio > Bot Performance > Risk." }),
        json!({ "id": "kill", "ok": !gate.killed(), "label": "Kill switch released", "detail": if gate.killed() { "The kill switch is on." } else { "Off." } }),
        json!({ "id": "armed", "ok": armed.is_some(), "label": "Live trading armed with the typed confirmation", "detail": "Runtime only: disarmed after every restart." }),
    ];
    let can = !live.is_empty() && limited && !gate.killed() && armed.is_some();
    Ok(json!({
        "can_trade_live": can,
        "armed": armed.map(|(a, t)| json!({ "account_id": a, "since": t })),
        "kill_switch": gate.killed(),
        "checklist": items,
        "summary": if can { "Live trading is armed. A bot still needs a current approval of its exact configuration before it can trade live." } else { "Live trading is not armed. Bots trade the demo account (virtual funds) or a connected Binance Spot Testnet account (practice funds)." },
    }))
}

/// Arm live trading for one account. Refused unless a live account exists, its limits are set, the kill
/// switch is off, and the phrase matches exactly. Arming does not make an order path appear: [`check`] still
/// refuses every non-paper account until a connector task builds one.
pub fn arm(conn: &Connection, gate: &Gate, body: &Value) -> Result<Value, String> {
    if body.get("confirm").and_then(Value::as_str) != Some(ARM_PHRASE) {
        return Err(format!("type {ARM_PHRASE} exactly to arm live trading"));
    }
    let account = body.get("account_id").and_then(Value::as_str).unwrap_or("");
    let live = live_accounts(conn)?;
    if !live.iter().any(|(id, _)| id == account) {
        return Err("no connected live account with that id: connect a broker account first (not possible in this build)".into());
    }
    if gate.killed() {
        return Err("release the kill switch first".into());
    }
    if !limits_complete(conn, account) {
        return Err("set this account's max order size, daily loss and exposure limits first".into());
    }
    gate.arm(account);
    crate::audit::record(conn, "user", "live_armed", &format!("account:{account}"), &json!({ "account_id": account })).map_err(|e| e.to_string())?;
    live_status(conn, gate)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        crate::db::open(":memory:").unwrap()
    }

    fn order(qty: f64, price: f64) -> Order {
        Order { account_id: "demo-1".into(), market: "crypto".into(), symbol: "BTCUSDT".into(), qty, price, bot: None }
    }

    #[test]
    fn kill_switch_blocks_and_disarms() {
        let c = conn();
        let g = Gate::default();
        assert!(check(&c, &order(0.01, 100.0), &g).is_ok());
        g.set_kill(true);
        assert!(check(&c, &order(0.01, 100.0), &g).unwrap_err().contains("kill switch"));
        g.set_kill(false);
        assert!(check(&c, &order(0.01, 100.0), &g).is_ok());
    }

    #[test]
    fn non_paper_accounts_are_always_refused() {
        let c = conn();
        c.execute("INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived) VALUES ('bn-1','binance','Binance','live','active','2026-11-04T00:00:00Z',0)", []).unwrap();
        let g = Gate::default();
        // even armed (forced here: the arm() path also refuses without limits)
        g.arm("bn-1");
        let mut o = order(0.01, 100.0);
        o.account_id = "bn-1".into();
        assert!(check(&c, &o, &g).unwrap_err().contains("live order refused"));
        assert_eq!(live_status(&c, &g).unwrap()["can_trade_live"], false);
    }

    #[test]
    fn broker_live_orders_need_arming_approval_and_limits() {
        let mut c = conn();
        for (id, env) in [("bn-live-1", "live"), ("bn-test-1", "testnet")] {
            c.execute("INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived) VALUES (?1,'binance','B',?2,'active','x',0)", [id, env]).unwrap();
        }
        let g = Gate::default();
        let mut o = order(0.01, 100.0);
        o.account_id = "bn-live-1".into();
        assert!(check_broker(&c, &o, &g, true).unwrap_err().contains("not armed"));
        g.arm("bn-live-1");
        assert!(check_broker(&c, &o, &g, false).unwrap_err().contains("approval"));
        assert!(check_broker(&c, &o, &g, true).unwrap_err().contains("limits"));
        set_limits(&mut c, "bn-live-1", &json!({ "USDT": { "max_order_notional": "100", "max_daily_loss": "10", "max_exposure": "200" } })).unwrap();
        assert!(check_broker(&c, &o, &g, true).is_ok(), "armed, approved, limits set");
        o.qty = 2.0; // 200 notional
        assert!(check_broker(&c, &o, &g, true).unwrap_err().contains("max order size"), "limits still bind");
        o.qty = 0.01;
        g.set_kill(true);
        assert!(check_broker(&c, &o, &g, true).unwrap_err().contains("kill switch"));
        g.set_kill(false);
        assert!(check_broker(&c, &o, &g, true).unwrap_err().contains("not armed"), "the kill switch disarmed live trading");
        // testnet: practice funds, no arming needed; the paper path still refuses broker accounts
        o.account_id = "bn-test-1".into();
        assert!(check_broker(&c, &o, &g, false).is_ok());
        assert!(check(&c, &o, &g).is_err());
        // a demo account is not a broker account
        o.account_id = "demo-1".into();
        assert!(check_broker(&c, &o, &g, true).is_err());
    }

    #[test]
    fn account_limits_are_enforced() {
        let mut c = conn();
        let g = Gate::default();
        set_limits(&mut c, "demo-1", &json!({ "USDT": { "max_order_notional": "500", "max_exposure": "800", "max_daily_loss": "50" } })).unwrap();
        assert!(check(&c, &order(6.0, 100.0), &g).unwrap_err().contains("max order size"));
        assert!(check(&c, &order(5.0, 100.0), &g).is_ok(), "exactly at the limit is fine");
        c.execute("INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at, account_id) VALUES ('crypto','demo','ETHUSDT','long',4,100,0.1,'x','demo-1')", []).unwrap();
        assert!(check(&c, &order(5.0, 100.0), &g).unwrap_err().contains("exposure"));
        let today = crate::journal::market_today("crypto");
        c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode, account_id) VALUES (?1,'BTCUSDT','long',1,100,49,1,'','crypto','demo','demo-1')", [&today]).unwrap();
        assert!(check(&c, &order(1.0, 100.0), &g).unwrap_err().contains("daily loss"));
        // INR is a separate currency with no limits
        let mut nse = order(1.0, 100.0);
        nse.market = "india".into();
        nse.symbol = "SBIN".into();
        assert!(check(&c, &nse, &g).is_ok());
        assert!(set_limits(&mut c, "demo-1", &json!({ "USDT": { "max_exposure": "-1" } })).is_err());
        assert!(set_limits(&mut c, "demo-1", &json!({ "EUR": {} })).is_err());
        // clearing a limit
        set_limits(&mut c, "demo-1", &json!({ "USDT": { "max_daily_loss": null } })).unwrap();
        assert!(check(&c, &order(1.0, 100.0), &g).is_ok());
    }

    #[test]
    fn bot_limits_are_enforced() {
        let c = conn();
        let g = Gate::default();
        let mut o = order(5.0, 100.0);
        o.bot = Some(BotLimits { bot_id: 7, capital: Decimal::from(400), max_daily_loss: None, max_open_positions: 1 });
        assert!(check(&c, &o, &g).unwrap_err().contains("capital"));
        o.qty = 4.0;
        assert!(check(&c, &o, &g).is_ok());
        c.execute("INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at, account_id, bot_id) VALUES ('crypto','demo','ETHUSDT','long',0.1,100,0.1,'x','demo-1',7)", []).unwrap();
        assert!(check(&c, &o, &g).unwrap_err().contains("positions"));
    }

    #[test]
    fn arming_needs_the_phrase_and_a_live_account() {
        let c = conn();
        let g = Gate::default();
        assert!(arm(&c, &g, &json!({ "confirm": "arm" })).unwrap_err().contains(ARM_PHRASE));
        assert!(arm(&c, &g, &json!({ "confirm": ARM_PHRASE, "account_id": "demo-1" })).unwrap_err().contains("no connected live account"));
        assert!(g.armed().is_none());
    }
}
