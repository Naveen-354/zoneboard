//! Bots: configuration, lifecycle, the database-backed [`Book`] the engine trades through, the runner's
//! per-bot step, and performance computed from the bot's own recorded fills.
//!
//! A bot trades only a **paper** (demo) account. Its positions and trades are rows in the account's
//! `positions` / `trades` tables tagged with `bot_id`, so they draw on the same virtual funds as manual orders
//! and appear in the Journal. Performance figures are computed from those rows; nothing is estimated.
//!
//! Lifecycle: `stopped` -> `running` <-> `paused` -> `stopped`. A paused bot opens nothing new but still
//! manages its open positions (stops, targets, exit signals). Stopping closes the bot's positions at the
//! latest price. A restart of the app pauses every running bot: nothing trades until the owner resumes it.

use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::accounts::dec_str;
use crate::botengine::{self, Book, Engine, Exec, Legs, Lot, OpenReq, State};
use crate::orders::{self, BotRef};
use crate::risk::Gate;
use crate::strategy::{self, Kind, Side};
use crate::zones::Frame;

pub const CRYPTO_INTERVALS: [&str; 5] = ["5m", "15m", "1h", "4h", "1d"];
pub const INDIA_INTERVALS: [&str; 3] = ["15m", "1h", "1d"];
/// The bots in the Explore catalog a bot can say it was inspired by (`source`).
pub const CATALOG_IDS: [&str; 7] = ["freqtrade", "hummingbot", "jesse", "superalgos", "octobot", "gunbot", "haasonline"];
const MAX_BOTS: i64 = 50;
const MAX_LOGS_PER_BOT: i64 = 2000;
/// More new closed candles than this at once (the app was asleep, the feed was down) are skipped, not
/// replayed: a forward test must trade at prices that were current, not catch up on history.
const MAX_CATCH_UP: usize = 3;

pub fn interval_secs(iv: &str) -> Option<i64> {
    Some(match iv {
        "5m" => 300,
        "15m" => 900,
        "1h" => 3600,
        "4h" => 14_400,
        "1d" => 86_400,
        _ => return None,
    })
}

pub fn qty_step(market: &str) -> f64 {
    if market == "india" {
        1.0
    } else {
        0.000001
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Clone, Debug)]
pub struct Bot {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub avatar: Value,
    pub market: String,
    pub exchange: String,
    pub mode: String,
    pub account_id: String,
    pub symbol: String,
    pub symbol2: Option<String>,
    pub interval: String,
    pub spec: Value,
    pub template: Option<String>,
    pub source: Option<String>,
    pub allocation: Decimal,
    pub max_daily_loss: Option<Decimal>,
    pub max_open_positions: i64,
    pub slippage_pct: f64,
    pub status: String,
    pub state: RunnerState,
    pub last_price: Option<f64>,
    pub last_price_utc: Option<String>,
    pub last_error: Option<String>,
    pub created_utc: String,
    pub updated_utc: String,
    pub started_utc: Option<String>,
}

/// What the runner keeps between ticks: the engine's state and the latest price of each leg.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunnerState {
    #[serde(default)]
    pub engine: State,
    /// Latest price per leg; `None` when a leg has no data (NaN would not survive JSON).
    #[serde(default)]
    pub prices: Vec<Option<f64>>,
}

const COLS: &str = "id, name, description, avatar, market, exchange, mode, account_id, symbol, symbol2, interval, spec, template, source, allocation, max_daily_loss, max_open_positions, slippage_pct, status, state, last_price, last_price_utc, last_error, created_utc, updated_utc, started_utc";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Bot> {
    let j = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
    let d = |s: String| Decimal::from_str(&s).unwrap_or(Decimal::ZERO);
    Ok(Bot {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        avatar: j(r.get(3)?),
        market: r.get(4)?,
        exchange: r.get(5)?,
        mode: r.get(6)?,
        account_id: r.get(7)?,
        symbol: r.get(8)?,
        symbol2: r.get(9)?,
        interval: r.get(10)?,
        spec: j(r.get(11)?),
        template: r.get(12)?,
        source: r.get(13)?,
        allocation: d(r.get(14)?),
        max_daily_loss: r.get::<_, Option<String>>(15)?.map(d),
        max_open_positions: r.get(16)?,
        slippage_pct: r.get(17)?,
        status: r.get(18)?,
        state: {
            let raw: String = r.get(19)?;
            serde_json::from_str(&raw).unwrap_or_else(|e| {
                if raw != "{}" {
                    tracing::warn!(bot = r.get::<_, i64>(0).unwrap_or(0), "bot state unreadable, starting clean: {e}");
                }
                RunnerState::default()
            })
        },
        last_price: r.get(20)?,
        last_price_utc: r.get(21)?,
        last_error: r.get(22)?,
        created_utc: r.get(23)?,
        updated_utc: r.get(24)?,
        started_utc: r.get(25)?,
    })
}

#[derive(Debug, PartialEq)]
pub enum BotError {
    NotFound,
    Invalid(String),
    Conflict(String),
    Db(String),
}

impl From<rusqlite::Error> for BotError {
    fn from(e: rusqlite::Error) -> Self {
        BotError::Db(e.to_string())
    }
}

fn bad<T>(m: impl Into<String>) -> Result<T, BotError> {
    Err(BotError::Invalid(m.into()))
}

pub fn load(conn: &Connection, id: i64) -> Result<Bot, BotError> {
    conn.query_row(&format!("SELECT {COLS} FROM bots WHERE id = ?1"), [id], from_row).optional()?.ok_or(BotError::NotFound)
}

pub fn ids_with_status(conn: &Connection, statuses: &[&str]) -> Vec<i64> {
    let mut st = match conn.prepare("SELECT id, status FROM bots ORDER BY id") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| rows.filter_map(Result::ok).filter(|(_, s)| statuses.contains(&s.as_str())).map(|(i, _)| i).collect())
        .unwrap_or_default()
}

pub fn log(conn: &Connection, bot: i64, level: &str, msg: &str) {
    let msg: String = msg.chars().take(1000).collect();
    let _ = conn.execute("INSERT INTO bot_logs (bot_id, ts_utc, level, message) VALUES (?1,?2,?3,?4)", params![bot, now(), level, msg]);
    // keep the newest entries only
    let _ = conn.execute(
        "DELETE FROM bot_logs WHERE bot_id = ?1 AND id <= (SELECT id FROM bot_logs WHERE bot_id = ?1 ORDER BY id DESC LIMIT 1 OFFSET ?2)",
        params![bot, MAX_LOGS_PER_BOT],
    );
}

fn realised_total(conn: &Connection, bot: i64) -> Result<Decimal, BotError> {
    let mut st = conn.prepare("SELECT side, qty, entry, exit, fees FROM trades WHERE bot_id = ?1")?;
    let rows: Vec<(String, f64, f64, f64, f64)> = st.query_map([bot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.filter_map(Result::ok).collect();
    Ok(rows.iter().filter_map(|(s, q, e, x, f)| crate::accounts::realised(s, *q, *e, *x, *f)).sum())
}

// ------------------------------------------------------------------ validation

struct Input {
    name: String,
    description: String,
    avatar: Value,
    market: String,
    exchange: String,
    account_id: String,
    symbol: String,
    symbol2: Option<String>,
    interval: String,
    spec: Value,
    template: Option<String>,
    source: Option<String>,
    allocation: Decimal,
    max_daily_loss: Option<Decimal>,
    max_open_positions: i64,
    slippage_pct: f64,
}

fn text(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).map(|s| s.trim().to_string())
}

fn decimal(v: Option<&Value>, what: &str, required: bool) -> Result<Option<Decimal>, BotError> {
    let t = match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => return bad(format!("{what} must be a number")),
    };
    let Some(t) = t else {
        return if required { bad(format!("{what} is required")) } else { Ok(None) };
    };
    let d = Decimal::from_str(&t).map_err(|_| BotError::Invalid(format!("{what} must be a decimal number")))?;
    if d <= Decimal::ZERO || d > Decimal::from(1_000_000_000_000_000i64) {
        return bad(format!("{what} must be above 0 and at most 1e15"));
    }
    if d.normalize().scale() > 8 {
        return bad(format!("{what} allows at most 8 decimal places"));
    }
    Ok(Some(d))
}

fn valid_symbol(market: &str, s: &str) -> bool {
    if !crate::data::valid_nse_symbol(s) {
        return false;
    }
    match market {
        "crypto" => s.len() > 4 && s.ends_with("USDT") && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
        _ => true,
    }
}

fn parse_input(conn: &Connection, body: &Value, base: Option<&Bot>) -> Result<Input, BotError> {
    let m = body.as_object().ok_or_else(|| BotError::Invalid("request body must be a JSON object".into()))?;
    const KEYS: [&str; 17] = ["name", "description", "avatar", "market", "exchange", "mode", "account_id", "symbol", "symbol2", "interval", "spec", "template", "source", "allocation", "max_daily_loss", "max_open_positions", "slippage_pct"];
    if let Some(k) = m.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return bad(format!("unknown field '{k}'"));
    }
    let pick = |k: &str| -> Option<Value> { m.get(k).cloned().or_else(|| base.map(|b| bot_field(b, k))) };
    let pv = |k: &str| pick(k).unwrap_or(Value::Null);
    let pm: Map<String, Value> = KEYS.iter().map(|k| (k.to_string(), pv(k))).collect();

    let name = text(&pm, "name").unwrap_or_default();
    if name.is_empty() || name.chars().count() > 60 {
        return bad("name must be 1 to 60 characters");
    }
    let description = text(&pm, "description").unwrap_or_default();
    if description.chars().count() > 500 {
        return bad("description is at most 500 characters");
    }
    let avatar = match pm.get("avatar") {
        Some(Value::Object(a)) => {
            let glyph = a.get("glyph").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let color = a.get("color").and_then(Value::as_str).unwrap_or("#4f46e5").to_string();
            if glyph.chars().count() > 2 {
                return bad("avatar.glyph is 1 or 2 characters");
            }
            let hex = color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !hex {
                return bad("avatar.color must be a colour like #4f46e5");
            }
            json!({ "glyph": glyph, "color": color })
        }
        _ => json!({ "glyph": "", "color": "#4f46e5" }),
    };
    let market = text(&pm, "market").unwrap_or_else(|| "crypto".into());
    if !["crypto", "india"].contains(&market.as_str()) {
        return bad("market must be crypto or india");
    }
    let exchange = text(&pm, "exchange").unwrap_or_else(|| if market == "crypto" { "binance".into() } else { "fyers".into() });
    match (market.as_str(), exchange.as_str()) {
        ("crypto", "binance") | ("india", "fyers") => {}
        _ => return bad("crypto bots use Binance market data; NSE bots use FYERS (data via Yahoo, 15 min delayed, until the FYERS feed lands)"),
    }
    let mode = text(&pm, "mode").unwrap_or_else(|| "demo".into());
    match mode.as_str() {
        "demo" => {}
        "live" => {
            return Err(BotError::Conflict(
                "Live bots need a connected live broker account, and this build has none: the Binance and FYERS order connectors are not built yet. Create the bot in Demo mode; Bot Performance shows what live deployment still requires.".into(),
            ))
        }
        _ => return bad("mode must be demo or live"),
    }
    let account_id = text(&pm, "account_id").filter(|s| !s.is_empty()).unwrap_or_else(|| crate::accounts::DEFAULT_ACCOUNT.into());
    crate::accounts::require_demo(conn, &account_id).map_err(BotError::Invalid)?;
    let symbol = text(&pm, "symbol").unwrap_or_default().to_uppercase();
    if !valid_symbol(&market, &symbol) {
        return bad(if market == "crypto" { "symbol must be a USDT pair like BTCUSDT" } else { "symbol must be an NSE symbol like SBIN" });
    }
    let interval = text(&pm, "interval").unwrap_or_else(|| "1h".into());
    let allowed: &[&str] = if market == "crypto" { &CRYPTO_INTERVALS } else { &INDIA_INTERVALS };
    if !allowed.contains(&interval.as_str()) {
        return bad(format!("timeframe must be one of {}", allowed.join(", ")));
    }
    let spec = pm.get("spec").cloned().unwrap_or(Value::Null);
    let parsed = strategy::parse(&spec).map_err(|e| BotError::Invalid(format!("strategy: {e}")))?;
    let symbol2 = text(&pm, "symbol2").filter(|s| !s.is_empty()).map(|s| s.to_uppercase());
    match (parsed.kind, &symbol2) {
        (Kind::Pairs, None) => return bad("a pairs strategy needs a second symbol"),
        (Kind::Pairs, Some(s2)) if !valid_symbol(&market, s2) || *s2 == symbol => return bad("the second symbol must be a different, valid symbol of the same market"),
        (Kind::Pairs, _) => {}
        (_, Some(_)) => return bad("only pairs strategies take a second symbol"),
        (_, None) => {}
    }
    let template = text(&pm, "template").filter(|s| !s.is_empty());
    if let Some(t) = &template {
        if !strategy::templates().iter().any(|x| x["id"] == t.as_str()) {
            return bad(format!("unknown template '{t}'"));
        }
    }
    let source = text(&pm, "source").filter(|s| !s.is_empty());
    if let Some(s) = &source {
        if !CATALOG_IDS.contains(&s.as_str()) {
            return bad(format!("unknown catalog bot '{s}'"));
        }
    }
    let allocation = decimal(pm.get("allocation"), "allocation", true)?.unwrap();
    let max_daily_loss = decimal(pm.get("max_daily_loss"), "max_daily_loss", false)?;
    let max_open_positions = match pm.get("max_open_positions") {
        None | Some(Value::Null) => 10,
        Some(v) => match v.as_i64() {
            Some(n) if (1..=100).contains(&n) => n,
            _ => return bad("max_open_positions must be a whole number from 1 to 100"),
        },
    };
    if parsed.kind == Kind::Grid {
        let levels = parsed.grid.as_ref().map_or(0, |g| g.levels) as i64;
        if max_open_positions < levels - 1 {
            return bad(format!("a grid with {levels} lines can hold {} lots: raise max open positions to at least that", levels - 1));
        }
    }
    let slippage_pct = match pm.get("slippage_pct") {
        None | Some(Value::Null) => 0.05,
        Some(v) => match v.as_f64() {
            Some(x) if (0.0..=2.0).contains(&x) => x,
            _ => return bad("slippage_pct must be between 0 and 2"),
        },
    };
    Ok(Input { name, description, avatar, market, exchange, account_id, symbol, symbol2, interval, spec, template, source, allocation, max_daily_loss, max_open_positions, slippage_pct })
}

/// A field of an existing bot as the request would spell it (for partial updates).
fn bot_field(b: &Bot, k: &str) -> Value {
    match k {
        "name" => json!(b.name),
        "description" => json!(b.description),
        "avatar" => b.avatar.clone(),
        "market" => json!(b.market),
        "exchange" => json!(b.exchange),
        "mode" => json!(b.mode),
        "account_id" => json!(b.account_id),
        "symbol" => json!(b.symbol),
        "symbol2" => json!(b.symbol2),
        "interval" => json!(b.interval),
        "spec" => b.spec.clone(),
        "template" => json!(b.template),
        "source" => json!(b.source),
        "allocation" => json!(dec_str(b.allocation)),
        "max_daily_loss" => json!(b.max_daily_loss.map(dec_str)),
        "max_open_positions" => json!(b.max_open_positions),
        "slippage_pct" => json!(b.slippage_pct),
        _ => Value::Null,
    }
}

// ------------------------------------------------------------------ CRUD

pub fn create(conn: &mut Connection, body: &Value) -> Result<i64, BotError> {
    let i = parse_input(conn, body, None)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let n: i64 = tx.query_row("SELECT count(*) FROM bots", [], |r| r.get(0))?;
    if n >= MAX_BOTS {
        return bad(format!("at most {MAX_BOTS} bots"));
    }
    let t = now();
    tx.execute(
        &format!("INSERT INTO bots ({}) VALUES (?1,?2,?3,?4,?5,'demo',?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,'stopped','{{}}',NULL,NULL,NULL,?17,?17,NULL)", &COLS[4..]),
        params![
            i.name, i.description, i.avatar.to_string(), i.market, i.exchange, i.account_id, i.symbol, i.symbol2, i.interval, i.spec.to_string(), i.template, i.source,
            dec_str(i.allocation), i.max_daily_loss.map(dec_str), i.max_open_positions, i.slippage_pct, t
        ],
    )?;
    let id = tx.last_insert_rowid();
    crate::audit::record(&tx, "user", "bot_created", &format!("bot:{id}"), &json!({ "bot_id": id, "name": i.name, "market": i.market, "symbol": i.symbol, "interval": i.interval, "allocation": dec_str(i.allocation), "template": i.template, "source": i.source, "mode": "demo" }))?;
    tx.commit()?;
    log(conn, id, "info", "bot created (stopped)");
    Ok(id)
}

pub fn update(conn: &mut Connection, id: i64, body: &Value) -> Result<(), BotError> {
    let b = load(conn, id)?;
    if b.status == "running" {
        return Err(BotError::Conflict("pause or stop the bot before editing it".into()));
    }
    let i = parse_input(conn, body, Some(&b))?;
    let open: i64 = conn.query_row("SELECT count(*) FROM positions WHERE bot_id = ?1", [id], |r| r.get(0))?;
    let structural = i.market != b.market || i.symbol != b.symbol || i.symbol2 != b.symbol2 || i.account_id != b.account_id || i.interval != b.interval || i.spec != b.spec;
    if open > 0 && structural {
        return Err(BotError::Conflict("the bot holds open positions: stop it (which closes them) before changing its market, symbols, account, timeframe or strategy".into()));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "UPDATE bots SET name=?1, description=?2, avatar=?3, market=?4, exchange=?5, account_id=?6, symbol=?7, symbol2=?8, interval=?9, spec=?10, template=?11, source=?12,
         allocation=?13, max_daily_loss=?14, max_open_positions=?15, slippage_pct=?16, updated_utc=?17 WHERE id=?18",
        params![
            i.name, i.description, i.avatar.to_string(), i.market, i.exchange, i.account_id, i.symbol, i.symbol2, i.interval, i.spec.to_string(), i.template, i.source,
            dec_str(i.allocation), i.max_daily_loss.map(dec_str), i.max_open_positions, i.slippage_pct, now(), id
        ],
    )?;
    if structural {
        // a new strategy or market starts from a clean engine state
        tx.execute("UPDATE bots SET state = '{}' WHERE id = ?1", [id])?;
    }
    crate::audit::record(&tx, "user", "bot_updated", &format!("bot:{id}"), &json!({ "bot_id": id, "name": i.name, "allocation": dec_str(i.allocation), "strategy_changed": i.spec != b.spec }))?;
    tx.commit()?;
    log(conn, id, "info", "configuration updated");
    Ok(())
}

pub fn duplicate(conn: &mut Connection, id: i64) -> Result<i64, BotError> {
    let b = load(conn, id)?;
    let mut body = Map::new();
    for k in ["description", "avatar", "market", "exchange", "account_id", "symbol", "symbol2", "interval", "spec", "template", "source", "allocation", "max_daily_loss", "max_open_positions", "slippage_pct"] {
        body.insert(k.into(), bot_field(&b, k));
    }
    let name: String = format!("{} (copy)", b.name).chars().take(60).collect();
    body.insert("name".into(), json!(name));
    create(conn, &Value::Object(body))
}

pub fn delete(conn: &mut Connection, id: i64) -> Result<(), BotError> {
    let b = load(conn, id)?;
    if b.status != "stopped" {
        return Err(BotError::Conflict("stop the bot before deleting it".into()));
    }
    let open: i64 = conn.query_row("SELECT count(*) FROM positions WHERE bot_id = ?1", [id], |r| r.get(0))?;
    if open > 0 {
        return Err(BotError::Conflict("the bot still holds positions; close them first".into()));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM bot_logs WHERE bot_id = ?1", [id])?;
    tx.execute("DELETE FROM bot_orders WHERE bot_id = ?1", [id])?;
    tx.execute("DELETE FROM backtests WHERE bot_id = ?1", [id])?;
    tx.execute("DELETE FROM bots WHERE id = ?1", [id])?;
    // The bot's trades stay in the Journal (they are the account's history), still tagged with its id.
    crate::audit::record(&tx, "user", "bot_deleted", &format!("bot:{id}"), &json!({ "bot_id": id, "name": b.name }))?;
    tx.commit()?;
    Ok(())
}

pub fn set_status(conn: &Connection, id: i64, status: &str, reason: &str, actor: &str) -> Result<(), BotError> {
    let b = load(conn, id)?;
    if b.status == status {
        return Ok(());
    }
    let started = if status == "running" { Some(now()) } else { b.started_utc.clone() };
    conn.execute("UPDATE bots SET status = ?1, started_utc = ?2, updated_utc = ?3 WHERE id = ?4", params![status, started, now(), id])?;
    crate::audit::record(conn, actor, &format!("bot_{status}"), &format!("bot:{id}"), &json!({ "bot_id": id, "from": b.status, "to": status, "reason": reason }))?;
    log(conn, id, "info", &format!("{} -> {status}: {reason}", b.status));
    Ok(())
}

/// `running` from `stopped` or `paused`. The engine re-baselines on the next tick (it acts from the next
/// candle close), keeping grid lines, the DCA cycle and open positions.
pub fn start(conn: &Connection, id: i64, gate: &Gate, sample: bool) -> Result<(), BotError> {
    let b = load(conn, id)?;
    if sample {
        return Err(BotError::Conflict("sample data is on (made-up prices), so bots cannot trade. Backtests still work.".into()));
    }
    if gate.killed() {
        return Err(BotError::Conflict("the kill switch is on: release it first".into()));
    }
    if b.mode != "demo" {
        return Err(BotError::Conflict("only demo bots can run in this build".into()));
    }
    crate::accounts::require_demo(conn, &b.account_id).map_err(BotError::Conflict)?;
    strategy::parse(&b.spec).map_err(|e| BotError::Invalid(format!("strategy: {e}")))?;
    let mut st = b.state.clone();
    st.engine.pending.clear();
    st.engine.last_closed = None;
    st.engine.last_opened = None;
    if b.status == "stopped" {
        st.engine = State::default();
    }
    conn.execute("UPDATE bots SET state = ?1, last_error = NULL WHERE id = ?2", params![serde_json::to_string(&st).unwrap(), id])?;
    set_status(conn, id, "running", if b.status == "paused" { "resumed" } else { "started" }, "user")
}

/// Close every position of a bot at the given prices (per leg), then mark it stopped. Positions without a
/// price are left open and reported, never closed at a guessed price.
pub fn stop(conn: &mut Connection, id: i64, prices: [Option<f64>; 2], reason: &str, actor: &str) -> Result<Value, BotError> {
    let b = load(conn, id)?;
    let r = bot_ref(conn, &b)?;
    let slip = b.slippage_pct / 100.0;
    let lots = DbBook::lots_of(conn, id)?;
    let (mut closed, mut left) = (0, 0);
    for l in lots {
        match prices[l.leg as usize] {
            Some(p) if p > 0.0 => {
                let px = if l.side == Side::Long { p * (1.0 - slip) } else { p * (1.0 + slip) };
                match orders::close(conn, &r, l.id, px, chrono::Utc::now().timestamp(), reason) {
                    Ok(_) => closed += 1,
                    Err(e) => {
                        left += 1;
                        log(conn, id, "error", &format!("could not close position {}: {e}", l.id));
                    }
                }
            }
            _ => left += 1,
        }
    }
    let mut st = b.state.clone();
    st.engine = State::default();
    conn.execute("UPDATE bots SET state = ?1 WHERE id = ?2", params![serde_json::to_string(&st).unwrap(), id])?;
    set_status(conn, id, "stopped", reason, actor)?;
    if left > 0 {
        log(conn, id, "warn", &format!("{left} position(s) left open: no current price. Close them from the Board."));
    }
    Ok(json!({ "closed": closed, "left_open": left }))
}

/// On app start: every running bot is paused (gate: nothing trades after a restart until the owner says so).
pub fn pause_all_on_start(conn: &Connection) {
    for id in ids_with_status(conn, &["running"]) {
        let _ = set_status(conn, id, "paused", "app restarted: bots never resume on their own", "system");
    }
}

fn bot_ref<'a>(conn: &Connection, b: &'a Bot) -> Result<BotRef<'a>, BotError> {
    Ok(BotRef {
        id: b.id,
        name: &b.name,
        account_id: &b.account_id,
        market: &b.market,
        capital: b.allocation + realised_total(conn, b.id)?,
        max_daily_loss: b.max_daily_loss,
        max_open_positions: b.max_open_positions,
    })
}

// ------------------------------------------------------------------ the database book

pub struct DbBook<'a> {
    conn: &'a mut Connection,
    bot: &'a Bot,
    gate: &'a Gate,
    symbols: [String; 2],
    pub events: usize,
}

impl<'a> DbBook<'a> {
    pub fn new(conn: &'a mut Connection, bot: &'a Bot, gate: &'a Gate) -> Self {
        let symbols = [bot.symbol.clone(), bot.symbol2.clone().unwrap_or_default()];
        Self { conn, bot, gate, symbols, events: 0 }
    }

    fn lots_of(conn: &Connection, bot: i64) -> Result<Vec<Lot>, BotError> {
        let mut st = conn.prepare("SELECT id, leg, side, qty, entry, stop, target, trail, trail_ref, bar_time, tag FROM positions WHERE bot_id = ?1 ORDER BY id")?;
        let rows = st.query_map([bot], |r| {
            let side: String = r.get(2)?;
            Ok(Lot {
                id: r.get(0)?,
                leg: r.get::<_, Option<i64>>(1)?.unwrap_or(0).clamp(0, 1) as u8,
                side: if side == "short" { Side::Short } else { Side::Long },
                qty: r.get(3)?,
                entry: r.get(4)?,
                stop: r.get(5)?,
                target: r.get(6)?,
                trail: orders::parse_trail(r.get(7)?),
                trail_ref: r.get(8)?,
                opened: r.get::<_, Option<i64>>(9)?.unwrap_or(0),
                tag: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }
}

impl Book for DbBook<'_> {
    fn lots(&self) -> Vec<Lot> {
        Self::lots_of(self.conn, self.bot.id).unwrap_or_default()
    }
    fn capital(&self) -> f64 {
        (self.bot.allocation + realised_total(self.conn, self.bot.id).unwrap_or(Decimal::ZERO)).to_f64().unwrap_or(0.0)
    }
    fn exposure(&self) -> f64 {
        self.lots().iter().map(|l| l.qty * l.entry).sum()
    }
    fn open(&mut self, req: OpenReq) -> Result<i64, String> {
        let r = bot_ref(self.conn, self.bot).map_err(|e| format!("{e:?}"))?;
        let symbol = self.symbols[req.leg as usize].clone();
        let res = orders::open(self.conn, &r, &symbol, &req, self.gate);
        self.events += 1;
        match &res {
            Ok(_) => log(self.conn, self.bot.id, "trade", &format!("opened {} {} {} at {} ({})", req.side.as_str(), req.qty, symbol, req.price, req.tag)),
            Err(e) => log(self.conn, self.bot.id, "warn", &format!("order refused: {} {} {}: {e}", req.side.as_str(), req.qty, symbol)),
        }
        res
    }
    fn close(&mut self, lot: i64, price: f64, time: i64, reason: &str) -> Result<(), String> {
        let r = bot_ref(self.conn, self.bot).map_err(|e| format!("{e:?}"))?;
        let res = orders::close(self.conn, &r, lot, price, time, reason).map(|_| ());
        self.events += 1;
        match &res {
            Ok(()) => log(self.conn, self.bot.id, "trade", &format!("closed position {lot} at {price} ({reason})")),
            Err(e) => log(self.conn, self.bot.id, "error", &format!("close of position {lot} failed: {e}")),
        }
        res
    }
    fn update_trail(&mut self, lot: i64, trail_ref: f64) {
        let _ = self.conn.execute("UPDATE positions SET trail_ref = ?1 WHERE id = ?2 AND bot_id = ?3", params![trail_ref, lot, self.bot.id]);
    }
    fn log(&mut self, level: &str, msg: String) {
        log(self.conn, self.bot.id, level, &msg);
    }
}

// ------------------------------------------------------------------ the runner's step

/// Process one bot against fresh bars. `a` (and `b` for pairs) may end with a still-forming bar: only bars
/// with `t + interval <= now` count as closed. Signals use closed bars; pending orders fill at the open of
/// the first bar after the signal, as soon as that bar exists. Returns a short status line.
pub fn process(conn: &mut Connection, gate: &Gate, id: i64, a: &Frame, b: Option<&Frame>, now: i64) -> Result<String, String> {
    let bot = load(conn, id).map_err(|e| format!("{e:?}"))?;
    if !["running", "paused"].contains(&bot.status.as_str()) {
        return Ok("not running".into());
    }
    let strat = strategy::parse(&bot.spec)?;
    let iv = interval_secs(&bot.interval).ok_or("bad interval")?;
    let aligned;
    let (fa, fb) = match b {
        Some(b) => {
            aligned = botengine::align(a, b);
            (&aligned.0, Some(&aligned.1))
        }
        None => (a, None),
    };
    if fa.is_empty() {
        return Err("no bars".into());
    }
    let closed_len = fa.t.iter().take_while(|&&t| t + iv <= now).count();
    let mut rs = bot.state.clone();
    let fin = |x: f64| x.is_finite().then_some(x);
    rs.prices = vec![fin(fa.c[fa.len() - 1]), fb.and_then(|f| fin(f.c[f.len() - 1]))];
    let save = |conn: &Connection, rs: &RunnerState, err: Option<&str>| {
        let _ = conn.execute(
            "UPDATE bots SET state = ?1, last_price = ?2, last_price_utc = ?3, last_error = ?4 WHERE id = ?5",
            params![serde_json::to_string(rs).unwrap(), rs.prices.first().copied().flatten(), now_iso(), err, id],
        );
    };
    if closed_len < 2 {
        save(conn, &rs, None);
        return Ok("waiting for closed candles".into());
    }
    let last_closed_t = fa.t[closed_len - 1];
    let Some(prev) = rs.engine.last_closed else {
        rs.engine.last_closed = Some(last_closed_t);
        rs.engine.last_opened = Some(last_closed_t);
        save(conn, &rs, None);
        log(conn, id, "info", &format!("watching {} {}: acts from the next candle close", bot.symbol, bot.interval));
        return Ok("baseline set".into());
    };
    let first_new = fa.t.partition_point(|&t| t <= prev);
    let new_closed = closed_len.saturating_sub(first_new);
    if new_closed > MAX_CATCH_UP {
        let dropped = rs.engine.pending.len();
        rs.engine.pending.clear();
        rs.engine.last_closed = Some(last_closed_t);
        rs.engine.last_opened = Some(last_closed_t);
        save(conn, &rs, None);
        log(conn, id, "warn", &format!("{new_closed} candles closed since the last check (app asleep or no data): skipped, not replayed{}", if dropped > 0 { format!("; {dropped} pending order(s) dropped") } else { String::new() }));
        return Ok("skipped a gap".into());
    }
    let closed_a = crate::backtest2::slice(fa, 0, closed_len);
    let closed_b = fb.map(|f| crate::backtest2::slice(f, 0, closed_len));
    let prep = botengine::prepare(&strat, &Legs { a: &closed_a, b: closed_b.as_ref() })?;
    let legs = Legs { a: fa, b: fb };
    let engine = Engine { s: &strat, exec: Exec { fee_pct: crate::journal::fee_pct(&bot.market), slippage_pct: bot.slippage_pct, qty_step: [qty_step(&bot.market); 2] } };
    let trade = bot.status == "running" && !gate.killed();
    let mut st = rs.engine.clone();
    let events;
    {
        let mut book = DbBook::new(conn, &bot, gate);
        // Bar by bar from the first new one. A bar's open fills what the previous close decided (as soon as the
        // bar exists, even still forming); its close is processed only once it has closed.
        for j in first_new..fa.len() {
            if st.last_opened.is_none_or(|t| fa.t[j] > t) && !st.pending.is_empty() {
                engine.on_open(&mut st, &mut book, &legs, j);
            } else if st.last_opened.is_none_or(|t| fa.t[j] > t) {
                st.last_opened = Some(fa.t[j]);
            }
            if j < closed_len {
                engine.on_close(&mut st, &mut book, &legs, &prep, j, trade);
            } else {
                break;
            }
        }
        events = book.events;
    }
    rs.engine = st;
    save(conn, &rs, None);
    Ok(format!("{new_closed} new candle(s), {events} order event(s)"))
}

fn now_iso() -> String {
    now()
}

// ------------------------------------------------------------------ performance

fn r2(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

/// Performance from the bot's recorded trades, plus open positions marked at the latest price.
pub fn performance(conn: &Connection, b: &Bot) -> Result<Value, BotError> {
    let mut st = conn.prepare("SELECT t.id, t.side, t.qty, t.entry, t.exit, t.fees, t.symbol, t.date, t.notes, o.ts_utc FROM trades t LEFT JOIN bot_orders o ON o.trade_id = t.id WHERE t.bot_id = ?1 ORDER BY t.id")?;
    let rows: Vec<(i64, String, f64, f64, f64, f64, String, String, String, Option<String>)> =
        st.query_map([b.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)))?.filter_map(Result::ok).collect();
    let mut equity = vec![json!({ "t": b.created_utc, "equity": dec_str(b.allocation) })];
    let (mut net, mut gw, mut gl, mut fees) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    let (mut wins, mut losses) = (0usize, 0usize);
    let mut peak = b.allocation;
    let mut mdd = 0.0f64;
    let mut mdd_abs = Decimal::ZERO;
    let mut recent = vec![];
    for (tid, side, qty, entry, exit, fee, symbol, date, notes, ts) in &rows {
        let Some(p) = crate::accounts::realised(side, *qty, *entry, *exit, *fee) else { continue };
        net += p;
        fees += crate::money::Money::from_legacy_f64(*fee).map(|m| m.0).unwrap_or_default();
        if p > Decimal::ZERO {
            wins += 1;
            gw += p;
        } else {
            losses += 1;
            gl -= p;
        }
        let eq = b.allocation + net;
        if eq > peak {
            peak = eq;
        }
        if peak > Decimal::ZERO {
            mdd = mdd.max(((peak - eq) / peak).to_f64().unwrap_or(0.0));
            mdd_abs = mdd_abs.max(peak - eq);
        }
        equity.push(json!({ "t": ts.clone().unwrap_or_else(|| date.clone()), "equity": dec_str(eq.round_dp(8)) }));
        recent.push(json!({ "id": tid, "symbol": symbol, "side": side, "qty": qty, "entry": entry, "exit": exit, "fees": fee, "pnl": dec_str(p), "date": date, "closed_utc": ts, "notes": notes }));
    }
    let n = wins + losses;
    let prices = &b.state.prices;
    let lots = DbBook::lots_of(conn, b.id)?;
    let mut unreal = 0.0;
    let positions: Vec<Value> = lots
        .iter()
        .map(|l| {
            let sym = if l.leg == 1 { b.symbol2.clone().unwrap_or_default() } else { b.symbol.clone() };
            let last = prices.get(l.leg as usize).copied().flatten();
            let u = last.map(|p| l.side.sign() * (p - l.entry) * l.qty);
            unreal += u.unwrap_or(0.0);
            json!({ "id": l.id, "symbol": sym, "side": l.side.as_str(), "qty": l.qty, "entry": l.entry, "last": last, "unrealised": u.map(|x| r2(x, 8)), "stop": l.stop, "target": l.target, "tag": l.tag, "opened_bar": l.opened })
        })
        .collect();
    let exposure: f64 = lots.iter().map(|l| l.qty * l.entry).sum();
    recent.reverse();
    recent.truncate(100);
    Ok(json!({
        "allocated": dec_str(b.allocation),
        "capital": dec_str((b.allocation + net).round_dp(8)),
        "net_pnl": dec_str(net),
        "return_pct": if b.allocation > Decimal::ZERO { r2((net / b.allocation).to_f64().unwrap_or(0.0) * 100.0, 3) } else { 0.0 },
        "trades": n, "wins": wins, "losses": losses,
        "win_rate": if n > 0 { Some(r2(wins as f64 / n as f64 * 100.0, 2)) } else { None },
        "profit_factor": if gl > Decimal::ZERO { Some(r2((gw / gl).to_f64().unwrap_or(0.0), 3)) } else { None },
        "max_drawdown_pct": r2(mdd * 100.0, 3),
        "max_drawdown": dec_str(mdd_abs.round_dp(8)),
        "fees": dec_str(fees.round_dp(8)),
        "unrealised": r2(unreal, 8),
        "exposure": r2(exposure, 8),
        "open_positions": positions,
        "equity": equity,
        "recent_trades": recent,
        "price_as_of": b.last_price_utc,
        "virtual": true,
    }))
}

pub fn to_json(conn: &Connection, b: &Bot, with_perf: bool) -> Result<Value, BotError> {
    let mut v = json!({
        "id": b.id, "name": b.name, "description": b.description, "avatar": b.avatar,
        "market": b.market, "exchange": b.exchange, "mode": b.mode, "account_id": b.account_id,
        "symbol": b.symbol, "symbol2": b.symbol2, "interval": b.interval, "spec": b.spec,
        "template": b.template, "source": b.source,
        "allocation": dec_str(b.allocation), "max_daily_loss": b.max_daily_loss.map(dec_str),
        "max_open_positions": b.max_open_positions, "slippage_pct": b.slippage_pct,
        "status": b.status, "last_price": b.last_price, "last_price_utc": b.last_price_utc, "last_error": b.last_error,
        "created_utc": b.created_utc, "updated_utc": b.updated_utc, "started_utc": b.started_utc,
        "pending_orders": b.state.engine.pending.len(), "halted": b.state.engine.halted,
        "grid_lines": b.state.engine.grid_lines,
        "virtual": true,
    });
    if with_perf {
        v["performance"] = performance(conn, b)?;
    }
    Ok(v)
}

pub fn list(conn: &Connection) -> Result<Value, BotError> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM bots ORDER BY id"))?;
    let bots: Vec<Bot> = st.query_map([], from_row)?.filter_map(Result::ok).collect();
    let mut out = vec![];
    for b in &bots {
        out.push(to_json(conn, b, true)?);
    }
    Ok(json!({ "bots": out, "notice": "Bots trade the demo account only: virtual funds, real prices. This app holds no money." }))
}

pub fn logs(conn: &Connection, id: i64, limit: usize, before: Option<i64>) -> Result<Value, BotError> {
    load(conn, id)?;
    let mut st = conn.prepare("SELECT id, ts_utc, level, message FROM bot_logs WHERE bot_id = ?1 AND (?2 IS NULL OR id < ?2) ORDER BY id DESC LIMIT ?3")?;
    let logs: Vec<Value> = st.query_map(params![id, before, limit as i64], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "ts": r.get::<_, String>(1)?, "level": r.get::<_, String>(2)?, "message": r.get::<_, String>(3)? })))?.filter_map(Result::ok).collect();
    let mut st = conn.prepare("SELECT id, ts_utc, bar_time, symbol, side, action, qty, price, status, reason FROM bot_orders WHERE bot_id = ?1 ORDER BY id DESC LIMIT 200")?;
    let orders: Vec<Value> = st
        .query_map([id], |r| {
            Ok(json!({ "id": r.get::<_, i64>(0)?, "ts": r.get::<_, String>(1)?, "bar_time": r.get::<_, Option<i64>>(2)?, "symbol": r.get::<_, String>(3)?, "side": r.get::<_, String>(4)?, "action": r.get::<_, String>(5)?, "qty": r.get::<_, f64>(6)?, "price": r.get::<_, f64>(7)?, "status": r.get::<_, String>(8)?, "reason": r.get::<_, String>(9)? }))
        })?
        .filter_map(Result::ok)
        .collect();
    let next = logs.last().and_then(|l| l["id"].as_i64());
    Ok(json!({ "logs": logs, "orders": orders, "next_before": if logs.len() == limit { next } else { None } }))
}

/// Live-readiness for one bot: the account gate plus the bot's own evidence (a backtest and a demo forward
/// test). Informational; the order path refuses live orders regardless.
pub fn readiness(conn: &Connection, b: &Bot, gate: &Gate) -> Result<Value, BotError> {
    let backtests: i64 = conn.query_row("SELECT count(*) FROM backtests WHERE bot_id = ?1", [b.id], |r| r.get(0))?;
    let trades: i64 = conn.query_row("SELECT count(*) FROM trades WHERE bot_id = ?1", [b.id], |r| r.get(0))?;
    let first: Option<String> = conn.query_row("SELECT min(ts_utc) FROM bot_orders WHERE bot_id = ?1 AND status = 'filled'", [b.id], |r| r.get(0))?;
    let days = first
        .and_then(|f| chrono::DateTime::parse_from_rfc3339(&f).ok())
        .map(|f| (chrono::Utc::now() - f.with_timezone(&chrono::Utc)).num_hours() as f64 / 24.0)
        .unwrap_or(0.0);
    let mut live = crate::risk::live_status(conn, gate).map_err(BotError::Db)?;
    let own = vec![
        json!({ "id": "backtest", "ok": backtests > 0, "label": "Backtested and the report saved", "detail": format!("{backtests} saved backtest(s)") }),
        json!({ "id": "forward", "ok": days >= 7.0 && trades >= 10, "label": "Forward-tested on the demo account for 7+ days with 10+ closed trades", "detail": format!("{:.1} days, {trades} closed trades", days) }),
    ];
    if let Some(list) = live["checklist"].as_array_mut() {
        for (i, x) in own.into_iter().enumerate() {
            list.insert(i, x);
        }
    }
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        crate::db::open(":memory:").unwrap()
    }

    fn body() -> Value {
        let spec = strategy::templates().into_iter().find(|t| t["id"] == "ema_cross").unwrap()["spec"].clone();
        json!({ "name": "EMA bot", "market": "crypto", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "1000", "template": "ema_cross" })
    }

    #[test]
    fn create_validates_and_live_is_refused() {
        let mut c = conn();
        let id = create(&mut c, &body()).unwrap();
        let b = load(&c, id).unwrap();
        assert_eq!((b.status.as_str(), b.mode.as_str(), b.exchange.as_str()), ("stopped", "demo", "binance"));
        let mut live = body();
        live["mode"] = json!("live");
        assert!(matches!(create(&mut c, &live), Err(BotError::Conflict(m)) if m.contains("Live bots")));
        for (k, v) in [("symbol", json!("ETHBTC")), ("interval", json!("2h")), ("allocation", json!("-5")), ("source", json!("nope")), ("exchange", json!("fyers")), ("symbol2", json!("ETHUSDT")), ("spec", json!({"kind": "signal"}))] {
            let mut x = body();
            x[k] = v.clone();
            assert!(matches!(create(&mut c, &x), Err(BotError::Invalid(_))), "{k}={v}");
        }
        let mut nse = body();
        nse["market"] = json!("india");
        nse["symbol"] = json!("SBIN");
        nse["interval"] = json!("4h");
        assert!(create(&mut c, &nse).is_err(), "4h is not an NSE timeframe here");
        nse["interval"] = json!("1d");
        let n = create(&mut c, &nse).unwrap();
        assert_eq!(load(&c, n).unwrap().exchange, "fyers");
    }

    #[test]
    fn lifecycle_edit_duplicate_delete() {
        let mut c = conn();
        let g = Gate::default();
        let id = create(&mut c, &body()).unwrap();
        assert!(matches!(start(&c, id, &g, true), Err(BotError::Conflict(m)) if m.contains("sample")));
        start(&c, id, &g, false).unwrap();
        assert_eq!(load(&c, id).unwrap().status, "running");
        assert!(matches!(update(&mut c, id, &json!({"name": "x"})), Err(BotError::Conflict(_))));
        assert!(matches!(delete(&mut c, id), Err(BotError::Conflict(_))));
        set_status(&c, id, "paused", "test", "user").unwrap();
        update(&mut c, id, &json!({"name": "Renamed", "allocation": "2000"})).unwrap();
        let b = load(&c, id).unwrap();
        assert_eq!((b.name.as_str(), dec_str(b.allocation)), ("Renamed", "2000".into()));
        let copy = duplicate(&mut c, id).unwrap();
        assert_eq!(load(&c, copy).unwrap().name, "Renamed (copy)");
        assert_eq!(load(&c, copy).unwrap().status, "stopped");
        stop(&mut c, id, [None, None], "test", "user").unwrap();
        delete(&mut c, id).unwrap();
        assert!(matches!(load(&c, id), Err(BotError::NotFound)));
        // restart pauses running bots
        start(&c, copy, &g, false).unwrap();
        pause_all_on_start(&c);
        assert_eq!(load(&c, copy).unwrap().status, "paused");
        g.set_kill(true);
        assert!(matches!(start(&c, copy, &g, false), Err(BotError::Conflict(m)) if m.contains("kill")));
    }

    fn bars(n: usize, f: impl Fn(usize) -> f64) -> Frame {
        let mut fr = Frame::default();
        for i in 0..n {
            let c = f(i);
            let o = if i == 0 { c } else { f(i - 1) };
            fr.push(1_700_000_000 + i as i64 * 3600, o, o.max(c) + 0.5, o.min(c) - 0.5, c, 1000.0);
        }
        fr
    }

    /// The runner drives a real demo bot through the order manager: entries and exits land as positions and
    /// Journal trades on the demo account, and the bot's performance matches its trades.
    #[test]
    fn runner_trades_the_demo_account_through_the_order_manager() {
        let mut c = conn();
        let g = Gate::default();
        let spec = json!({
            "indicators": [{"id": "f", "type": "sma", "params": {"period": 3}}, {"id": "s", "type": "sma", "params": {"period": 8}}],
            "entry": {"long": {"left": {"ind": "f"}, "op": "crosses_above", "right": {"ind": "s"}}},
            "exit": {"long": {"left": {"ind": "f"}, "op": "crosses_below", "right": {"ind": "s"}}},
            "sizing": {"type": "pct_capital", "value": 50}
        });
        let id = create(&mut c, &json!({ "name": "t", "market": "crypto", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "1000", "slippage_pct": 0 })).unwrap();
        start(&c, id, &g, false).unwrap();
        // falling, then rising (entry), then falling (exit)
        let price = |i: usize| -> f64 {
            match i {
                0..=20 => 100.0 - i as f64,
                21..=40 => 80.0 + (i - 20) as f64 * 2.0,
                _ => 120.0 - (i - 40) as f64 * 3.0,
            }
        };
        let full = bars(70, price);
        // first tick: baseline at bar 19's close
        let upto = |n: usize| crate::backtest2::slice(&full, 0, n);
        let now_after = |n: usize| full.t[n - 1] + 3600; // every bar up to n-1 is closed
        process(&mut c, &g, id, &upto(20), None, now_after(20)).unwrap();
        for n in 21..=70 {
            // the newest bar is still forming: `now` is inside it
            let f = upto(n);
            process(&mut c, &g, id, &f, None, f.t[n - 1] + 1800).unwrap();
        }
        let b = load(&c, id).unwrap();
        let perf = performance(&c, &b).unwrap();
        assert!(perf["trades"].as_u64().unwrap() >= 1, "{perf}");
        let trades: i64 = c.query_row("SELECT count(*) FROM trades WHERE bot_id = ?1 AND account_id = 'demo-1' AND mode = 'demo'", [id], |r| r.get(0)).unwrap();
        assert_eq!(trades as u64, perf["trades"].as_u64().unwrap());
        let filled: i64 = c.query_row("SELECT count(*) FROM bot_orders WHERE bot_id = ?1 AND status = 'filled'", [id], |r| r.get(0)).unwrap();
        assert!(filled >= 2);
        // every fill is at a bar open from the data, never a made-up price
        let mut st = c.prepare("SELECT price, bar_time FROM bot_orders WHERE bot_id = ?1 AND status = 'filled'").unwrap();
        let fills: Vec<(f64, i64)> = st.query_map([id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().filter_map(Result::ok).collect();
        for (p, t) in fills {
            let i = full.t.iter().position(|x| *x == t).unwrap();
            assert_eq!(p, full.o[i], "fill at bar {i}'s open");
        }
        // the demo wallet moved by exactly the bot's realised P&L
        let bal: String = c.query_row("SELECT balance FROM demo_wallets WHERE account_id = 'demo-1' AND currency = 'USDT'", [], |r| r.get(0)).unwrap();
        let net: Decimal = perf["net_pnl"].as_str().unwrap().parse().unwrap();
        assert_eq!(Decimal::from_str(&bal).unwrap(), Decimal::from(10_000) + net);
    }

    #[test]
    fn runner_skips_gaps_instead_of_replaying_them() {
        let mut c = conn();
        let g = Gate::default();
        let id = create(&mut c, &body()).unwrap();
        start(&c, id, &g, false).unwrap();
        let f = bars(100, |i| 100.0 + (i as f64 * 0.3).sin() * 5.0);
        process(&mut c, &g, id, &crate::backtest2::slice(&f, 0, 50), None, f.t[49] + 3600).unwrap();
        let out = process(&mut c, &g, id, &f, None, f.t[99] + 3600).unwrap();
        assert_eq!(out, "skipped a gap");
        let orders: i64 = c.query_row("SELECT count(*) FROM bot_orders WHERE bot_id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(orders, 0);
    }

    #[test]
    fn account_limits_bind_bots_too() {
        let mut c = conn();
        let g = Gate::default();
        crate::risk::set_limits(&mut c, "demo-1", &json!({ "USDT": { "max_order_notional": "10" } })).unwrap();
        let spec = json!({ "entry": {"long": {"left": {"price": "close"}, "op": "gt", "right": {"value": 0}}}, "sizing": {"type": "fixed_notional", "value": 100} });
        let id = create(&mut c, &json!({ "name": "t", "symbol": "BTCUSDT", "interval": "1h", "spec": spec, "allocation": "1000", "slippage_pct": 0 })).unwrap();
        start(&c, id, &g, false).unwrap();
        let f = bars(10, |i| 100.0 + i as f64);
        process(&mut c, &g, id, &crate::backtest2::slice(&f, 0, 5), None, f.t[4] + 3600).unwrap();
        for n in 6..=10 {
            let s = crate::backtest2::slice(&f, 0, n);
            process(&mut c, &g, id, &s, None, s.t[n - 1] + 1800).unwrap();
        }
        let filled: i64 = c.query_row("SELECT count(*) FROM bot_orders WHERE bot_id = ?1 AND status = 'filled'", [id], |r| r.get(0)).unwrap();
        let rejected: String = c.query_row("SELECT reason FROM bot_orders WHERE bot_id = ?1 AND status = 'rejected' LIMIT 1", [id], |r| r.get(0)).unwrap();
        assert_eq!(filled, 0);
        assert!(rejected.contains("max order size"), "{rejected}");
        // the kill switch blocks the bot too
        crate::risk::set_limits(&mut c, "demo-1", &json!({ "USDT": { "max_order_notional": null } })).unwrap();
        g.set_kill(true);
        let s = crate::backtest2::slice(&f, 0, 10);
        process(&mut c, &g, id, &s, None, s.t[9] + 7200).unwrap();
        let filled: i64 = c.query_row("SELECT count(*) FROM bot_orders WHERE bot_id = ?1 AND status = 'filled'", [id], |r| r.get(0)).unwrap();
        assert_eq!(filled, 0);
    }
}
