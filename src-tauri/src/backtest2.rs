//! Backtester v2 for bot strategies: replays history through [`crate::botengine`], the same engine the demo
//! runner uses. The order-block backtest in `backtest.rs` (and its parity tests) is separate and unchanged.
//!
//! Money (P&L, fees, capital) is `Decimal`, through the same `accounts::realised` formula the paper account
//! uses, so a trade closes for the same amount here and on the demo account. Ratios (Sharpe, drawdown %) are
//! plain floats: they are statistics, not money.
//!
//! Optimisation is a capped grid search with an in-sample / out-of-sample split. Parameters are chosen on
//! the in-sample part only; the out-of-sample numbers are the ones to believe.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{json, Value};

use crate::botengine::{self, Book, Engine, Exec, Legs, Lot, OpenReq, State};
use crate::money::Money;
use crate::strategy::{self, Strategy};
use crate::zones::Frame;

pub const MAX_COMBINATIONS: usize = 200;
const MAX_LOG: usize = 200;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct BtTrade {
    pub leg: u8,
    pub side: &'static str,
    pub tag: String,
    pub qty: f64,
    pub entry_time: i64,
    pub entry: f64,
    pub exit_time: i64,
    pub exit: f64,
    /// Decimal strings, like every money value the API returns.
    pub fees: String,
    pub pnl: String,
    pub reason: String,
    pub bars: usize,
}

struct SimBook {
    capital0: Decimal,
    realised: Decimal,
    fee_pct: f64,
    lots: Vec<Lot>,
    next_id: i64,
    trades: Vec<BtTrade>,
    log: Vec<Value>,
    pnl_f: Vec<f64>,
    times: Vec<i64>,
}

fn dec(x: f64) -> Decimal {
    Money::from_legacy_f64(x).map(|m| m.0).unwrap_or(Decimal::ZERO)
}

impl SimBook {
    fn bars_between(&self, a: i64, b: i64) -> usize {
        let s = self.times.partition_point(|&t| t < a);
        let e = self.times.partition_point(|&t| t <= b);
        e.saturating_sub(s)
    }
}

impl Book for SimBook {
    fn lots(&self) -> Vec<Lot> {
        self.lots.clone()
    }
    fn capital(&self) -> f64 {
        (self.capital0 + self.realised).to_f64().unwrap_or(0.0)
    }
    fn exposure(&self) -> f64 {
        self.lots.iter().map(|l| l.qty * l.entry).sum()
    }
    fn open(&mut self, r: OpenReq) -> Result<i64, String> {
        if !(r.qty > 0.0 && r.price > 0.0 && r.qty.is_finite() && r.price.is_finite()) {
            return Err("invalid size or price".into());
        }
        self.next_id += 1;
        self.lots.push(Lot { id: self.next_id, leg: r.leg, side: r.side, qty: r.qty, entry: r.price, stop: r.stop, target: r.target, trail: r.trail, trail_ref: r.trail.map(|_| r.price), opened: r.time, tag: r.tag });
        Ok(self.next_id)
    }
    fn close(&mut self, id: i64, price: f64, time: i64, reason: &str) -> Result<(), String> {
        let i = self.lots.iter().position(|l| l.id == id).ok_or("no such lot")?;
        let l = self.lots.remove(i);
        // the paper account's formula: fee on each side's own notional, charged at close
        let fees = self.fee_pct / 100.0 * l.qty * (l.entry + price);
        let net = crate::accounts::realised(l.side.as_str(), l.qty, l.entry, price, fees).ok_or("P&L overflow")?;
        self.realised += net;
        self.pnl_f.push(net.to_f64().unwrap_or(0.0));
        let bars = self.bars_between(l.opened, time);
        self.trades.push(BtTrade {
            leg: l.leg,
            side: l.side.as_str(),
            tag: l.tag,
            qty: l.qty,
            entry_time: l.opened,
            entry: l.entry,
            exit_time: time,
            exit: price,
            fees: crate::accounts::dec_str(dec(fees).round_dp(8)),
            pnl: crate::accounts::dec_str(net),
            reason: reason.to_string(),
            bars,
        });
        Ok(())
    }
    fn update_trail(&mut self, id: i64, r: f64) {
        if let Some(l) = self.lots.iter_mut().find(|l| l.id == id) {
            l.trail_ref = Some(r);
        }
    }
    fn log(&mut self, level: &str, msg: String) {
        if self.log.len() < MAX_LOG {
            self.log.push(json!({ "level": level, "msg": msg }));
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub capital: f64,
    pub fee_pct: f64,
    pub slippage_pct: f64,
    pub qty_step: [f64; 2],
    /// First bar on which new positions may be opened (earlier bars only warm up indicators).
    pub trade_from: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Metrics {
    pub start_capital: String,
    pub end_capital: String,
    pub net_profit: String,
    pub return_pct: f64,
    pub buy_hold_pct: f64,
    pub trades: usize,
    pub wins: usize,
    pub losses: usize,
    pub win_rate: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub largest_win: f64,
    pub largest_loss: f64,
    /// Gross profit / gross loss. `None` when there were no losing trades (undefined, not infinite).
    pub profit_factor: Option<f64>,
    pub expectancy: f64,
    pub max_drawdown_pct: f64,
    pub max_drawdown: f64,
    /// Annualised from per-bar equity returns; `None` with too few bars or no variance.
    pub sharpe: Option<f64>,
    pub sortino: Option<f64>,
    pub exposure_pct: f64,
    pub avg_bars_held: f64,
    pub max_consecutive_losses: usize,
    pub bars: usize,
    pub fees: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub metrics: Metrics,
    pub trades: Vec<BtTrade>,
    /// (bar time, equity) at every bar from `trade_from`.
    pub equity: Vec<(i64, f64)>,
    pub log: Vec<Value>,
    pub open_lots: usize,
}

/// Run a strategy over closed bars. `b` is the second symbol for pairs (aligned here).
pub fn run(s: &Strategy, a: &Frame, b: Option<&Frame>, cfg: &Config) -> Result<Report, String> {
    let aligned;
    let (fa, fb) = match b {
        Some(b) => {
            aligned = botengine::align(a, b);
            (&aligned.0, Some(&aligned.1))
        }
        None => (a, None),
    };
    if fa.len() < 2 {
        return Err("not enough bars to backtest".into());
    }
    if !(cfg.capital > 0.0 && cfg.capital <= 1e15) {
        return Err("capital must be above 0".into());
    }
    let legs = Legs { a: fa, b: fb };
    let prep = botengine::prepare(s, &legs)?;
    let engine = Engine { s, exec: Exec { fee_pct: cfg.fee_pct, slippage_pct: cfg.slippage_pct, qty_step: cfg.qty_step } };
    let mut book = SimBook { capital0: dec(cfg.capital), realised: Decimal::ZERO, fee_pct: cfg.fee_pct, lots: vec![], next_id: 0, trades: vec![], log: vec![], pnl_f: vec![], times: fa.t.clone() };
    let mut st = State::default();
    let mut equity = Vec::with_capacity(fa.len());
    let mut in_market = 0usize;
    let from = cfg.trade_from.min(fa.len() - 1);
    for j in 0..fa.len() {
        if j > 0 {
            engine.on_open(&mut st, &mut book, &legs, j);
        }
        engine.on_close(&mut st, &mut book, &legs, &prep, j, j >= from);
        if j >= from {
            let unreal: f64 = book
                .lots
                .iter()
                .map(|l| {
                    let c = legs_close(&legs, l.leg, j);
                    l.side.sign() * (c - l.entry) * l.qty - cfg.fee_pct / 100.0 * l.qty * l.entry
                })
                .sum();
            if !book.lots.is_empty() {
                in_market += 1;
            }
            equity.push((fa.t[j], book.capital() + unreal));
        }
    }
    // Open lots at the end are marked to the last close in equity but are not trades: no exit happened.
    let open_lots = book.lots.len();
    let metrics = metrics(cfg, &book, &equity, fa, from, in_market);
    Ok(Report { metrics, trades: book.trades, equity, log: book.log, open_lots })
}

fn legs_close(legs: &Legs, leg: u8, j: usize) -> f64 {
    if leg == 1 {
        legs.b.map_or(f64::NAN, |b| b.c[j])
    } else {
        legs.a.c[j]
    }
}

fn r(x: f64, d: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let m = 10f64.powi(d);
    (x * m).round() / m
}

fn metrics(_cfg: &Config, book: &SimBook, equity: &[(i64, f64)], f: &Frame, from: usize, in_market: usize) -> Metrics {
    let pnl = &book.pnl_f;
    let wins: Vec<f64> = pnl.iter().copied().filter(|p| *p > 0.0).collect();
    let losses: Vec<f64> = pnl.iter().copied().filter(|p| *p <= 0.0).collect();
    let gross_w: f64 = wins.iter().sum();
    let gross_l: f64 = -losses.iter().sum::<f64>();
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let (mut peak, mut mdd, mut mdd_abs) = (f64::MIN, 0.0f64, 0.0f64);
    for &(_, e) in equity {
        peak = peak.max(e);
        if peak > 0.0 {
            mdd = mdd.max((peak - e) / peak);
            mdd_abs = mdd_abs.max(peak - e);
        }
    }
    let rets: Vec<f64> = equity.windows(2).filter(|w| w[0].1 > 0.0).map(|w| w[1].1 / w[0].1 - 1.0).collect();
    let span_years = match (equity.first(), equity.last()) {
        (Some(a), Some(b)) if b.0 > a.0 => (b.0 - a.0) as f64 / 31_557_600.0,
        _ => 0.0,
    };
    let per_year = if span_years > 0.0 { rets.len() as f64 / span_years } else { 0.0 };
    let (m, sd) = (mean(&rets), {
        let mu = mean(&rets);
        (rets.iter().map(|x| (x - mu).powi(2)).sum::<f64>() / rets.len().max(1) as f64).sqrt()
    });
    let sharpe = (rets.len() >= 10 && sd > 0.0 && per_year > 0.0).then(|| r(m / sd * per_year.sqrt(), 3));
    let down = (rets.iter().map(|x| x.min(0.0).powi(2)).sum::<f64>() / rets.len().max(1) as f64).sqrt();
    let sortino = (rets.len() >= 10 && down > 0.0 && per_year > 0.0).then(|| r(m / down * per_year.sqrt(), 3));
    let mut streak = 0;
    let mut worst = 0;
    for p in pnl {
        if *p <= 0.0 {
            streak += 1;
            worst = worst.max(streak);
        } else {
            streak = 0;
        }
    }
    let fees: Decimal = book.trades.iter().filter_map(|t| t.fees.parse::<Decimal>().ok()).sum();
    let end = book.capital0 + book.realised;
    let last = f.c[f.len() - 1];
    let first = f.c[from.min(f.len() - 1)];
    Metrics {
        start_capital: crate::accounts::dec_str(book.capital0),
        end_capital: crate::accounts::dec_str(end),
        net_profit: crate::accounts::dec_str(book.realised),
        return_pct: r((book.realised / book.capital0).to_f64().unwrap_or(0.0) * 100.0, 3),
        buy_hold_pct: r((last / first - 1.0) * 100.0, 3),
        trades: pnl.len(),
        wins: wins.len(),
        losses: losses.len(),
        win_rate: if pnl.is_empty() { 0.0 } else { r(wins.len() as f64 / pnl.len() as f64 * 100.0, 2) },
        avg_win: r(mean(&wins), 8),
        avg_loss: r(mean(&losses), 8),
        largest_win: r(wins.iter().copied().fold(0.0, f64::max), 8),
        largest_loss: r(losses.iter().copied().fold(0.0, f64::min), 8),
        profit_factor: (gross_l > 0.0).then(|| r(gross_w / gross_l, 3)),
        expectancy: r(mean(pnl), 8),
        max_drawdown_pct: r(mdd * 100.0, 3),
        max_drawdown: r(mdd_abs, 8),
        sharpe,
        sortino,
        exposure_pct: if equity.is_empty() { 0.0 } else { r(in_market as f64 / equity.len() as f64 * 100.0, 2) },
        avg_bars_held: r(mean(&book.trades.iter().map(|t| t.bars as f64).collect::<Vec<_>>()), 2),
        max_consecutive_losses: worst,
        bars: equity.len(),
        fees: crate::accounts::dec_str(fees),
    }
}

// ------------------------------------------------------------------ optimisation

/// One parameter to vary: a JSON pointer into the strategy spec (for example `/indicators/0/params/period`)
/// and the values to try.
pub struct Axis {
    pub path: String,
    pub values: Vec<Value>,
}

fn score(m: &Metrics, objective: &str) -> f64 {
    let v = match objective {
        "sharpe" => m.sharpe.unwrap_or(f64::NEG_INFINITY),
        "profit_factor" => m.profit_factor.unwrap_or(if m.trades > 0 && m.losses == 0 { 1e6 } else { f64::NEG_INFINITY }),
        "win_rate" => m.win_rate,
        "max_drawdown" => -m.max_drawdown_pct,
        _ => m.return_pct,
    };
    if m.trades == 0 { f64::NEG_INFINITY } else { v }
}

pub fn optimize(spec: &Value, axes: &[Axis], a: &Frame, b: Option<&Frame>, cfg: &Config, split: f64, objective: &str) -> Result<Value, String> {
    if axes.is_empty() {
        return Err("give at least one parameter to vary".into());
    }
    if !["return", "sharpe", "profit_factor", "win_rate", "max_drawdown"].contains(&objective) {
        return Err("objective must be return, sharpe, profit_factor, win_rate or max_drawdown".into());
    }
    if !(0.3..=0.9).contains(&split) {
        return Err("split must be between 0.3 and 0.9 (the in-sample share)".into());
    }
    let total: usize = axes.iter().map(|x| x.values.len().max(1)).product();
    if total > MAX_COMBINATIONS {
        return Err(format!("{total} combinations; at most {MAX_COMBINATIONS}. Use fewer values"));
    }
    for ax in axes {
        if spec.pointer(&ax.path).is_none() {
            return Err(format!("{} is not a field of this strategy", ax.path));
        }
        if ax.values.is_empty() {
            return Err(format!("{}: no values to try", ax.path));
        }
    }
    let n = a.len();
    let cut = ((n as f64) * split) as usize;
    if cut < 50 || n - cut < 20 {
        return Err("not enough bars for an in-sample / out-of-sample split".into());
    }
    let ins = slice(a, 0, cut);
    let ins_b = b.map(|b| slice(b, 0, b.t.partition_point(|&t| t < a.t[cut])));
    let mut rows = vec![];
    let mut idx = vec![0usize; axes.len()];
    for _ in 0..total {
        let mut v = spec.clone();
        let mut chosen = serde_json::Map::new();
        for (k, ax) in axes.iter().enumerate() {
            *v.pointer_mut(&ax.path).unwrap() = ax.values[idx[k]].clone();
            chosen.insert(ax.path.clone(), ax.values[idx[k]].clone());
        }
        // advance the odometer
        for k in (0..axes.len()).rev() {
            idx[k] += 1;
            if idx[k] < axes[k].values.len() {
                break;
            }
            idx[k] = 0;
        }
        let st = match strategy::parse(&v) {
            Ok(s) => s,
            Err(e) => {
                rows.push(json!({ "params": chosen, "error": e }));
                continue;
            }
        };
        let is = run(&st, &ins, ins_b.as_ref(), &Config { trade_from: 0, ..*cfg })?;
        // Out of sample: the whole history so indicators are warm, but trades only open after the cut.
        let oos = run(&st, a, b, &Config { trade_from: cut, ..*cfg })?;
        rows.push(json!({ "params": chosen, "score": score(&is.metrics, objective), "in_sample": is.metrics, "out_of_sample": oos.metrics }));
    }
    rows.sort_by(|x, y| {
        let s = |r: &Value| r["score"].as_f64().unwrap_or(f64::NEG_INFINITY);
        s(y).partial_cmp(&s(x)).unwrap_or(std::cmp::Ordering::Equal)
    });
    for r in &mut rows {
        if r["score"].as_f64().is_some_and(|s| !s.is_finite()) {
            r["score"] = Value::Null;
        }
    }
    Ok(json!({
        "combinations": total, "objective": objective,
        "in_sample": { "from": a.t[0], "to": a.t[cut - 1], "bars": cut },
        "out_of_sample": { "from": a.t[cut], "to": a.t[n - 1], "bars": n - cut },
        "results": rows,
        "note": "Ranked by the in-sample objective. Judge the winner by its out-of-sample numbers: in-sample results of an optimised strategy are flattering by construction.",
    }))
}

pub fn slice(f: &Frame, from: usize, to: usize) -> Frame {
    let mut out = Frame::default();
    for i in from..to.min(f.len()) {
        out.push(f.t[i], f.o[i], f.h[i], f.l[i], f.c[i], f.v[i]);
    }
    out
}

pub fn report_json(r: &Report) -> Value {
    json!({ "metrics": r.metrics, "trades": r.trades, "equity": r.equity, "log": r.log, "open_lots": r.open_lots })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> Config {
        Config { capital: 10_000.0, fee_pct: 0.0, slippage_pct: 0.0, qty_step: [0.000001, 0.000001], trade_from: 0 }
    }

    fn bars(rows: &[(f64, f64, f64, f64)]) -> Frame {
        let mut f = Frame::default();
        for (i, (o, h, l, c)) in rows.iter().enumerate() {
            f.push(1_700_000_000 + i as i64 * 3600, *o, *h, *l, *c, 1000.0);
        }
        f
    }

    /// Enters long on bar 1's close (close > 100.5), fills at bar 2's open.
    fn always_long(extra: Value) -> Strategy {
        let mut v = json!({ "entry": { "long": { "left": { "price": "close" }, "op": "gt", "right": { "value": 100.5 } } }, "sizing": { "type": "fixed_qty", "value": 1 } });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        strategy::parse(&v).unwrap()
    }

    #[test]
    fn signal_fills_at_the_next_open_not_the_signal_close() {
        let f = bars(&[(100.0, 100.0, 100.0, 100.0), (100.0, 102.0, 99.0, 101.0), (103.0, 104.0, 102.0, 103.0), (103.0, 103.0, 103.0, 103.0)]);
        let s = always_long(json!({ "exit": { "long": { "left": { "price": "close" }, "op": "gt", "right": { "value": 102.5 } } } }));
        let r = run(&s, &f, None, &cfg()).unwrap();
        // signal on bar 1 close 101 -> fill at bar 2 open 103; exit signal on bar 2 close -> fill at bar 3 open 103
        assert_eq!(r.trades.len(), 1);
        assert_eq!(r.trades[0].entry, 103.0);
        assert_eq!(r.trades[0].entry_time, f.t[2]);
        assert_eq!(r.trades[0].exit, 103.0);
    }

    #[test]
    fn stop_wins_when_stop_and_target_share_a_bar_and_gaps_fill_at_the_open() {
        let s = always_long(json!({ "stop_loss": { "type": "pct", "value": 2 }, "take_profit": { "type": "pct", "value": 2 } }));
        // entry 100 at bar 2 open; bar 3 spans 97..103: both hit -> stop at 98
        let f = bars(&[(100.0, 100.0, 100.0, 100.0), (100.0, 101.0, 100.0, 101.0), (100.0, 100.5, 99.5, 100.0), (100.0, 103.0, 97.0, 100.0)]);
        let r = run(&s, &f, None, &cfg()).unwrap();
        assert_eq!(r.trades[0].reason, "stop-loss");
        assert_eq!(r.trades[0].exit, 98.0);
        // gap below the stop: fills at the open (95), worse than the stop
        let g = bars(&[(100.0, 100.0, 100.0, 100.0), (100.0, 101.0, 100.0, 101.0), (100.0, 100.5, 99.5, 100.0), (95.0, 96.0, 94.0, 95.0)]);
        let r = run(&s, &g, None, &cfg()).unwrap();
        assert_eq!(r.trades[0].exit, 95.0);
    }

    #[test]
    fn trailing_stop_never_uses_the_same_bars_high() {
        let s = always_long(json!({ "trailing_stop": { "type": "pct", "value": 5 } }));
        // entry 100 at bar 2. Bar 3: high 120, low 98. If bar 3's own high raised the stop to 114 first, the
        // low 98 would stop us out at 114 - a fill that could never happen. The rule tests 95 (from 100).
        let f = bars(&[(100.0, 100.0, 100.0, 100.0), (100.0, 101.0, 100.0, 101.0), (100.0, 100.0, 100.0, 100.0), (100.0, 120.0, 98.0, 119.0), (119.0, 119.0, 113.0, 113.0)]);
        let r = run(&s, &f, None, &cfg()).unwrap();
        assert_eq!(r.trades.len(), 1, "still open after bar 3, stopped on bar 4");
        assert_eq!(r.trades[0].reason, "trailing stop");
        assert!((r.trades[0].exit - 114.0).abs() < 1e-9, "120 - 5% = 114 from bar 3's high, applied on bar 4");
    }

    #[test]
    fn fees_and_slippage_are_charged() {
        let s = always_long(json!({ "exit": { "long": { "left": { "price": "close" }, "op": "gt", "right": { "value": 102.5 } } } }));
        let f = bars(&[(100.0, 100.0, 100.0, 100.0), (100.0, 102.0, 99.0, 101.0), (103.0, 104.0, 102.0, 103.0), (103.0, 103.0, 103.0, 103.0)]);
        let c = Config { fee_pct: 0.1, slippage_pct: 0.1, ..cfg() };
        let r = run(&s, &f, None, &c).unwrap();
        let t = &r.trades[0];
        assert!((t.entry - 103.103).abs() < 1e-9 && (t.exit - 102.897).abs() < 1e-9);
        let pnl: f64 = t.pnl.parse().unwrap();
        let want = (102.897 - 103.103) - 0.001 * (103.103 + 102.897);
        assert!((pnl - want).abs() < 1e-7, "{pnl} vs {want}");
    }

    fn walk(n: usize, seed: u64) -> Frame {
        let mut s = seed;
        let mut x = 100.0;
        let mut f = Frame::default();
        for i in 0..n {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let r = (s % 1000) as f64 / 1000.0 - 0.5;
            let o = x;
            x *= 1.0 + r * 0.03;
            let sp = ((s >> 20) % 100) as f64 / 100.0 * 0.8;
            f.push(1_700_000_000 + i as i64 * 3600, o, o.max(x) + sp, o.min(x) - sp, x, 1000.0 + ((s >> 30) % 4000) as f64);
        }
        f
    }

    /// The backtest's anti-look-ahead property: changing bars after `k` cannot change anything that
    /// happened before bar `k`. Run for every template in the library.
    #[test]
    fn future_bars_never_change_past_trades() {
        let base = walk(600, 7);
        let other = walk(600, 99);
        for t in strategy::templates() {
            let s = strategy::parse(&t["spec"]).unwrap();
            let b = (s.kind == strategy::Kind::Pairs).then_some(&other);
            let full = run(&s, &base, b, &cfg()).unwrap();
            for k in [200usize, 400] {
                let mut changed = base.clone();
                for i in k..changed.len() {
                    changed.c[i] *= 1.3;
                    changed.h[i] *= 1.35;
                    changed.l[i] *= 0.7;
                    changed.o[i] *= 1.1;
                }
                let alt = run(&s, &changed, b, &cfg()).unwrap();
                let before = |r: &Report| r.trades.iter().filter(|t| t.exit_time < base.t[k]).cloned().collect::<Vec<_>>();
                assert_eq!(before(&full), before(&alt), "{}: trades closed before bar {k} changed when only later bars changed", t["id"]);
                let eq_before = |r: &Report| r.equity.iter().filter(|e| e.0 < base.t[k]).cloned().collect::<Vec<_>>();
                assert_eq!(eq_before(&full), eq_before(&alt), "{}: equity before bar {k} changed", t["id"]);
            }
        }
    }

    #[test]
    fn every_template_runs_and_metrics_are_consistent() {
        let a = walk(800, 3);
        let b = walk(800, 5);
        for t in strategy::templates() {
            let s = strategy::parse(&t["spec"]).unwrap();
            let r = run(&s, &a, (s.kind == strategy::Kind::Pairs).then_some(&b), &cfg()).unwrap();
            let m = &r.metrics;
            assert_eq!(m.wins + m.losses, m.trades, "{}", t["id"]);
            let sum: Decimal = r.trades.iter().map(|t| t.pnl.parse::<Decimal>().unwrap()).sum();
            assert_eq!(crate::accounts::dec_str(sum), m.net_profit, "{}: net profit is the exact sum of trade P&L", t["id"]);
            assert!(m.max_drawdown_pct >= 0.0 && m.max_drawdown_pct <= 100.0);
        }
    }

    #[test]
    fn grid_buys_lines_below_and_sells_one_line_up() {
        let s = strategy::parse(&json!({ "kind": "grid", "grid": { "lower": 90, "upper": 110, "levels": 5 } })).unwrap();
        // lines 90, 95, 100, 105, 110. Bar 0 sets the grid; bar 1 dips to 94 (buys 95 and 100... only lines
        // below the previous close 102: 100 and 95); bar 2 rises to 106 -> both sell (100->105, 95->100).
        let f = bars(&[(102.0, 102.0, 102.0, 102.0), (102.0, 102.0, 94.0, 96.0), (96.0, 106.0, 96.0, 104.0)]);
        let r = run(&s, &f, None, &cfg()).unwrap();
        let mut sells: Vec<(f64, f64)> = r.trades.iter().map(|t| (t.entry, t.exit)).collect();
        sells.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(sells, vec![(95.0, 100.0), (100.0, 105.0)]);
    }

    #[test]
    fn dca_averages_down_and_exits_at_take_profit_over_the_average() {
        let s = strategy::parse(&json!({ "kind": "dca", "dca": { "base_pct": 10, "safety_pct": 10, "safety_orders": 2, "step_pct": 5, "step_scale": 1, "volume_scale": 1, "take_profit_pct": 2, "restart": false } })).unwrap();
        // base at bar 1 open 100; SO1 at 95 on bar 2; TP 2% over the average hit on bar 3
        let f = bars(&[(100.0, 100.0, 100.0, 100.0), (100.0, 100.0, 99.0, 99.0), (99.0, 99.0, 94.0, 96.0), (96.0, 100.0, 96.0, 99.0)]);
        let r = run(&s, &f, None, &cfg()).unwrap();
        assert_eq!(r.trades.len(), 2);
        assert_eq!(r.trades.iter().map(|t| t.entry).collect::<Vec<_>>(), vec![100.0, 95.0]);
        // equal notional (1000 each): qty 10 at 100 and 10.526315 at 95
        let avg = (10.0 * 100.0 + 10.526315 * 95.0) / (10.0 + 10.526315);
        let tp = avg * 1.02;
        assert!(r.trades.iter().all(|t| (t.exit - tp).abs() < 1e-9 && t.reason == "dca take-profit"), "{:?} vs {tp}", r.trades);
    }

    #[test]
    fn pairs_open_both_legs_and_close_together() {
        let s = strategy::parse(&json!({ "kind": "pairs", "direction": "both", "pairs": { "lookback": 20, "entry_z": 1.5, "exit_z": 0.3 } })).unwrap();
        let b = walk(300, 11);
        let mut a = b.clone();
        // A tracks B, then jumps rich for a while and comes back
        for i in 0..a.len() {
            let k = if (150..160).contains(&i) { 1.08 } else { 1.0 };
            a.c[i] *= k;
            a.o[i] *= k;
            a.h[i] *= k;
            a.l[i] *= k;
        }
        let r = run(&s, &a, Some(&b), &cfg()).unwrap();
        assert!(r.trades.len() >= 2 && r.trades.len() % 2 == 0, "legs open and close in pairs: {:?}", r.trades.len());
        let first_a = r.trades.iter().find(|t| t.leg == 0).unwrap();
        let first_b = r.trades.iter().find(|t| t.leg == 1).unwrap();
        assert_ne!(first_a.side, first_b.side);
        assert_eq!(first_a.entry_time, first_b.entry_time);
    }

    #[test]
    fn optimizer_ranks_in_sample_and_reports_out_of_sample() {
        let a = walk(500, 21);
        let spec = strategy::templates().into_iter().find(|t| t["id"] == "ema_cross").unwrap()["spec"].clone();
        let axes = vec![Axis { path: "/indicators/0/params/period".into(), values: vec![json!(5), json!(9), json!(13)] }, Axis { path: "/indicators/1/params/period".into(), values: vec![json!(21), json!(34)] }];
        let out = optimize(&spec, &axes, &a, None, &cfg(), 0.7, "return").unwrap();
        assert_eq!(out["combinations"], 6);
        let res = out["results"].as_array().unwrap();
        assert_eq!(res.len(), 6);
        assert!(res.iter().all(|r| r.get("out_of_sample").is_some() || r.get("error").is_some()));
        assert!(optimize(&spec, &[Axis { path: "/nope".into(), values: vec![json!(1)] }], &a, None, &cfg(), 0.7, "return").is_err());
        let many = vec![Axis { path: "/indicators/0/params/period".into(), values: (1..=30).map(|x| json!(x)).collect() }, Axis { path: "/indicators/1/params/period".into(), values: (31..=40).map(|x| json!(x)).collect() }];
        assert!(optimize(&spec, &many, &a, None, &cfg(), 0.7, "return").unwrap_err().contains("at most"));
    }
}
