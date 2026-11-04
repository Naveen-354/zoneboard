//! The bot engine: one bar-by-bar state machine shared by the backtester and the live demo runner, so a bot
//! does in a demo forward test exactly what it did in its backtest.
//!
//! Execution rules (the same everywhere, chosen to avoid look-ahead and flattering fills):
//!  * Signals are evaluated on a bar's **close**, using indicators computed on closed bars only, and fill at
//!    the **next bar's open** (`on_open`), with slippage against you.
//!  * Stops and targets are checked on each closed bar's high and low. When both are inside the same bar the
//!    **stop wins** (we cannot know which came first). A gap through a stop fills at the open, not the stop.
//!  * Trailing stops move with highs (lows for shorts) of bars **already finished**: a bar's own high never
//!    raises the stop that the same bar's low is tested against.
//!  * Grid and DCA orders rest at prices fixed before the bar starts; they fill at that price (or at the open
//!    if the bar gaps through it). A lot bought inside a bar cannot be sold in the same bar.
//!
//! The engine never touches a database or a network: it talks to a [`Book`], which the backtester implements
//! in memory and the runner implements on the order manager (with every risk check).

use serde::{Deserialize, Serialize};

use crate::indicators::{self, Series};
use crate::strategy::{Ctx, Direction, Kind, Level, Sizing, Strategy};
pub use crate::strategy::Side;
use crate::zones::Frame;

/// A position the bot holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lot {
    pub id: i64,
    /// 0 = the bot's symbol, 1 = the second symbol of a pairs bot.
    pub leg: u8,
    pub side: Side,
    pub qty: f64,
    pub entry: f64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
    /// Trailing distance: a percent of the running extreme, or a fixed price distance (ATR at entry).
    pub trail: Option<Dist>,
    pub trail_ref: Option<f64>,
    /// Bar time (unix seconds) of the bar the lot was filled in.
    pub opened: i64,
    pub tag: String,
}

#[derive(Clone, Debug)]
pub struct OpenReq {
    pub leg: u8,
    pub side: Side,
    pub qty: f64,
    pub price: f64,
    pub time: i64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
    pub trail: Option<Dist>,
    pub tag: String,
}

/// Where the engine's orders go. Errors are refusals (risk limits, funds): the book records them; the
/// engine carries on.
pub trait Book {
    fn lots(&self) -> Vec<Lot>;
    /// Allocation plus realised P&L: what sizing percentages are taken of.
    fn capital(&self) -> f64;
    /// Notional of open lots at their entry prices.
    fn exposure(&self) -> f64;
    fn open(&mut self, req: OpenReq) -> Result<i64, String>;
    fn close(&mut self, lot: i64, price: f64, time: i64, reason: &str) -> Result<(), String>;
    fn update_trail(&mut self, lot: i64, trail_ref: f64);
    fn log(&mut self, level: &str, msg: String);
}

/// A stop or target distance captured at signal time, applied to the actual fill price.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Dist {
    Pct(f64),
    Abs(f64),
}

impl Dist {
    pub fn of(self, fill: f64) -> f64 {
        match self {
            Dist::Pct(p) => fill * p / 100.0,
            Dist::Abs(a) => a,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Pending {
    Open { leg: u8, side: Side, stop: Option<Dist>, target: Option<Dist>, trail: Option<Dist>, notional: Option<f64>, tag: String, reason: String },
    OpenPair { side_a: Side, reason: String },
    Close { lot: i64, reason: String },
    CloseAll { reason: String },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DcaCycle {
    pub base_price: f64,
    pub filled_so: usize,
}

/// Everything the engine remembers between bars. The runner stores it as JSON with the bot.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub pending: Vec<Pending>,
    pub grid_lines: Vec<f64>,
    pub dca: Option<DcaCycle>,
    pub halted: bool,
    pub last_closed: Option<i64>,
    pub last_opened: Option<i64>,
}

#[derive(Clone, Copy, Debug)]
pub struct Exec {
    /// Percent of notional per side, charged by the book.
    pub fee_pct: f64,
    /// Percent the market fills move against you.
    pub slippage_pct: f64,
    /// Quantity step of each leg: 1 for NSE shares, 0.000001 for crypto.
    pub qty_step: [f64; 2],
    /// Partial-fill model: a market entry fills at most this percent of the bar's volume; the rest is
    /// cancelled. `None` = unlimited. Exits always fill in full (getting out is never left half done).
    pub max_volume_pct: Option<f64>,
}

/// Price bars of the bot's symbol (and the second symbol, aligned bar for bar).
pub struct Legs<'a> {
    pub a: &'a Frame,
    pub b: Option<&'a Frame>,
}

impl Legs<'_> {
    fn f(&self, leg: u8) -> &Frame {
        if leg == 1 {
            self.b.unwrap_or(self.a)
        } else {
            self.a
        }
    }
    pub fn len(&self) -> usize {
        self.a.len()
    }
    pub fn is_empty(&self) -> bool {
        self.a.len() == 0
    }
}

/// Keep only the bars both frames have, in time order. Pairs need the same bars on both legs.
pub fn align(a: &Frame, b: &Frame) -> (Frame, Frame) {
    let (mut x, mut y) = (Frame::default(), Frame::default());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a.t[i].cmp(&b.t[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                x.push(a.t[i], a.o[i], a.h[i], a.l[i], a.c[i], a.v[i]);
                y.push(b.t[j], b.o[j], b.h[j], b.l[j], b.c[j], b.v[j]);
                i += 1;
                j += 1;
            }
        }
    }
    (x, y)
}

/// Z-score of the pair's spread at each bar, and the hedge ratio used. Causal: bar `i` uses bars `i-n+1..=i`.
/// Ratio mode: spread = ln A - ln B. OLS mode: residual of ln A regressed on ln B over the window.
pub fn pairs_z(a: &Frame, b: &Frame, n: usize, ols: bool) -> (Series, Series) {
    let len = a.len().min(b.len());
    let (la, lb): (Series, Series) = ((0..len).map(|i| a.c[i].ln()).collect(), (0..len).map(|i| b.c[i].ln()).collect());
    let (mut z, mut beta) = (vec![f64::NAN; len], vec![f64::NAN; len]);
    for i in n.saturating_sub(1)..len {
        let (wa, wb) = (&la[i + 1 - n..=i], &lb[i + 1 - n..=i]);
        let (ma, mb) = (wa.iter().sum::<f64>() / n as f64, wb.iter().sum::<f64>() / n as f64);
        let (bt, al) = if ols {
            let cov: f64 = wa.iter().zip(wb).map(|(x, y)| (x - ma) * (y - mb)).sum();
            let var: f64 = wb.iter().map(|y| (y - mb).powi(2)).sum();
            if var <= 0.0 {
                continue;
            }
            let bt = cov / var;
            (bt, ma - bt * mb)
        } else {
            (1.0, 0.0)
        };
        let res: Series = wa.iter().zip(wb).map(|(x, y)| x - bt * y - al).collect();
        let m = res.iter().sum::<f64>() / n as f64;
        let sd = (res.iter().map(|r| (r - m).powi(2)).sum::<f64>() / n as f64).sqrt();
        if sd > 0.0 && sd.is_finite() {
            z[i] = (res[n - 1] - m) / sd;
            beta[i] = bt;
        }
    }
    (z, beta)
}

/// Series the engine reads on a bar's close, built from closed bars.
pub struct Prepared {
    pub ctx: Ctx,
    pub pair_z: Option<Series>,
}

pub fn prepare(s: &Strategy, legs: &Legs) -> Result<Prepared, String> {
    let ctx = Ctx::build(s, legs.a)?;
    let pair_z = match (&s.pairs, legs.b) {
        (Some(p), Some(b)) => Some(pairs_z(legs.a, b, p.lookback, p.ols).0),
        (Some(_), None) => return Err("a pairs strategy needs a second symbol".into()),
        _ => None,
    };
    Ok(Prepared { ctx, pair_z })
}

pub struct Engine<'a> {
    pub s: &'a Strategy,
    pub exec: Exec,
}

fn floor_step(q: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return q;
    }
    let n = (q / step + 1e-9).floor();
    // round away float noise from the multiplication, keeping at most 8 decimals
    ((n * step) * 1e8).round() / 1e8
}

fn level_dist(ctx: &Ctx, l: &Option<Level>, i: usize) -> Option<Dist> {
    match l.as_ref()? {
        Level::Pct(p) => Some(Dist::Pct(*p)),
        a @ Level::Atr { .. } => ctx.distance(a, 1.0, i).map(Dist::Abs),
    }
}

impl Engine<'_> {
    fn buy(&self, p: f64) -> f64 {
        p * (1.0 + self.exec.slippage_pct / 100.0)
    }
    fn sell(&self, p: f64) -> f64 {
        p * (1.0 - self.exec.slippage_pct / 100.0)
    }
    /// Market fill price: opening a long or closing a short buys.
    fn market(&self, side: Side, opening: bool, p: f64) -> f64 {
        if (side == Side::Long) == opening {
            self.buy(p)
        } else {
            self.sell(p)
        }
    }

    fn qty_for(&self, book: &dyn Book, leg: u8, price: f64, notional: Option<f64>, stop_d: Option<f64>) -> f64 {
        let free = (book.capital() - book.exposure()).max(0.0);
        let want = match (notional, &self.s.sizing) {
            (Some(n), _) => n / price,
            (None, Sizing::PctCapital(p)) => book.capital() * p / 100.0 / price,
            (None, Sizing::FixedNotional(n)) => n / price,
            (None, Sizing::FixedQty(q)) => *q,
            (None, Sizing::RiskPct(p)) => match stop_d {
                Some(d) if d > 0.0 => book.capital() * p / 100.0 / d,
                _ => 0.0,
            },
        };
        let capped = want.min(free / price); // never more than the bot's unused capital
        floor_step(capped.max(0.0), self.exec.qty_step[leg as usize])
    }

    /// Fill the orders decided on the previous close at bar `j`'s open. Closes first, so a reversal frees
    /// its capital before the new side is sized.
    pub fn on_open(&self, st: &mut State, book: &mut dyn Book, legs: &Legs, j: usize) {
        let t = legs.a.t[j];
        let pending = std::mem::take(&mut st.pending);
        let (closes, opens): (Vec<_>, Vec<_>) = pending.into_iter().partition(|p| matches!(p, Pending::Close { .. } | Pending::CloseAll { .. }));
        for p in closes {
            let ids: Vec<(i64, String)> = match &p {
                Pending::Close { lot, reason } => vec![(*lot, reason.clone())],
                Pending::CloseAll { reason } => book.lots().iter().map(|l| (l.id, reason.clone())).collect(),
                _ => vec![],
            };
            let lots = book.lots();
            for (id, reason) in ids {
                if let Some(l) = lots.iter().find(|l| l.id == id) {
                    let px = self.market(l.side, false, legs.f(l.leg).o[j]);
                    if let Err(e) = book.close(id, px, t, &reason) {
                        book.log("error", format!("close of lot {id} failed: {e}"));
                    }
                }
            }
        }
        if st.halted {
            st.last_opened = Some(t);
            return;
        }
        for p in opens {
            match p {
                Pending::Open { leg, side, stop, target, trail, notional, tag, reason } => {
                    let fill = self.market(side, true, legs.f(leg).o[j]);
                    let stop_d = stop.map(|d| d.of(fill));
                    let mut qty = self.qty_for(book, leg, fill, notional, stop_d);
                    if let Some(p) = self.exec.max_volume_pct {
                        let cap = floor_step(legs.f(leg).v[j] * p / 100.0, self.exec.qty_step[leg as usize]);
                        if qty > cap {
                            book.log("warn", format!("{reason}: partial fill, {cap} of {qty} ({p}% of the bar's volume); the rest is cancelled"));
                            qty = cap;
                        }
                    }
                    if qty <= 0.0 {
                        book.log("warn", format!("{reason}: skipped, the order size rounds to zero with the capital left"));
                        continue;
                    }
                    let sg = side.sign();
                    let req = OpenReq {
                        leg,
                        side,
                        qty,
                        price: fill,
                        time: t,
                        stop: stop_d.map(|d| fill - sg * d),
                        target: target.map(|d| fill + sg * d.of(fill)),
                        trail,
                        tag: tag.clone(),
                    };
                    match book.open(req) {
                        Ok(_) => {
                            if tag == "dca:base" {
                                st.dca = Some(DcaCycle { base_price: fill, filled_so: 0 });
                            }
                        }
                        Err(e) => book.log("warn", format!("{reason}: order refused: {e}")),
                    }
                }
                Pending::OpenPair { side_a, reason } => {
                    let Some(fb) = legs.b else { continue };
                    let pa = self.market(side_a, true, legs.a.o[j]);
                    let side_b = if side_a == Side::Long { Side::Short } else { Side::Long };
                    let pb = self.market(side_b, true, fb.o[j]);
                    let total = match self.s.sizing {
                        Sizing::FixedNotional(n) => n,
                        Sizing::PctCapital(p) => book.capital() * p / 100.0,
                        _ => book.capital(),
                    }
                    .min((book.capital() - book.exposure()).max(0.0));
                    let qa = floor_step(total / 2.0 / pa, self.exec.qty_step[0]);
                    let qb = floor_step(total / 2.0 / pb, self.exec.qty_step[1]);
                    if qa <= 0.0 || qb <= 0.0 {
                        book.log("warn", format!("{reason}: skipped, a leg's size rounds to zero"));
                        continue;
                    }
                    let mk = |leg, side, qty, price| OpenReq { leg, side, qty, price, time: t, stop: None, target: None, trail: None, tag: "pair".into() };
                    match book.open(mk(0, side_a, qa, pa)) {
                        Ok(first) => {
                            if let Err(e) = book.open(mk(1, side_b, qb, pb)) {
                                // never leave half a pair open
                                book.log("warn", format!("{reason}: second leg refused ({e}); closing the first leg"));
                                let _ = book.close(first, self.market(side_a, false, legs.a.o[j]), t, "pair leg refused");
                            }
                        }
                        Err(e) => book.log("warn", format!("{reason}: order refused: {e}")),
                    }
                }
                _ => {}
            }
        }
        st.last_opened = Some(t);
    }

    /// Stops, targets and trailing stops on bar `j`'s range. Returns the lots still open.
    fn protect(&self, book: &mut dyn Book, legs: &Legs, j: usize) {
        let t = legs.a.t[j];
        for l in book.lots() {
            let f = legs.f(l.leg);
            let (o, h, lo) = (f.o[j], f.h[j], f.l[j]);
            let long = l.side == Side::Long;
            let trail_stop = match (l.trail_ref, l.trail) {
                (Some(r), Some(d)) => Some(if long { r - d.of(r) } else { r + d.of(r) }),
                _ => None,
            };
            let eff = match (l.stop, trail_stop) {
                (Some(a), Some(b)) => Some(if long { a.max(b) } else { a.min(b) }),
                (a, b) => a.or(b),
            };
            let by_trail = trail_stop.is_some() && eff == trail_stop && l.stop != eff;
            if let Some(s) = eff {
                let hit = if long { lo <= s } else { h >= s };
                if hit {
                    let base = if long { s.min(o) } else { s.max(o) }; // a gap fills at the open, worse than the stop
                    let px = self.market(l.side, false, base);
                    let why = if by_trail { "trailing stop" } else { "stop-loss" };
                    if let Err(e) = book.close(l.id, px, t, why) {
                        book.log("error", format!("{why} close failed: {e}"));
                    }
                    continue;
                }
            }
            if let Some(tg) = l.target {
                let hit = if long { h >= tg } else { lo <= tg };
                if hit {
                    let px = if long { tg.max(o) } else { tg.min(o) }; // resting limit: the target, or better on a gap
                    if let Err(e) = book.close(l.id, px, t, "take-profit") {
                        book.log("error", format!("take-profit close failed: {e}"));
                    }
                    continue;
                }
            }
            if l.trail.is_some() {
                let r = l.trail_ref.unwrap_or(l.entry);
                let nr = if long { r.max(h) } else { r.min(lo) };
                if nr != r {
                    book.update_trail(l.id, nr);
                }
            }
        }
    }

    fn held_bars(&self, legs: &Legs, j: usize, opened: i64) -> usize {
        let start = legs.a.t.partition_point(|&x| x < opened);
        (j + 1).saturating_sub(start)
    }

    /// Everything that happens on bar `j`'s close. `trade` = false still manages open lots but opens nothing
    /// new (the backtest's in-sample / out-of-sample split, and a paused start).
    pub fn on_close(&self, st: &mut State, book: &mut dyn Book, legs: &Legs, prep: &Prepared, j: usize, trade: bool) {
        let t = legs.a.t[j];
        self.protect(book, legs, j);
        if !st.halted {
            match self.s.kind {
                Kind::Signal => self.signal(st, book, legs, prep, j, trade),
                Kind::Grid => self.grid(st, book, legs, j, trade),
                Kind::Dca => self.dca(st, book, legs, prep, j, trade),
                Kind::Pairs => self.pairs(st, book, legs, prep, j, trade),
            }
        }
        if self.s.max_hold_bars > 0 {
            for l in book.lots() {
                let queued = st.pending.iter().any(|p| matches!(p, Pending::Close { lot, .. } if *lot == l.id) || matches!(p, Pending::CloseAll { .. }));
                if !queued && self.held_bars(legs, j, l.opened) >= self.s.max_hold_bars {
                    st.pending.push(Pending::Close { lot: l.id, reason: format!("held {} bars", self.s.max_hold_bars) });
                }
            }
        }
        st.last_closed = Some(t);
    }

    fn signal(&self, st: &mut State, book: &mut dyn Book, _legs: &Legs, prep: &Prepared, j: usize, trade: bool) {
        let ctx = &prep.ctx;
        let lots = book.lots();
        let mut closing: Vec<i64> = vec![];
        for l in &lots {
            let exit = match l.side {
                Side::Long => self.s.exit_long.as_ref(),
                Side::Short => self.s.exit_short.as_ref(),
            };
            if exit.is_some_and(|c| ctx.eval(c, j)) {
                st.pending.push(Pending::Close { lot: l.id, reason: "exit signal".into() });
                closing.push(l.id);
            }
        }
        if !trade {
            return;
        }
        let allow_long = matches!(self.s.direction, Direction::Long | Direction::Both);
        let allow_short = matches!(self.s.direction, Direction::Short | Direction::Both);
        let want_long = allow_long && self.s.entry_long.as_ref().is_some_and(|c| ctx.eval(c, j));
        let want_short = allow_short && self.s.entry_short.as_ref().is_some_and(|c| ctx.eval(c, j));
        let side = match (want_long, want_short) {
            (true, false) => Side::Long,
            (false, true) => Side::Short,
            _ => return, // nothing, or contradictory signals
        };
        let open_same = lots.iter().any(|l| l.side == side && !closing.contains(&l.id));
        let exiting_same = lots.iter().any(|l| l.side == side && closing.contains(&l.id));
        if open_same || exiting_same {
            return; // no pyramiding, and no re-entry on the bar we exit
        }
        for l in lots.iter().filter(|l| l.side != side && !closing.contains(&l.id)) {
            st.pending.push(Pending::Close { lot: l.id, reason: "reversed by an opposite signal".into() });
        }
        st.pending.push(Pending::Open {
            leg: 0,
            side,
            stop: level_dist(ctx, &self.s.stop_loss, j),
            target: level_dist(ctx, &self.s.take_profit, j),
            trail: level_dist(ctx, &self.s.trailing_stop, j),
            notional: None,
            tag: "signal".into(),
            reason: format!("{} entry signal", side.as_str()),
        });
    }

    fn grid(&self, st: &mut State, book: &mut dyn Book, legs: &Legs, j: usize, trade: bool) {
        let g = self.s.grid.as_ref().expect("validated");
        let f = legs.a;
        if st.grid_lines.is_empty() {
            let (lo, hi) = match (g.lower, g.upper) {
                (Some(l), Some(u)) => (l, u),
                _ => (f.c[j] * (1.0 - g.range_pct / 100.0), f.c[j] * (1.0 + g.range_pct / 100.0)),
            };
            let n = g.levels;
            st.grid_lines = (0..n).map(|k| lo + (hi - lo) * k as f64 / (n - 1) as f64).collect();
            book.log("info", format!("grid set: {n} lines from {lo:.8} to {hi:.8}"));
            return; // orders rest from the next bar
        }
        if let Some(sb) = g.stop_below_pct {
            if f.c[j] < st.grid_lines[0] * (1.0 - sb / 100.0) {
                st.pending.push(Pending::CloseAll { reason: "grid stop: price left the range".into() });
                st.halted = true;
                book.log("warn", "grid halted: price closed below the stop; open lots close at the next open".into());
                return;
            }
        }
        if !trade || j == 0 {
            return;
        }
        let prev_close = f.c[j - 1];
        let held: Vec<String> = book.lots().into_iter().map(|l| l.tag).collect();
        let cells = (st.grid_lines.len() - 1) as f64;
        for k in 0..st.grid_lines.len() - 1 {
            let line = st.grid_lines[k];
            let tag = format!("grid:{k}");
            // a buy rests at every line below the last close that has no lot yet
            if line < prev_close && f.l[j] <= line && !held.contains(&tag) {
                let px = line.min(f.o[j]);
                let notional = book.capital() / cells;
                let qty = self.qty_for(book, 0, px, Some(notional), None);
                if qty <= 0.0 {
                    book.log("warn", format!("grid line {k}: size rounds to zero"));
                    continue;
                }
                let req = OpenReq { leg: 0, side: Side::Long, qty, price: px, time: f.t[j], stop: None, target: Some(st.grid_lines[k + 1]), trail: None, tag };
                if let Err(e) = book.open(req) {
                    book.log("warn", format!("grid buy at line {k} refused: {e}"));
                }
            }
        }
    }

    fn dca(&self, st: &mut State, book: &mut dyn Book, legs: &Legs, prep: &Prepared, j: usize, trade: bool) {
        let d = self.s.dca.as_ref().expect("validated");
        let f = legs.a;
        let lots = book.lots();
        if lots.is_empty() {
            st.dca = None;
            let waiting = st.pending.iter().any(|p| matches!(p, Pending::Open { .. }));
            let go = self.s.entry_long.as_ref().is_none_or(|c| prep.ctx.eval(c, j));
            if trade && go && !waiting {
                let notional = book.capital() * d.base_pct / 100.0;
                st.pending.push(Pending::Open { leg: 0, side: Side::Long, stop: None, target: None, trail: None, notional: Some(notional), tag: "dca:base".into(), reason: "dca base order".into() });
            }
            return;
        }
        let Some(cycle) = st.dca.clone() else {
            st.dca = Some(DcaCycle { base_price: lots[0].entry, filled_so: lots.len().saturating_sub(1) });
            return;
        };
        let qty: f64 = lots.iter().map(|l| l.qty).sum();
        let avg = lots.iter().map(|l| l.qty * l.entry).sum::<f64>() / qty;
        let end = |st: &mut State, book: &mut dyn Book, why: &str| {
            st.dca = None;
            if !d.restart {
                st.halted = true;
                book.log("info", format!("dca cycle ended ({why}); restart is off, so the bot stops trading"));
            }
        };
        if let Some(sl) = d.stop_loss_pct {
            let stop = avg * (1.0 - sl / 100.0);
            if f.l[j] <= stop {
                let px = self.sell(stop.min(f.o[j]));
                for l in &lots {
                    let _ = book.close(l.id, px, f.t[j], "dca stop-loss");
                }
                return end(st, book, "stop-loss");
            }
        }
        let tp = avg * (1.0 + d.take_profit_pct / 100.0);
        if f.h[j] >= tp {
            let px = tp.max(f.o[j]);
            for l in &lots {
                let _ = book.close(l.id, px, f.t[j], "dca take-profit");
            }
            return end(st, book, "take-profit");
        }
        if !trade || j == 0 {
            return;
        }
        let mut c = cycle;
        while c.filled_so < d.safety_orders {
            let k = c.filled_so;
            let dev: f64 = (0..=k).map(|i| d.step_pct * d.step_scale.powi(i as i32)).sum();
            let level = c.base_price * (1.0 - dev / 100.0);
            if !(f.l[j] <= level && f.c[j - 1] > level) {
                break;
            }
            let px = level.min(f.o[j]);
            let notional = book.capital() * d.safety_pct / 100.0 * d.volume_scale.powi(k as i32);
            let q = self.qty_for(book, 0, px, Some(notional), None);
            c.filled_so += 1; // counted even if refused, so a refusal does not retry every bar
            if q <= 0.0 {
                book.log("warn", format!("safety order {} skipped: size rounds to zero", k + 1));
                continue;
            }
            let req = OpenReq { leg: 0, side: Side::Long, qty: q, price: px, time: f.t[j], stop: None, target: None, trail: None, tag: format!("dca:so{}", k + 1) };
            if let Err(e) = book.open(req) {
                book.log("warn", format!("safety order {} refused: {e}", k + 1));
            }
        }
        st.dca = Some(c);
    }

    fn pairs(&self, st: &mut State, book: &mut dyn Book, _legs: &Legs, prep: &Prepared, j: usize, trade: bool) {
        let p = self.s.pairs.as_ref().expect("validated");
        let Some(z) = prep.pair_z.as_ref().and_then(|z| z.get(j)).copied().filter(|z| z.is_finite()) else { return };
        let lots = book.lots();
        if !lots.is_empty() {
            let queued = st.pending.iter().any(|x| matches!(x, Pending::CloseAll { .. }));
            if queued {
                return;
            }
            if z.abs() <= p.exit_z {
                st.pending.push(Pending::CloseAll { reason: format!("spread back to z {z:.2}") });
            } else if p.stop_z.is_some_and(|s| z.abs() >= s) {
                st.pending.push(Pending::CloseAll { reason: format!("spread stop at z {z:.2}") });
            }
            return;
        }
        if !trade || !st.pending.is_empty() {
            return;
        }
        let allow_long = matches!(self.s.direction, Direction::Long | Direction::Both);
        let allow_short = matches!(self.s.direction, Direction::Short | Direction::Both);
        if z >= p.entry_z && allow_short {
            st.pending.push(Pending::OpenPair { side_a: Side::Short, reason: format!("spread rich, z {z:.2}: short A, long B") });
        } else if z <= -p.entry_z && allow_long {
            st.pending.push(Pending::OpenPair { side_a: Side::Long, reason: format!("spread cheap, z {z:.2}: long A, short B") });
        }
    }
}

/// Indicator lines for drawing a strategy on a chart: every series an indicator produced, by `id.output`.
pub fn overlay_series(s: &Strategy, f: &Frame) -> Result<Vec<(String, Series)>, String> {
    let mut out = vec![];
    for d in &s.indicators {
        for (name, v) in indicators::compute(&d.kind, &d.params, f)? {
            out.push((format!("{}.{name}", d.id), v));
        }
    }
    Ok(out)
}
