//! Portfolio-wide risk: extra limits enforced by the central risk engine, and the portfolio risk view.
//!
//! Limits added to `risk::check_limits` (so every order path, manual, bot, broker, workflow or API, uses them):
//!  * per account and currency: max position per symbol, max open positions, denied symbols;
//!  * per currency across **all** accounts: max exposure, max daily loss, max drawdown of the demo equity.
//! Drawdown is measured on demo wallets only (their equity is known exactly); broker accounts have no equity
//! history in ZoneBoard yet, which the view says.
//!
//! Nothing here closes positions. A breach blocks new risk and is recorded as an event and a notification.

use std::str::FromStr;

use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::accounts::{self, dec_str};
use crate::risk::Order;

fn d(s: Option<String>) -> Option<Decimal> {
    s.and_then(|s| Decimal::from_str(&s).ok())
}

struct Extra {
    max_position: Option<Decimal>,
    max_open: Option<i64>,
    denied: Vec<String>,
}

fn extra(conn: &Connection, account: &str, currency: &str) -> Extra {
    let row: Option<(Option<String>, Option<i64>, Option<String>)> = conn
        .query_row("SELECT max_position_notional, max_open_positions, denied_symbols FROM account_limits WHERE account_id = ?1 AND currency = ?2", params![account, currency], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .ok()
        .flatten();
    let (a, b, c) = row.unwrap_or((None, None, None));
    Extra { max_position: d(a), max_open: b, denied: c.unwrap_or_default().split(',').map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()).collect() }
}

/// (max exposure, max daily loss, max drawdown %) for one currency across every account.
pub fn portfolio_limits(conn: &Connection, currency: &str) -> (Option<Decimal>, Option<Decimal>, Option<Decimal>) {
    conn.query_row("SELECT max_exposure, max_daily_loss, max_drawdown_pct FROM portfolio_limits WHERE currency = ?1", [currency], |r| Ok((d(r.get(0)?), d(r.get(1)?), d(r.get(2)?))))
        .optional()
        .ok()
        .flatten()
        .unwrap_or((None, None, None))
}

fn all_open_notional(conn: &Connection, currency: &str, symbol: Option<&str>, account: Option<&str>) -> Decimal {
    let mut st = match conn.prepare("SELECT market, symbol, qty, entry FROM positions WHERE account_id IS NOT NULL AND (?1 IS NULL OR symbol = ?1) AND (?2 IS NULL OR account_id = ?2)") {
        Ok(s) => s,
        Err(_) => return Decimal::ZERO,
    };
    let rows: Vec<(String, String, f64, f64)> = st.query_map(params![symbol, account], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map(|m| m.filter_map(Result::ok).collect()).unwrap_or_default();
    rows.into_iter().filter(|(m, s, _, _)| accounts::currency_for(m, s).ok() == Some(currency)).filter_map(|(_, _, q, e)| accounts::notional(q, e)).sum()
}

/// Demo equity in one currency (sum of wallet balances) and its running peak, which is remembered.
pub fn demo_drawdown(conn: &Connection, currency: &str) -> (Decimal, Decimal, f64) {
    let eq: Decimal = conn
        .prepare("SELECT balance FROM demo_wallets w JOIN accounts a ON a.id = w.account_id WHERE w.currency = ?1 AND a.archived = 0")
        .and_then(|mut st| st.query_map([currency], |r| r.get::<_, String>(0)).map(|m| m.filter_map(Result::ok).filter_map(|s| Decimal::from_str(&s).ok()).sum()))
        .unwrap_or_default();
    let key = format!("risk.peak_equity.{currency}");
    let peak = crate::events::kv_get(conn, &key).and_then(|s| Decimal::from_str(&s).ok()).unwrap_or(eq).max(eq);
    crate::events::kv_set(conn, &key, &dec_str(peak));
    let dd = if peak > Decimal::ZERO { ((peak - eq) / peak).to_f64().unwrap_or(0.0) * 100.0 } else { 0.0 };
    (eq, peak, dd)
}

/// Checks run by `risk::check_limits` before the original ones.
pub fn extra_checks(conn: &Connection, o: &Order) -> Result<(), String> {
    if let Some(e) = crate::intel::blackout_hit(conn, &o.market, chrono::Utc::now()) {
        return Err(format!("economic-event blackout: {e}; new entries are paused around high-importance events"));
    }
    let currency = accounts::currency_for(&o.market, &o.symbol)?;
    let need = accounts::notional(o.qty, o.price).ok_or("order size is too large")?;
    let x = extra(conn, &o.account_id, currency);
    if x.denied.iter().any(|s| *s == o.symbol) {
        return Err(format!("{} is on this account's denied-instrument list", o.symbol));
    }
    if let Some(m) = x.max_position {
        let held = all_open_notional(conn, currency, Some(&o.symbol), Some(&o.account_id));
        if held + need > m {
            return Err(format!("position in {} would be {} {currency}, above this account's per-symbol limit of {} {currency}", o.symbol, dec_str(held + need), dec_str(m)));
        }
    }
    if let Some(m) = x.max_open {
        let n: i64 = conn.query_row("SELECT count(*) FROM positions WHERE account_id = ?1", [&o.account_id], |r| r.get(0)).unwrap_or(0);
        if n >= m {
            return Err(format!("this account already has {n} open positions, its limit"));
        }
    }
    let (pe, pl, pdd) = portfolio_limits(conn, currency);
    if let Some(m) = pe {
        let all = all_open_notional(conn, currency, None, None);
        if all + need > m {
            return Err(format!("portfolio exposure would be {} {currency}, above the portfolio limit of {} {currency}", dec_str(all + need), dec_str(m)));
        }
    }
    if let Some(m) = pl {
        let accts: Vec<String> = conn.prepare("SELECT id FROM accounts WHERE archived = 0").and_then(|mut st| st.query_map([], |r| r.get(0)).map(|x| x.filter_map(Result::ok).collect())).unwrap_or_default();
        let today: Decimal = accts.iter().filter_map(|a| crate::risk::realised_today(conn, a, None, currency).ok()).sum();
        if -today >= m {
            return Err(format!("portfolio daily loss limit reached: {} {currency} lost today across all accounts, limit {} {currency}", dec_str(-today), dec_str(m)));
        }
    }
    if let Some(m) = pdd {
        let (_, _, dd) = demo_drawdown(conn, currency);
        if dd >= m.to_f64().unwrap_or(f64::MAX) {
            return Err(format!("portfolio drawdown limit reached: demo equity is {dd:.2}% below its peak, limit {m}%"));
        }
    }
    Ok(())
}

/// Broker data must be trustworthy before a broker order: an account whose last call failed is held.
pub fn broker_data_reliable(conn: &Connection, account: &str) -> Result<(), String> {
    let e: Option<String> = conn.query_row("SELECT last_error FROM accounts WHERE id = ?1", [account], |r| r.get(0)).optional().map_err(|e| e.to_string())?.flatten();
    match e {
        Some(e) => Err(format!("risk cannot be evaluated reliably: the broker's last reply was an error ({e}); refresh or verify the account first")),
        None => Ok(()),
    }
}

/// `PUT /api/risk/limits/{account}/extra`: `{ "USDT": { "max_position_notional": "..", "max_open_positions": 5, "denied_symbols": "DOGEUSDT,SHIBUSDT" } }`.
pub fn set_extra(conn: &Connection, account: &str, body: &Value) -> Result<Value, String> {
    let m = body.as_object().ok_or("body must be an object")?;
    for (cur, v) in m {
        if !accounts::CURRENCIES.contains(&cur.as_str()) {
            return Err(format!("unknown currency '{cur}'"));
        }
        let lm = v.as_object().ok_or(format!("{cur} must be an object"))?;
        if let Some(k) = lm.keys().find(|k| !["max_position_notional", "max_open_positions", "denied_symbols"].contains(&k.as_str())) {
            return Err(format!("{cur}: unknown field '{k}'"));
        }
        let pos = match lm.get("max_position_notional") {
            None | Some(Value::Null) => None,
            Some(x) => {
                let t = x.as_str().map(String::from).unwrap_or_else(|| x.to_string());
                let v = Decimal::from_str(t.trim()).map_err(|_| format!("{cur}.max_position_notional must be a decimal number"))?;
                if v <= Decimal::ZERO {
                    return Err(format!("{cur}.max_position_notional must be above 0"));
                }
                Some(dec_str(v))
            }
        };
        let open = match lm.get("max_open_positions") {
            None | Some(Value::Null) => None,
            Some(x) => Some(x.as_i64().filter(|n| (1..=1000).contains(n)).ok_or(format!("{cur}.max_open_positions must be 1 to 1000"))?),
        };
        let denied = lm.get("denied_symbols").and_then(Value::as_str).map(|s| s.split(',').map(|x| x.trim().to_uppercase()).filter(|x| !x.is_empty()).collect::<Vec<_>>());
        if denied.as_ref().is_some_and(|d| d.iter().any(|s| !crate::data::valid_nse_symbol(s))) {
            return Err("denied_symbols must be symbols separated by commas".into());
        }
        conn.execute(
            "INSERT INTO account_limits (account_id, currency, max_position_notional, max_open_positions, denied_symbols) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(account_id, currency) DO UPDATE SET max_position_notional = excluded.max_position_notional, max_open_positions = excluded.max_open_positions, denied_symbols = excluded.denied_symbols",
            params![account, cur, pos, open, denied.map(|d| d.join(","))],
        )
        .map_err(|e| e.to_string())?;
    }
    crate::audit::record(conn, "user", "risk_limits_updated", &format!("account:{account}"), &json!({ "account_id": account, "extra": body })).map_err(|e| e.to_string())?;
    Ok(json!({ "ok": true }))
}

/// `PUT /api/risk/portfolio`: `{ "USDT": { "max_exposure": "..", "max_daily_loss": "..", "max_drawdown_pct": "10" } }`.
pub fn set_portfolio(conn: &Connection, body: &Value) -> Result<Value, String> {
    let m = body.as_object().ok_or("body must be an object")?;
    for (cur, v) in m {
        if !accounts::CURRENCIES.contains(&cur.as_str()) {
            return Err(format!("unknown currency '{cur}'"));
        }
        let lm = v.as_object().ok_or(format!("{cur} must be an object"))?;
        let mut vals = vec![];
        for k in ["max_exposure", "max_daily_loss", "max_drawdown_pct"] {
            vals.push(match lm.get(k) {
                None | Some(Value::Null) => None,
                Some(x) => {
                    let t = x.as_str().map(String::from).unwrap_or_else(|| x.to_string());
                    let v = Decimal::from_str(t.trim()).map_err(|_| format!("{cur}.{k} must be a decimal number"))?;
                    if v <= Decimal::ZERO || (k == "max_drawdown_pct" && v > Decimal::from(100)) {
                        return Err(format!("{cur}.{k} is out of range"));
                    }
                    Some(dec_str(v))
                }
            });
        }
        conn.execute(
            "INSERT INTO portfolio_limits (currency, max_exposure, max_daily_loss, max_drawdown_pct) VALUES (?1,?2,?3,?4)
             ON CONFLICT(currency) DO UPDATE SET max_exposure = excluded.max_exposure, max_daily_loss = excluded.max_daily_loss, max_drawdown_pct = excluded.max_drawdown_pct",
            params![cur, vals[0], vals[1], vals[2]],
        )
        .map_err(|e| e.to_string())?;
    }
    crate::audit::record(conn, "user", "portfolio_limits_updated", "portfolio", body).map_err(|e| e.to_string())?;
    Ok(view(conn))
}

/// The portfolio risk view. Unrealised P&L uses the latest price the runner saw for a symbol; positions with no
/// recent price say so instead of guessing.
pub fn view(conn: &Connection) -> Value {
    let prices: std::collections::HashMap<String, (f64, Option<String>)> = conn
        .prepare("SELECT symbol, last_price, last_price_utc FROM bots WHERE last_price IS NOT NULL")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, f64>(1)?, r.get::<_, Option<String>>(2)?)))).map(|m| m.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let rows: Vec<(String, Option<i64>, String, String, String, f64, f64)> = conn
        .prepare("SELECT account_id, bot_id, market, symbol, side, qty, entry FROM positions WHERE account_id IS NOT NULL")
        .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).map(|m| m.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let mut out_cur = serde_json::Map::new();
    for cur in accounts::CURRENCIES {
        let mine: Vec<&(String, Option<i64>, String, String, String, f64, f64)> = rows.iter().filter(|r| accounts::currency_for(&r.2, &r.3).ok() == Some(cur)).collect();
        let gross: f64 = mine.iter().map(|r| r.5 * r.6).sum();
        let net: f64 = mine.iter().map(|r| if r.4 == "long" { r.5 * r.6 } else { -r.5 * r.6 }).sum();
        let mut by_symbol: std::collections::BTreeMap<String, (f64, std::collections::BTreeSet<String>)> = Default::default();
        let mut contributors = vec![];
        let mut unreal = 0.0;
        let mut unpriced = 0;
        for r in &mine {
            let n = r.5 * r.6;
            let src = r.1.map(|b| format!("bot {b}")).unwrap_or_else(|| "manual".into());
            let e = by_symbol.entry(r.3.clone()).or_default();
            e.0 += n;
            e.1.insert(format!("{} / {src}", r.0));
            let u = prices.get(&r.3).map(|(p, _)| (p - r.6) * r.5 * if r.4 == "long" { 1.0 } else { -1.0 });
            match u {
                Some(x) => unreal += x,
                None => unpriced += 1,
            }
            contributors.push(json!({ "account_id": r.0, "source": src, "symbol": r.3, "side": r.4, "notional": n, "share_pct": if gross > 0.0 { (n / gross * 10000.0).round() / 100.0 } else { 0.0 }, "unrealised": u }));
        }
        contributors.sort_by(|a, b| b["notional"].as_f64().partial_cmp(&a["notional"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
        let shares: Vec<f64> = by_symbol.values().map(|(n, _)| if gross > 0.0 { n / gross } else { 0.0 }).collect();
        let hhi = shares.iter().map(|s| s * s).sum::<f64>();
        let overlaps: Vec<Value> = by_symbol.iter().filter(|(_, (_, srcs))| srcs.len() > 1).map(|(s, (n, srcs))| json!({ "symbol": s, "notional": n, "held_by": srcs })).collect();
        let (pe, pl, pdd) = portfolio_limits(conn, cur);
        let (eq, peak, dd) = demo_drawdown(conn, cur);
        let util = |lim: Option<Decimal>, v: f64| lim.and_then(|l| l.to_f64()).map(|l| (v / l * 10000.0).round() / 100.0);
        out_cur.insert(cur.to_string(), json!({
            "positions": mine.len(), "gross_exposure": gross, "net_exposure": net, "unrealised": unreal, "positions_without_price": unpriced,
            "largest_symbol_share_pct": shares.iter().copied().fold(0.0, f64::max) * 100.0, "concentration_hhi": (hhi * 1000.0).round() / 1000.0,
            "by_symbol": by_symbol.iter().map(|(s, (n, srcs))| json!({ "symbol": s, "notional": n, "sources": srcs })).collect::<Vec<_>>(),
            "overlapping_exposure": overlaps, "top_contributors": contributors.into_iter().take(10).collect::<Vec<_>>(),
            "demo_equity": dec_str(eq), "demo_peak_equity": dec_str(peak), "demo_drawdown_pct": (dd * 100.0).round() / 100.0,
            "limits": { "max_exposure": pe.map(dec_str), "max_daily_loss": pl.map(dec_str), "max_drawdown_pct": pdd.map(dec_str) },
            "utilisation_pct": { "exposure": util(pe, gross), "drawdown": pdd.and_then(|l| l.to_f64()).map(|l| (dd / l * 10000.0).round() / 100.0) },
        }));
    }
    let breaches: i64 = conn.query_row("SELECT count(*) FROM events WHERE type = 'RiskLimitBreached' AND ts_utc > ?1", [(chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339()], |r| r.get(0)).unwrap_or(0);
    let unreliable: Vec<String> = conn.prepare("SELECT id FROM accounts WHERE kind <> 'demo' AND archived = 0 AND last_error IS NOT NULL").and_then(|mut st| st.query_map([], |r| r.get(0)).map(|m| m.filter_map(Result::ok).collect())).unwrap_or_default();
    json!({
        "currencies": out_cur, "breaches_24h": breaches, "accounts_with_unreliable_data": unreliable,
        "not_available": ["leverage, margin and liquidation distance: ZoneBoard trades spot and paper only, so there is no margin data", "broker-account drawdown: no equity history is stored for broker accounts yet"],
        "notes": "USDT and INR are never added together. Exposure at entry prices; unrealised P&L at the latest price a bot saw.",
    })
}

/// Record a refused order as events (and a notification when a limit, not a validation, refused it).
pub fn on_rejection(conn: &Connection, source: &str, account: &str, symbol: &str, reason: &str) {
    let corr = format!("{source}:{account}:{symbol}");
    crate::events::emit(conn, "OrderRiskRejected", &format!("account:{account}"), json!({ "source": source, "symbol": symbol, "reason": reason }), Some(&corr), None);
    if reason.contains("limit") || reason.contains("kill switch") {
        crate::events::emit(conn, "RiskLimitBreached", &format!("account:{account}"), json!({ "source": source, "symbol": symbol, "reason": reason }), Some(&corr), None);
        let key = format!("breach:{account}:{}", reason.split(':').next().unwrap_or(reason));
        crate::events::notify(conn, "risk", &format!("Risk limit reached on {account}"), reason, Some(&key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn order(account: &str, symbol: &str, qty: f64) -> Order {
        Order { account_id: account.into(), market: "crypto".into(), symbol: symbol.into(), qty, price: 100.0, bot: None }
    }

    #[test]
    fn extra_and_portfolio_limits() {
        let mut c = crate::db::open(":memory:").unwrap();
        let g = crate::risk::Gate::default();
        crate::accounts::create(&mut c, &json!({ "label": "Second" })).unwrap();
        set_extra(&c, "demo-1", &json!({ "USDT": { "denied_symbols": "dogeusdt", "max_position_notional": "300", "max_open_positions": 2 } })).unwrap();
        assert!(crate::risk::check(&c, &order("demo-1", "DOGEUSDT", 1.0), &g).unwrap_err().contains("denied"));
        assert!(crate::risk::check(&c, &order("demo-1", "BTCUSDT", 4.0), &g).unwrap_err().contains("per-symbol"));
        for s in ["BTCUSDT", "ETHUSDT"] {
            c.execute("INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at, account_id) VALUES ('crypto','demo',?1,'long',1,100,0.1,'x','demo-1')", [s]).unwrap();
        }
        assert!(crate::risk::check(&c, &order("demo-1", "SOLUSDT", 1.0), &g).unwrap_err().contains("open positions"));
        // portfolio exposure spans accounts: demo-1 holds 200; demo-2 may add only 100 more
        set_portfolio(&c, &json!({ "USDT": { "max_exposure": "300" } })).unwrap();
        assert!(crate::risk::check(&c, &order("demo-2", "SOLUSDT", 2.0), &g).unwrap_err().contains("portfolio exposure"));
        assert!(crate::risk::check(&c, &order("demo-2", "SOLUSDT", 1.0), &g).is_ok());
        // drawdown: equity 20000 -> 18000 is 10%
        set_portfolio(&c, &json!({ "USDT": { "max_drawdown_pct": "5" } })).unwrap();
        demo_drawdown(&c, "USDT");
        c.execute("UPDATE demo_wallets SET balance = '9000' WHERE currency = 'USDT'", []).unwrap();
        assert!(crate::risk::check(&c, &order("demo-2", "SOLUSDT", 0.1), &g).unwrap_err().contains("drawdown"));
        let v = view(&c);
        assert_eq!(v["currencies"]["USDT"]["positions"], 2);
        assert!((v["currencies"]["USDT"]["demo_drawdown_pct"].as_f64().unwrap() - 10.0).abs() < 1e-9);
        assert!(set_portfolio(&c, &json!({ "USDT": { "max_drawdown_pct": "150" } })).is_err());
    }

    #[test]
    fn concurrent_orders_cannot_exceed_an_aggregate_limit() {
        let path = std::env::temp_dir().join(format!("zb_conc_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let j = Arc::new(crate::journal::Journal::open(path.to_str().unwrap()).unwrap());
        j.with_conn(|c| set_portfolio(c, &json!({ "USDT": { "max_exposure": "1000" } })).unwrap());
        let g = Arc::new(crate::risk::Gate::default());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (j, g) = (j.clone(), g.clone());
                std::thread::spawn(move || j.open_position_checked(&json!({ "market": "crypto", "mode": "demo", "symbol": "BTCUSDT", "side": "long", "qty": 3, "price": 100 }), |c, o| crate::risk::check(c, o, &g)).is_ok())
            })
            .collect();
        let ok = handles.into_iter().map(|h| h.join().unwrap()).filter(|x| *x).count();
        assert_eq!(ok, 3, "8 concurrent orders of 300 against a 1000 limit: exactly 3 fit");
        drop(j);
        let _ = std::fs::remove_file(&path);
    }
}
