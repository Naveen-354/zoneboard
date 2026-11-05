//! Broker execution for bots on a connected Binance account (Spot Testnet, or live when approved and armed).
//!
//! The bot engine is synchronous and runs inside one database transaction, but a broker order is a network
//! call. So for a broker account the engine's book records **intents**; after the engine step, the runner
//! executes them here:
//!
//! 1. risk check (kill switch, live arming and approval, account and bot limits) - fail closed;
//! 2. normalise the quantity to the symbol's lot rules;
//! 3. write an outbox row (`broker_orders`, status `submitting`) keyed by a deterministic client order id
//!    `zb{bot}-{bar}-{n}`; the unique key makes a second submission of the same intent impossible;
//! 4. send the MARKET order with that client order id;
//! 5. record the fill from the exchange's own numbers (executed quantity and quote amount);
//! 6. on an unknown outcome (timeout, connection drop), look the order up by its client order id **before**
//!    anything else; if the exchange cannot tell, the row stays `unknown` and the bot is stopped for a person.
//!
//! Spot only: broker bots are long only (no shorting on spot) and never pairs.

use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::botengine::Dist;
use crate::exchange::binance::{normalize_qty, BinanceClient, OrderReport};
use crate::exchange::ExchangeError;
use crate::risk::{BotLimits, Gate, Order};

/// What the engine asked for on a broker account.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    Open { qty: f64, ref_price: f64, bar_time: i64, stop: Option<f64>, target: Option<f64>, trail: Option<Dist>, tag: String },
    Close { position_id: i64, bar_time: i64, reason: String },
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub struct BotCtx {
    pub bot_id: i64,
    pub bot_name: String,
    pub account_id: String,
    pub symbol: String,
    pub capital: Decimal,
    pub max_daily_loss: Option<Decimal>,
    pub max_open_positions: i64,
    pub live_approved: bool,
}

/// Deterministic client order id: the same intent (bot, bar, position in that bar's list) always maps to the
/// same id, so a replayed step cannot create a second order. Binance allows up to 36 characters.
pub fn client_order_id(bot: i64, bar_time: i64, n: usize) -> String {
    format!("zb{bot}-{bar_time}-{n}")
}

fn set(conn: &Connection, cid: &str, status: &str, rep: Option<&OrderReport>, reason: &str) {
    let _ = conn.execute(
        "UPDATE broker_orders SET status = ?1, broker_order_id = coalesce(?2, broker_order_id), executed_qty = coalesce(?3, executed_qty), avg_price = coalesce(?4, avg_price), reason = ?5, updated_utc = ?6 WHERE client_order_id = ?7",
        params![status, rep.map(|r| r.order_id.to_string()), rep.map(|r| r.executed_qty.clone()), rep.and_then(|r| r.avg_price.clone()), reason, now(), cid],
    );
}

/// Turn a filled report into a position or a closed trade. Everything comes from the exchange's numbers.
fn record_fill(conn: &mut Connection, ctx: &BotCtx, cid: &str, rep: &OrderReport) -> Result<(), String> {
    let row: (String, Option<i64>, Option<i64>, String, Option<f64>, Option<f64>, Option<String>) = conn
        .query_row("SELECT action, position_id, bar_time, tag, stop, target, trail FROM broker_orders WHERE client_order_id = ?1", [cid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
        .map_err(|e| e.to_string())?;
    let (action, pos, bar, tag, stop, target, trail) = row;
    let qty: f64 = rep.executed_qty.parse().unwrap_or(0.0);
    let Some(px) = rep.avg_price.as_deref().and_then(|p| p.parse::<f64>().ok()) else {
        set(conn, cid, "rejected", Some(rep), "exchange reported no executed quantity");
        return Ok(());
    };
    if qty <= 0.0 {
        set(conn, cid, "rejected", Some(rep), "nothing executed");
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    if action == "open" {
        tx.execute(
            "INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at, account_id, bot_id, leg, stop, target, trail, trail_ref, tag, bar_time)
             VALUES ('crypto','live',?1,'long',?2,?3,?4,?5,?6,?7,0,?8,?9,?10,?11,?12,?13)",
            params![ctx.symbol, qty, px, crate::journal::fee_pct("crypto"), chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(), ctx.account_id, ctx.bot_id, stop, target, trail, trail.as_ref().map(|_| px), tag, bar],
        )
        .map_err(|e| e.to_string())?;
        let id = tx.last_insert_rowid();
        tx.execute("UPDATE broker_orders SET position_id = ?1 WHERE client_order_id = ?2", params![id, cid]).map_err(|e| e.to_string())?;
    } else if let Some(p) = pos {
        let notes = format!("Bot {} on {}: broker fill {} ({})", ctx.bot_name, ctx.account_id, rep.order_id, tag);
        crate::journal::close_in(&tx, p, px, None, &notes).map_err(|e| format!("{e:?}"))?;
    }
    tx.execute(
        "INSERT INTO bot_orders (bot_id, ts_utc, bar_time, symbol, side, action, qty, price, status, reason, client_order_id, broker_order_id, environment) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'filled',?9,?10,?11,'broker')",
        params![ctx.bot_id, now(), bar, ctx.symbol, "long", action, qty, px, tag, cid, rep.order_id.to_string()],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    set(conn, cid, "filled", Some(rep), "");
    Ok(())
}

/// Write the outbox row for one intent. `Ok(None)` when this exact intent was already submitted.
fn outbox(conn: &mut Connection, ctx: &BotCtx, cid: &str, intent: &Intent, qty: &str) -> Result<Option<()>, String> {
    let (action, side, pos, bar, tag, stop, target, trail) = match intent {
        Intent::Open { bar_time, stop, target, trail, tag, .. } => ("open", "BUY", None, *bar_time, tag.clone(), *stop, *target, trail.map(|d| serde_json::to_string(&d).unwrap_or_default())),
        Intent::Close { position_id, bar_time, reason } => ("close", "SELL", Some(*position_id), *bar_time, reason.clone(), None, None, None),
    };
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO broker_orders (bot_id, account_id, client_order_id, symbol, side, action, qty, position_id, bar_time, tag, stop, target, trail, status, created_utc, updated_utc)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'submitting',?14,?14)",
            params![ctx.bot_id, ctx.account_id, cid, ctx.symbol, side, action, qty, pos, bar, tag, stop, target, trail, now()],
        )
        .map_err(|e| e.to_string())?;
    Ok((n == 1).then_some(()))
}

fn reject(conn: &Connection, ctx: &BotCtx, intent: &Intent, reason: &str) {
    let (action, qty, px, bar) = match intent {
        Intent::Open { qty, ref_price, bar_time, .. } => ("open", *qty, *ref_price, *bar_time),
        Intent::Close { bar_time, .. } => ("close", 0.0, 0.0, *bar_time),
    };
    let _ = conn.execute(
        "INSERT INTO bot_orders (bot_id, ts_utc, bar_time, symbol, side, action, qty, price, status, reason, environment) VALUES (?1,?2,?3,?4,'long',?5,?6,?7,'rejected',?8,'broker')",
        params![ctx.bot_id, now(), bar, ctx.symbol, action, qty, px, reason],
    );
    crate::bots::log(conn, ctx.bot_id, "warn", &format!("broker order refused: {reason}"));
}

/// Execute the intents of one runner step. `with_conn` gives synchronous access to the database between
/// network calls. Returns how many orders filled.
pub async fn execute<F>(client: &BinanceClient, gate: &Gate, ctx: &BotCtx, intents: Vec<Intent>, with_conn: F) -> usize
where
    F: Fn(&mut dyn FnMut(&mut Connection)),
{
    let mut filled = 0;
    let rules = match client.symbol_rules(&ctx.symbol).await {
        Ok(r) if r.status == "TRADING" => Some(r),
        Ok(r) => {
            with_conn(&mut |c| intents.iter().for_each(|i| reject(c, ctx, i, &format!("{} is not trading ({})", ctx.symbol, r.status))));
            return 0;
        }
        Err(e) => {
            with_conn(&mut |c| intents.iter().for_each(|i| reject(c, ctx, i, &format!("could not read the symbol's lot rules: {e}"))));
            return 0;
        }
    };
    let rules = rules.unwrap();
    for (n, intent) in intents.iter().enumerate() {
        let bar = match intent {
            Intent::Open { bar_time, .. } | Intent::Close { bar_time, .. } => *bar_time,
        };
        let cid = client_order_id(ctx.bot_id, bar, n);
        // quantity and the risk gate, before anything is written or sent
        let mut prepared: Result<String, String> = Err(String::new());
        with_conn(&mut |c| {
            prepared = (|| -> Result<String, String> {
                match intent {
                    Intent::Open { qty, ref_price, .. } => {
                        let q = Decimal::from_str(&format!("{qty:.8}")).map_err(|e| e.to_string())?;
                        let qs = normalize_qty(q, &rules).ok_or("quantity below the symbol's minimum")?;
                        let qf: f64 = qs.parse().unwrap_or(0.0);
                        if let Some(mn) = rules.min_notional.as_deref().and_then(|m| m.parse::<f64>().ok()) {
                            if qf * ref_price < mn {
                                return Err(format!("order value below the symbol's minimum of {mn}"));
                            }
                        }
                        let order = Order {
                            account_id: ctx.account_id.clone(),
                            market: "crypto".into(),
                            symbol: ctx.symbol.clone(),
                            qty: qf,
                            price: *ref_price,
                            bot: Some(BotLimits { bot_id: ctx.bot_id, capital: ctx.capital, max_daily_loss: ctx.max_daily_loss, max_open_positions: ctx.max_open_positions }),
                        };
                        crate::risk::check_broker(c, &order, gate, ctx.live_approved)?;
                        Ok(qs)
                    }
                    Intent::Close { position_id, .. } => {
                        let q: Option<f64> = c.query_row("SELECT qty FROM positions WHERE id = ?1 AND bot_id = ?2", params![position_id, ctx.bot_id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
                        let q = q.ok_or("the position is already closed")?;
                        // a close is never blocked by limits; it must still respect the lot step
                        let d = Decimal::from_str(&format!("{q:.8}")).map_err(|e| e.to_string())?;
                        normalize_qty(d, &rules).ok_or_else(|| "position smaller than the symbol's minimum".to_string())
                    }
                }
            })();
        });
        let qs = match prepared {
            Ok(q) => q,
            Err(e) => {
                with_conn(&mut |c| reject(c, ctx, intent, &e));
                continue;
            }
        };
        let mut fresh = Ok(None);
        with_conn(&mut |c| fresh = outbox(c, ctx, &cid, intent, &qs));
        match fresh {
            Ok(Some(())) => {}
            Ok(None) => {
                with_conn(&mut |c| crate::bots::log(c, ctx.bot_id, "warn", &format!("order {cid} was already submitted: not sending it again")));
                continue;
            }
            Err(e) => {
                with_conn(&mut |c| reject(c, ctx, intent, &e));
                continue;
            }
        }
        let side = if matches!(intent, Intent::Open { .. }) { "BUY" } else { "SELL" };
        let outcome = match client.place_market_order(&ctx.symbol, side, &qs, &cid).await {
            Ok(rep) => Ok(rep),
            Err(ExchangeError::Network(m)) => {
                // unknown outcome: ask the exchange before deciding anything
                match client.get_order(&ctx.symbol, &cid).await {
                    Ok(Some(rep)) => Ok(rep),
                    Ok(None) => Err(("rejected", format!("not placed ({m}); the exchange has no such order"))),
                    Err(e) => Err(("unknown", format!("outcome unknown ({m}; lookup failed: {e}). The bot is stopped: check the exchange and reconcile."))),
                }
            }
            Err(e) => Err(("rejected", e.to_string())),
        };
        match outcome {
            Ok(rep) => {
                let mut ok = Err(String::new());
                with_conn(&mut |c| ok = record_fill(c, ctx, &cid, &rep));
                match ok {
                    Ok(()) => {
                        filled += 1;
                        with_conn(&mut |c| crate::bots::log(c, ctx.bot_id, "trade", &format!("broker {side} {} {} filled at avg {} (order {})", rep.executed_qty, ctx.symbol, rep.avg_price.clone().unwrap_or_default(), rep.order_id)));
                    }
                    Err(e) => with_conn(&mut |c| crate::bots::log(c, ctx.bot_id, "error", &format!("fill of {cid} could not be recorded: {e}"))),
                }
            }
            Err((status, reason)) => {
                with_conn(&mut |c| {
                    set(c, &cid, status, None, &reason);
                    crate::bots::log(c, ctx.bot_id, if status == "unknown" { "error" } else { "warn" }, &format!("{side} {qs} {}: {reason}", ctx.symbol));
                    if status == "unknown" {
                        let _ = crate::bots::set_status(c, ctx.bot_id, "stopped", "an order's outcome is unknown: reconcile before trading again", "system");
                    }
                });
            }
        }
    }
    filled
}

/// Re-check every outbox row still `submitting` or `unknown` for this account (after a crash or a lost
/// reply), using the exchange as the source of truth. Returns a summary.
pub async fn reconcile<F>(client: &BinanceClient, account_id: &str, with_conn: F) -> Value
where
    F: Fn(&mut dyn FnMut(&mut Connection)),
{
    let mut rows: Vec<(String, String, i64, String)> = vec![];
    with_conn(&mut |c| {
        if let Ok(mut st) = c.prepare("SELECT client_order_id, symbol, bot_id, b.name FROM broker_orders o LEFT JOIN bots b ON b.id = o.bot_id WHERE o.account_id = ?1 AND o.status IN ('submitting','unknown')") {
            rows = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, Option<String>>(3)?.unwrap_or_default()))).map(|m| m.filter_map(Result::ok).collect()).unwrap_or_default();
        }
    });
    let (mut filled, mut absent, mut still) = (0, 0, 0);
    for (cid, symbol, bot, name) in rows {
        let ctx = BotCtx { bot_id: bot, bot_name: name, account_id: account_id.to_string(), symbol: symbol.clone(), capital: Decimal::ZERO, max_daily_loss: None, max_open_positions: 0, live_approved: false };
        match client.get_order(&symbol, &cid).await {
            Ok(Some(rep)) if rep.executed_qty.parse::<f64>().unwrap_or(0.0) > 0.0 => {
                with_conn(&mut |c| {
                    let _ = record_fill(c, &ctx, &cid, &rep);
                });
                filled += 1;
            }
            Ok(Some(rep)) => {
                with_conn(&mut |c| set(c, &cid, "rejected", Some(&rep), &format!("exchange status {}", rep.status)));
                absent += 1;
            }
            Ok(None) => {
                with_conn(&mut |c| set(c, &cid, "rejected", None, "the exchange has no such order"));
                absent += 1;
            }
            Err(_) => still += 1,
        }
    }
    json!({ "recorded_fills": filled, "not_placed": absent, "still_unknown": still })
}
