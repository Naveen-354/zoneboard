//! The bot order manager: the only way a bot opens or closes a position.
//!
//! An open runs in one IMMEDIATE transaction: account gate, funds check, every risk check ([`crate::risk`]),
//! the position insert and the order record. A refusal is recorded too (`status = 'rejected'` with the reason),
//! so the execution log shows what a bot *tried* to do. Closes go through `journal::close_in`, the same code
//! as a manual close, and are never blocked by limits or the kill switch.
//!
//! Fills are paper fills on the demo account at the price the server's own data gave (the engine's fill
//! price), never a price sent by the browser.

use rusqlite::{params, Connection, TransactionBehavior};
use rust_decimal::Decimal;

use crate::botengine::{Dist, OpenReq};
use crate::risk::{self, BotLimits, Gate, Order};

pub struct BotRef<'a> {
    pub id: i64,
    pub name: &'a str,
    pub account_id: &'a str,
    pub market: &'a str,
    pub capital: Decimal,
    pub max_daily_loss: Option<Decimal>,
    pub max_open_positions: i64,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[allow(clippy::too_many_arguments)]
fn record(conn: &Connection, bot: i64, bar: i64, symbol: &str, side: &str, action: &str, qty: f64, price: f64, status: &str, reason: &str, pos: Option<i64>, trade: Option<i64>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO bot_orders (bot_id, ts_utc, bar_time, symbol, side, action, qty, price, status, reason, position_id, trade_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![bot, now(), bar, symbol, side, action, qty, price, status, reason, pos, trade],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Open a bot position. Returns the position id, or the reason it was refused (also written to `bot_orders`).
pub fn open(conn: &mut Connection, bot: &BotRef, symbol: &str, req: &OpenReq, gate: &Gate) -> Result<i64, String> {
    let result = (|| -> Result<i64, String> {
        if !(req.qty.is_finite() && req.qty > 0.0 && req.price.is_finite() && req.price > 0.0) {
            return Err("invalid size or price".into());
        }
        if bot.market == "india" && req.qty.fract() != 0.0 {
            return Err("NSE quantity must be a whole number".into());
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        crate::accounts::require_demo(&tx, bot.account_id)?;
        crate::accounts::check_funds(&tx, bot.account_id, bot.market, symbol, req.qty, req.price)?;
        let order = Order {
            account_id: bot.account_id.to_string(),
            market: bot.market.to_string(),
            symbol: symbol.to_string(),
            qty: req.qty,
            price: req.price,
            bot: Some(BotLimits { bot_id: bot.id, capital: bot.capital, max_daily_loss: bot.max_daily_loss, max_open_positions: bot.max_open_positions }),
        };
        risk::check(&tx, &order, gate)?;
        let trail = req.trail.map(|d| serde_json::to_string(&d).unwrap_or_default());
        tx.execute(
            "INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at, account_id, bot_id, leg, stop, target, trail, trail_ref, tag, bar_time, strategy_version_id)
             VALUES (?1,'demo',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,(SELECT strategy_version_id FROM bots WHERE id = ?9))",
            params![
                bot.market, symbol, req.side.as_str(), req.qty, req.price, crate::journal::fee_pct(bot.market),
                chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(), bot.account_id, bot.id, req.leg,
                req.stop, req.target, trail, req.trail.map(|_| req.price), req.tag, req.time
            ],
        )
        .map_err(|e| e.to_string())?;
        let id = tx.last_insert_rowid();
        record(&tx, bot.id, req.time, symbol, req.side.as_str(), "open", req.qty, req.price, "filled", &req.tag, Some(id), None)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(id)
    })();
    if let Err(reason) = &result {
        let _ = record(conn, bot.id, req.time, symbol, req.side.as_str(), "open", req.qty, req.price, "rejected", reason, None, None);
        crate::riskintel::on_rejection(conn, &format!("bot {}", bot.id), bot.account_id, symbol, reason);
    } else {
        crate::events::emit(conn, "OrderFilled", &format!("bot:{}", bot.id), serde_json::json!({ "symbol": symbol, "side": req.side.as_str(), "qty": req.qty, "price": req.price, "paper": true }), Some(&format!("bot:{}:{}", bot.id, req.time)), None);
    }
    result
}

/// Close a bot position at `price`. The trade lands in the Journal with the bot's name and the reason.
pub fn close(conn: &mut Connection, bot: &BotRef, position: i64, price: f64, bar: i64, reason: &str) -> Result<i64, String> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let owner: Option<(Option<i64>, String, String, f64)> = tx
        .query_row("SELECT bot_id, symbol, side, qty FROM positions WHERE id = ?1", [position], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .ok();
    let Some((Some(owner), symbol, side, qty)) = owner else { return Err("no such bot position".into()) };
    if owner != bot.id {
        return Err("that position belongs to another bot".into());
    }
    let notes: String = format!("Bot {}: {reason}", bot.name).chars().take(500).collect();
    let trade = crate::journal::close_in(&tx, position, price, None, &notes).map_err(|e| match e {
        crate::journal::CloseError::NotFound => "no such bot position".to_string(),
        crate::journal::CloseError::Invalid(m) => m,
    })?;
    record(&tx, bot.id, bar, &symbol, &side, "close", qty, price, "filled", reason, Some(position), Some(trade.id))?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(trade.id)
}

pub fn parse_trail(s: Option<String>) -> Option<Dist> {
    s.and_then(|s| serde_json::from_str(&s).ok())
}
