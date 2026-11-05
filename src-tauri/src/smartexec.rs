//! Smart order execution (paper only): split one parent order into child orders over time (TWAP) or into
//! equal slices, each child a separate paper fill that passes the same central risk check as a manual order.
//!
//! Lifecycle: a parent is `working` until every child is resolved, then `done` (all filled), `partial` (some
//! filled, some skipped or rejected) or `rejected` (none filled). `cancel` stops it (`cancelled`); pending
//! children are cancelled, filled ones stay. A rejected child stops the parent (`stopped`): a risk refusal is a
//! reason to stop, not to keep trying. On restart every working parent is `paused` (like bots) and the owner
//! resumes it; children whose time passed while paused are re-scheduled from the resume time.
//!
//! Price limit: with `limit_price`, a child whose current price is worse is `skipped` (not filled), which is how
//! partial completion happens on paper. Metrics: arrival price, average fill, implementation shortfall in basis
//! points (positive = cost), filled percentage. Broker (testnet/live) routing of child orders is not built:
//! paper accounts only, refused otherwise.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const ALGOS: [&str; 2] = ["twap", "slice"];
pub const MAX_CHILDREN: usize = 50;

fn iso(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Child quantities: equal parts rounded down to `step`, the last child takes the remainder. Never more than
/// `total`; every child at least one step (fewer children when the total is small).
pub fn plan(total: f64, n: usize, step: f64) -> Vec<f64> {
    let units = (total / step + 1e-9).floor() as u64;
    let n = (n as u64).clamp(1, units.max(1));
    let each = units / n;
    let mut v: Vec<f64> = (0..n).map(|_| each as f64 * step).collect();
    let rest = units - each * n;
    if let Some(l) = v.last_mut() {
        *l += rest as f64 * step;
    }
    v.into_iter().map(|q| (q / step).round() * step).map(|q| (q * 1e10).round() / 1e10).filter(|q| *q > 0.0).collect()
}

pub struct Req {
    pub account_id: String,
    pub market: String,
    pub symbol: String,
    pub side: String,
    pub total_qty: f64,
    pub algo: String,
    pub children: usize,
    pub interval_s: i64,
    pub limit_price: Option<f64>,
    pub arrival_price: f64,
}

pub fn parse(b: &Value) -> Result<Req, String> {
    let s = |k: &str, d: &str| b.get(k).and_then(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| d.to_string());
    let f = |k: &str| b.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|x| x.parse().ok())));
    let r = Req {
        account_id: s("account_id", crate::accounts::DEFAULT_ACCOUNT),
        market: s("market", "crypto"),
        symbol: s("symbol", "").to_uppercase(),
        side: s("side", "long"),
        total_qty: f("total_qty").ok_or("total_qty is required")?,
        algo: s("algo", "twap"),
        children: f("children").unwrap_or(5.0) as usize,
        interval_s: f("interval_s").unwrap_or(60.0) as i64,
        limit_price: f("limit_price"),
        arrival_price: f("price").ok_or("price (the price on screen) is required")?,
    };
    if !crate::journal::MARKETS.contains(&r.market.as_str()) || r.symbol.is_empty() {
        return Err("market and symbol are required".into());
    }
    if !["long", "short"].contains(&r.side.as_str()) {
        return Err("side must be long or short".into());
    }
    if !ALGOS.contains(&r.algo.as_str()) {
        return Err("algo must be twap or slice".into());
    }
    if !(2..=MAX_CHILDREN).contains(&r.children) {
        return Err(format!("children must be between 2 and {MAX_CHILDREN}"));
    }
    if !(5..=86_400).contains(&r.interval_s) {
        return Err("interval_s must be between 5 and 86400".into());
    }
    if !(r.total_qty > 0.0 && r.total_qty.is_finite() && r.arrival_price > 0.0 && r.arrival_price.is_finite()) {
        return Err("total_qty and price must be positive numbers".into());
    }
    if r.limit_price.is_some_and(|p| !(p > 0.0 && p.is_finite())) {
        return Err("limit_price must be a positive number".into());
    }
    Ok(r)
}

/// Create the parent and its schedule. `precheck` is the central risk check for the whole order at the arrival
/// price, so an order the account could never take is refused up front.
pub fn create(conn: &Connection, r: &Req, now: chrono::DateTime<chrono::Utc>, precheck: impl FnOnce(&Connection, &crate::risk::Order) -> Result<(), String>) -> Result<Value, String> {
    let (kind, env): (String, String) = conn.query_row("SELECT kind, environment FROM accounts WHERE id = ?1", [&r.account_id], |x| Ok((x.get(0)?, x.get(1)?))).map_err(|_| format!("unknown account {}", r.account_id))?;
    if kind != "demo" || env != "paper" {
        return Err("smart execution runs on paper (demo) accounts only; broker routing of child orders is not built".into());
    }
    precheck(conn, &crate::risk::Order { account_id: r.account_id.clone(), market: r.market.clone(), symbol: r.symbol.clone(), qty: r.total_qty, price: r.arrival_price, bot: None })?;
    let step = if r.market == "india" { 1.0 } else { crate::bots::qty_step(&r.market) };
    let qtys = plan(r.total_qty, r.children, step);
    if qtys.is_empty() {
        return Err("total_qty is smaller than one quantity step".into());
    }
    let p = json!({ "children": qtys.len(), "interval_s": r.interval_s, "limit_price": r.limit_price, "step": step });
    conn.execute(
        "INSERT INTO parent_orders (account_id, market, symbol, side, total_qty, algo, params, status, arrival_price, created_utc) VALUES (?1,?2,?3,?4,?5,?6,?7,'working',?8,?9)",
        params![r.account_id, r.market, r.symbol, r.side, r.total_qty, r.algo, p.to_string(), r.arrival_price, iso(now)],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    // twap spreads children over time; slice sends them back to back (one per tick)
    let gap = if r.algo == "twap" { r.interval_s } else { 0 };
    for (i, q) in qtys.iter().enumerate() {
        conn.execute("INSERT INTO child_orders (parent_id, seq, due_utc, qty, status) VALUES (?1,?2,?3,?4,'pending')", params![id, i as i64 + 1, iso(now + chrono::Duration::seconds(gap * i as i64)), q]).map_err(|e| e.to_string())?;
    }
    crate::events::emit(conn, "OrderProposed", &format!("parent:{id}"), json!({ "parent_id": id, "algo": r.algo, "symbol": r.symbol, "qty": r.total_qty, "children": qtys.len(), "paper": true }), None, None);
    let _ = crate::audit::record(conn, "user", "smart_order_created", &format!("parent_order:{id}"), &json!({ "algo": r.algo, "symbol": r.symbol, "qty": r.total_qty }));
    get(conn, id)
}

pub struct Due {
    pub child_id: i64,
    pub parent_id: i64,
    pub account_id: String,
    pub market: String,
    pub symbol: String,
    pub side: String,
    pub qty: f64,
    pub limit_price: Option<f64>,
}

/// Children due now on working parents, at most one per parent per call (children fill in order).
pub fn due(conn: &Connection, now: chrono::DateTime<chrono::Utc>) -> Vec<Due> {
    conn.prepare(
        "SELECT c.id, p.id, p.account_id, p.market, p.symbol, p.side, c.qty, p.params FROM child_orders c JOIN parent_orders p ON p.id = c.parent_id
         WHERE p.status = 'working' AND c.status = 'pending' AND c.due_utc <= ?1
           AND c.seq = (SELECT min(seq) FROM child_orders c2 WHERE c2.parent_id = p.id AND c2.status = 'pending') ORDER BY c.due_utc",
    )
    .and_then(|mut st| {
        st.query_map([iso(now)], |r| {
            let p: Value = serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(Value::Null);
            Ok(Due { child_id: r.get(0)?, parent_id: r.get(1)?, account_id: r.get(2)?, market: r.get(3)?, symbol: r.get(4)?, side: r.get(5)?, qty: r.get(6)?, limit_price: p["limit_price"].as_f64() })
        })
        .map(|m| m.filter_map(Result::ok).collect())
    })
    .unwrap_or_default()
}

/// Is `price` acceptable under the parent's limit? (long: not above; short: not below)
pub fn within_limit(side: &str, price: f64, limit: Option<f64>) -> bool {
    match limit {
        None => true,
        Some(l) if side == "long" => price <= l,
        Some(l) => price >= l,
    }
}

pub enum Outcome {
    Filled { position_id: i64, price: f64 },
    Skipped(String),
    Rejected(String),
}

/// Record a child's outcome and settle the parent when nothing is left.
pub fn record(conn: &Connection, d: &Due, o: Outcome, now: chrono::DateTime<chrono::Utc>) {
    let t = iso(now);
    match &o {
        Outcome::Filled { position_id, price } => {
            let _ = conn.execute("UPDATE child_orders SET status = 'filled', price = ?2, position_id = ?3, done_utc = ?4 WHERE id = ?1 AND status = 'pending'", params![d.child_id, price, position_id, t]);
            let (fq, avg): (f64, Option<f64>) = conn.query_row("SELECT filled_qty, avg_price FROM parent_orders WHERE id = ?1", [d.parent_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap_or((0.0, None));
            let nq = fq + d.qty;
            let navg = (avg.unwrap_or(0.0) * fq + price * d.qty) / nq;
            let _ = conn.execute("UPDATE parent_orders SET filled_qty = ?2, avg_price = ?3 WHERE id = ?1", params![d.parent_id, nq, navg]);
            crate::events::emit(conn, "OrderPartiallyFilled", &format!("parent:{}", d.parent_id), json!({ "parent_id": d.parent_id, "child_id": d.child_id, "qty": d.qty, "price": price, "paper": true }), None, Some(&format!("child:{}", d.child_id)));
        }
        Outcome::Skipped(why) => {
            let _ = conn.execute("UPDATE child_orders SET status = 'skipped', reason = ?2, done_utc = ?3 WHERE id = ?1 AND status = 'pending'", params![d.child_id, why, t]);
        }
        Outcome::Rejected(why) => {
            let _ = conn.execute("UPDATE child_orders SET status = 'rejected', reason = ?2, done_utc = ?3 WHERE id = ?1 AND status = 'pending'", params![d.child_id, why, t]);
            let _ = conn.execute("UPDATE child_orders SET status = 'cancelled', reason = 'parent stopped after a rejection', done_utc = ?2 WHERE parent_id = ?1 AND status = 'pending'", params![d.parent_id, t]);
            let _ = conn.execute("UPDATE parent_orders SET status = 'stopped', finished_utc = ?2, note = ?3 WHERE id = ?1", params![d.parent_id, t, format!("stopped: child refused by risk checks: {why}")]);
            crate::riskintel::on_rejection(conn, "smart_execution", &d.account_id, &d.symbol, why);
            return;
        }
    }
    let left: i64 = conn.query_row("SELECT count(*) FROM child_orders WHERE parent_id = ?1 AND status = 'pending'", [d.parent_id], |r| r.get(0)).unwrap_or(1);
    if left == 0 {
        let filled: i64 = conn.query_row("SELECT count(*) FROM child_orders WHERE parent_id = ?1 AND status = 'filled'", [d.parent_id], |r| r.get(0)).unwrap_or(0);
        let total: i64 = conn.query_row("SELECT count(*) FROM child_orders WHERE parent_id = ?1", [d.parent_id], |r| r.get(0)).unwrap_or(0);
        let st = if filled == total { "done" } else if filled == 0 { "rejected" } else { "partial" };
        let _ = conn.execute("UPDATE parent_orders SET status = ?2, finished_utc = ?3 WHERE id = ?1 AND status = 'working'", params![d.parent_id, st, t]);
        crate::events::emit(conn, if st == "done" { "OrderFilled" } else { "OrderPartiallyFilled" }, &format!("parent:{}", d.parent_id), json!({ "parent_id": d.parent_id, "status": st, "paper": true }), None, Some(&format!("parent-done:{}", d.parent_id)));
    }
}

pub fn cancel(conn: &Connection, id: i64) -> Result<Value, String> {
    let st: String = conn.query_row("SELECT status FROM parent_orders WHERE id = ?1", [id], |r| r.get(0)).map_err(|_| "no such parent order".to_string())?;
    if !["working", "paused"].contains(&st.as_str()) {
        return Err(format!("this order is already {st}"));
    }
    let t = iso(chrono::Utc::now());
    conn.execute("UPDATE child_orders SET status = 'cancelled', reason = 'cancelled by you', done_utc = ?2 WHERE parent_id = ?1 AND status = 'pending'", params![id, t]).map_err(|e| e.to_string())?;
    conn.execute("UPDATE parent_orders SET status = 'cancelled', finished_utc = ?2 WHERE id = ?1", params![id, t]).map_err(|e| e.to_string())?;
    let _ = crate::audit::record(conn, "user", "smart_order_cancelled", &format!("parent_order:{id}"), &json!({}));
    get(conn, id)
}

/// After a restart nothing resumes by itself.
pub fn pause_all_on_start(conn: &Connection) {
    let _ = conn.execute("UPDATE parent_orders SET status = 'paused', note = 'paused by a restart: resume it to continue' WHERE status = 'working'", []);
}

pub fn resume(conn: &Connection, id: i64, now: chrono::DateTime<chrono::Utc>) -> Result<Value, String> {
    let (st, algo, p): (String, String, String) = conn.query_row("SELECT status, algo, params FROM parent_orders WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|_| "no such parent order".to_string())?;
    if st != "paused" {
        return Err(format!("only a paused order can be resumed (this one is {st})"));
    }
    let gap = if algo == "twap" { serde_json::from_str::<Value>(&p).ok().and_then(|v| v["interval_s"].as_i64()).unwrap_or(60) } else { 0 };
    let pending: Vec<i64> = conn.prepare("SELECT id FROM child_orders WHERE parent_id = ?1 AND status = 'pending' ORDER BY seq").and_then(|mut s| s.query_map([id], |r| r.get(0)).map(|m| m.filter_map(Result::ok).collect())).map_err(|e| e.to_string())?;
    for (i, c) in pending.iter().enumerate() {
        conn.execute("UPDATE child_orders SET due_utc = ?2 WHERE id = ?1", params![c, iso(now + chrono::Duration::seconds(gap * i as i64))]).map_err(|e| e.to_string())?;
    }
    conn.execute("UPDATE parent_orders SET status = 'working', note = '' WHERE id = ?1", [id]).map_err(|e| e.to_string())?;
    get(conn, id)
}

pub fn get(conn: &Connection, id: i64) -> Result<Value, String> {
    let row = conn
        .query_row("SELECT account_id, market, symbol, side, total_qty, algo, params, status, arrival_price, filled_qty, avg_price, created_utc, finished_utc, note FROM parent_orders WHERE id = ?1", [id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, f64>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?, r.get::<_, Option<f64>>(8)?, r.get::<_, f64>(9)?, r.get::<_, Option<f64>>(10)?, r.get::<_, String>(11)?, r.get::<_, Option<String>>(12)?, r.get::<_, String>(13)?))
        })
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("no such parent order")?;
    let (acct, market, sym, side, total, algo, p, status, arrival, filled, avg, created, finished, note) = row;
    let children: Vec<Value> = conn
        .prepare("SELECT id, seq, due_utc, qty, status, price, position_id, reason, done_utc FROM child_orders WHERE parent_id = ?1 ORDER BY seq")
        .and_then(|mut st| {
            st.query_map([id], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "seq": r.get::<_, i64>(1)?, "due_utc": r.get::<_, String>(2)?, "qty": r.get::<_, f64>(3)?, "status": r.get::<_, String>(4)?, "price": r.get::<_, Option<f64>>(5)?, "position_id": r.get::<_, Option<i64>>(6)?, "reason": r.get::<_, String>(7)?, "done_utc": r.get::<_, Option<String>>(8)? })))
                .map(|m| m.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    let sign = if side == "long" { 1.0 } else { -1.0 };
    let shortfall = match (arrival, avg) {
        (Some(a), Some(f)) if a > 0.0 => Some(((f - a) / a * 10_000.0 * sign * 100.0).round() / 100.0),
        _ => None,
    };
    Ok(json!({
        "id": id, "account_id": acct, "market": market, "symbol": sym, "side": side, "total_qty": total, "algo": algo, "params": serde_json::from_str::<Value>(&p).unwrap_or(Value::Null),
        "status": status, "note": note, "created_utc": created, "finished_utc": finished, "children": children, "label": "PAPER (simulated fills)",
        "metrics": { "arrival_price": arrival, "avg_fill_price": avg, "filled_qty": filled, "filled_pct": (filled / total * 10_000.0).round() / 100.0, "implementation_shortfall_bps": shortfall,
            "note": "Shortfall is the average fill versus the arrival price, in basis points; positive means it cost you. Paper fills use the price on screen." },
    }))
}

pub fn list(conn: &Connection) -> Result<Value, String> {
    let ids: Vec<i64> = conn.prepare("SELECT id FROM parent_orders ORDER BY id DESC LIMIT 50").and_then(|mut s| s.query_map([], |r| r.get(0)).map(|m| m.filter_map(Result::ok).collect())).map_err(|e| e.to_string())?;
    Ok(json!({ "orders": ids.into_iter().filter_map(|i| get(conn, i).ok()).collect::<Vec<_>>(), "algos": ALGOS }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(qty: f64, limit: Option<f64>) -> Req {
        Req { account_id: "demo-1".into(), market: "crypto".into(), symbol: "BTCUSDT".into(), side: "long".into(), total_qty: qty, algo: "twap".into(), children: 4, interval_s: 60, limit_price: limit, arrival_price: 100.0 }
    }

    #[test]
    fn plans_sum_to_the_total_in_steps() {
        let p = plan(1.0, 3, 0.0001);
        assert_eq!(p.len(), 3);
        assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-9, "{p:?}");
        assert_eq!(plan(3.0, 10, 1.0), vec![1.0, 1.0, 1.0], "never smaller than a step");
        assert!(plan(0.00001, 3, 0.0001).is_empty());
    }

    #[test]
    fn lifecycle_partial_reject_cancel_and_restart() {
        let c = crate::db::open(":memory:").unwrap();
        crate::accounts::seed_from_legacy(&c).unwrap();
        let t0 = chrono::Utc::now();
        let p = create(&c, &req(0.4, Some(101.0)), t0, |_, _| Ok(())).unwrap();
        let id = p["id"].as_i64().unwrap();
        assert_eq!(p["children"].as_array().unwrap().len(), 4);
        // only the first child is due; one per parent per call
        let d = due(&c, t0);
        assert_eq!(d.len(), 1);
        record(&c, &d[0], Outcome::Filled { position_id: 1, price: 100.5 }, t0);
        assert!(due(&c, t0).is_empty(), "the next child waits its turn");
        let t1 = t0 + chrono::Duration::seconds(60);
        let d = due(&c, t1);
        assert!(!within_limit("long", 102.0, d[0].limit_price));
        record(&c, &d[0], Outcome::Skipped("price 102 above the limit 101".into()), t1);
        // restart: the order pauses and resumes on the owner's word
        pause_all_on_start(&c);
        assert!(due(&c, t0 + chrono::Duration::hours(1)).is_empty());
        let t2 = t0 + chrono::Duration::hours(1);
        resume(&c, id, t2).unwrap();
        for k in 0..2 {
            let d = due(&c, t2 + chrono::Duration::seconds(60 * k));
            record(&c, &d[0], Outcome::Filled { position_id: 2 + k, price: 100.0 }, t2);
        }
        let g = get(&c, id).unwrap();
        assert_eq!(g["status"], "partial");
        assert!((g["metrics"]["filled_qty"].as_f64().unwrap() - 0.3).abs() < 1e-9);
        assert!(g["metrics"]["implementation_shortfall_bps"].as_f64().unwrap() > 0.0, "{g}");
        // a rejection stops the parent; a cancel cancels the rest
        let p2 = create(&c, &req(0.4, None), t0, |_, _| Ok(())).unwrap();
        let d = due(&c, t0);
        record(&c, &d[0], Outcome::Rejected("daily loss limit reached".into()), t0);
        let g2 = get(&c, p2["id"].as_i64().unwrap()).unwrap();
        assert_eq!(g2["status"], "stopped");
        assert!(g2["children"].as_array().unwrap().iter().skip(1).all(|x| x["status"] == "cancelled"));
        let p3 = create(&c, &req(0.4, None), t0, |_, _| Ok(())).unwrap();
        let g3 = cancel(&c, p3["id"].as_i64().unwrap()).unwrap();
        assert_eq!(g3["status"], "cancelled");
        assert!(cancel(&c, p3["id"].as_i64().unwrap()).is_err());
        // the up-front risk check and paper-only rule
        assert!(create(&c, &req(0.4, None), t0, |_, _| Err("too big".into())).is_err());
        let mut r = req(0.4, None);
        r.account_id = "nope".into();
        assert!(create(&c, &r, t0, |_, _| Ok(())).is_err());
    }
}
