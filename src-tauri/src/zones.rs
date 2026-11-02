//! Order block / liquidity zone detection and a simple strength score.
//!
//! Port of `zones.py` (and `static/js/zones.js`): same rules, same weights. The parity test
//! (`tests/parity.rs`) replays frames and answers produced by the Python version, so the three
//! implementations can't drift apart silently.
//!
//! These are simplified, rule-based readings of "smart money" chart concepts. A starting point to
//! test, not a proven edge: run the backtest before trusting any of it with real money.

use std::collections::HashMap;

use serde::Serialize;

/// OHLCV bars, oldest first. `t` is unix seconds (UTC).
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub t: Vec<i64>,
    pub o: Vec<f64>,
    pub h: Vec<f64>,
    pub l: Vec<f64>,
    pub c: Vec<f64>,
    pub v: Vec<f64>,
}

impl Frame {
    pub fn len(&self) -> usize {
        self.c.len()
    }

    pub fn is_empty(&self) -> bool {
        self.c.is_empty()
    }

    pub fn push(&mut self, t: i64, o: f64, h: f64, l: f64, c: f64, v: f64) {
        self.t.push(t);
        self.o.push(o);
        self.h.push(h);
        self.l.push(l);
        self.c.push(c);
        self.v.push(v);
    }
}

fn sign(x: f64) -> f64 {
    // not f64::signum: that returns 1.0 for 0.0, numpy's sign returns 0
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn clip(x: f64, lo: f64, hi: f64) -> f64 {
    x.max(lo).min(hi)
}

fn round(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

/// Average true range, Wilder smoothing. NaN until `n` bars exist (pandas `min_periods=n`).
pub fn atr(f: &Frame, n: usize) -> Vec<f64> {
    let alpha = 1.0 / n as f64;
    let mut out = vec![f64::NAN; f.len()];
    let mut prev = 0.0;
    for i in 0..f.len() {
        let mut tr = f.h[i] - f.l[i];
        if i > 0 {
            tr = tr.max((f.h[i] - f.c[i - 1]).abs()).max((f.l[i] - f.c[i - 1]).abs());
        }
        prev = if i == 0 { tr } else { alpha * tr + (1.0 - alpha) * prev };
        if i + 1 >= n {
            out[i] = prev;
        }
    }
    out
}

/// Swing highs and lows. A pivot is confirmed `right` bars after it forms.
pub fn pivots(f: &Frame, left: usize, right: usize) -> (Vec<usize>, Vec<usize>) {
    let (mut highs, mut lows) = (vec![], vec![]);
    let n = f.len();
    for i in left.max(1)..n.saturating_sub(right) {
        let (mut mx, mut mn) = (f64::NEG_INFINITY, f64::INFINITY);
        for k in i - left..=i + right {
            mx = mx.max(f.h[k]);
            mn = mn.min(f.l[k]);
        }
        if f.h[i] == mx && f.h[i - 1] != f.h[i] {
            highs.push(i);
        }
        if f.l[i] == mn && f.l[i - 1] != f.l[i] {
            lows.push(i);
        }
    }
    (highs, lows)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LevelKind {
    High,
    Low,
}

#[derive(Clone, Debug)]
pub struct Level {
    pub kind: LevelKind,
    pub price: f64,
    pub touches: usize,
    pub first_idx: usize,
    pub last_idx: usize,
    pub swept_idx: Option<usize>,
}

/// Swing highs/lows with nearly equal ones merged into one level. A level is "swept" once a later
/// bar trades through it.
pub fn liquidity_levels(f: &Frame, a: &[f64], left: usize, right: usize, tol_atr: f64) -> Vec<Level> {
    let (highs, lows) = pivots(f, left, right);
    let mut out = vec![];
    for (kind, idxs, arr) in [(LevelKind::High, &highs, &f.h), (LevelKind::Low, &lows, &f.l)] {
        let mut pts: Vec<(f64, usize)> = idxs.iter().map(|&i| (arr[i], i)).collect();
        pts.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap().then(x.1.cmp(&y.1)));
        let mut clusters: Vec<Vec<(f64, usize)>> = vec![];
        for (price, i) in pts {
            let tol = tol_atr * if a[i].is_nan() { price * 0.01 } else { a[i] };
            match clusters.last_mut() {
                Some(cl) if price - cl.last().unwrap().0 <= tol => cl.push((price, i)),
                _ => clusters.push(vec![(price, i)]),
            }
        }
        for cl in clusters {
            let level = cl.iter().map(|p| p.0).fold(if kind == LevelKind::High { f64::NEG_INFINITY } else { f64::INFINITY }, |m, p| {
                if kind == LevelKind::High {
                    m.max(p)
                } else {
                    m.min(p)
                }
            });
            let first_idx = cl.iter().map(|p| p.1).min().unwrap();
            let last_idx = cl.iter().map(|p| p.1).max().unwrap();
            let swept_idx = (last_idx + 1..arr.len()).find(|&k| if kind == LevelKind::High { arr[k] > level } else { arr[k] < level });
            out.push(Level { kind, price: level, touches: cl.len(), first_idx, last_idx, swept_idx });
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Bullish,
    Bearish,
}

#[derive(Clone, Debug)]
pub struct OrderBlock {
    pub side: Side,
    pub idx: usize,
    /// The bar where the impulse completes. A trading rule must not act on the zone before it.
    pub confirm_idx: usize,
    pub top: f64,
    pub bottom: f64,
    pub tested_idx: Option<usize>,
    pub mitigated_idx: Option<usize>,
    pub active: bool,
}

/// Bullish OB: the last down candle before an up-move that closes above that candle's high and gains
/// at least `atr_mult` x ATR within `lookahead` bars. Bearish is the mirror image. The zone is the
/// candle's full range. A zone is mitigated once a bar closes through it.
pub fn order_blocks(f: &Frame, atr_mult: f64, lookahead: usize, a: &[f64]) -> Vec<OrderBlock> {
    let n = f.len();
    let mut found: HashMap<(Side, usize), usize> = HashMap::new(); // a later i overwrites: last opposite candle wins
    for i in 14..n.saturating_sub(1) {
        let ai = a[i];
        if ai.is_nan() || ai <= 0.0 {
            continue;
        }
        if f.c[i] < f.o[i] {
            for k in 1..=lookahead {
                let j = i + k;
                if j >= n {
                    break;
                }
                if f.c[j] > f.h[i] && f.c[j] - f.c[i] >= atr_mult * ai {
                    found.insert((Side::Bullish, j), i);
                    break;
                }
            }
        } else if f.c[i] > f.o[i] {
            for k in 1..=lookahead {
                let j = i + k;
                if j >= n {
                    break;
                }
                if f.c[j] < f.l[i] && f.c[i] - f.c[j] >= atr_mult * ai {
                    found.insert((Side::Bearish, j), i);
                    break;
                }
            }
        }
    }
    let mut obs: Vec<OrderBlock> = found
        .into_iter()
        .map(|((side, j), i)| {
            let (top, bottom) = (f.h[i], f.l[i]);
            let (mut tested, mut mitigated) = (None, None);
            for m in j + 1..n {
                if side == Side::Bullish {
                    if tested.is_none() && f.l[m] <= top {
                        tested = Some(m);
                    }
                    if f.c[m] < bottom {
                        mitigated = Some(m);
                        break;
                    }
                } else {
                    if tested.is_none() && f.h[m] >= bottom {
                        tested = Some(m);
                    }
                    if f.c[m] > top {
                        mitigated = Some(m);
                        break;
                    }
                }
            }
            OrderBlock { side, idx: i, confirm_idx: j, top, bottom, tested_idx: tested, mitigated_idx: mitigated, active: mitigated.is_none() }
        })
        .collect();
    obs.sort_by_key(|o| o.idx);
    obs
}

fn ema_last(c: &[f64], span: f64) -> f64 {
    let k = 2.0 / (span + 1.0);
    c.iter().skip(1).fold(c[0], |e, &x| k * x + (1.0 - k) * e)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Label {
    Bullish,
    Bearish,
    Choppy,
}

#[derive(Clone, Debug, Serialize)]
pub struct Strength {
    pub score: f64,
    pub label: Label,
    pub change_pct: f64,
    pub rvol: f64,
    pub atr: f64,
}

/// Score from -100 (strongest bearish) to +100 (strongest bullish): today's move in ATR units 40%,
/// trend vs EMA20/EMA50 25%, relative volume confirming the move 20%, nearness to an active order
/// block in the move's direction 15%. Under 15 in magnitude is "choppy".
pub fn strength(f: &Frame, obs: &[OrderBlock], a: &[f64]) -> Strength {
    let n = f.len();
    let price = f.c[n - 1];
    let atrv = a[n - 1];
    let ret = if n > 1 { f.c[n - 1] / f.c[n - 2] - 1.0 } else { 0.0 };
    let atr_pct = if price != 0.0 { atrv / price } else { 0.0 };
    let mom = if atr_pct > 0.0 { clip(ret / atr_pct / 2.0, -1.0, 1.0) } else { 0.0 };

    let ema20 = ema_last(&f.c, 20.0);
    let ema50 = ema_last(&f.c, 50.0);
    let trend = 0.5 * sign(price - ema20) + 0.5 * sign(ema20 - ema50);

    let base = if n > 21 { f.v[n - 21..n - 1].iter().sum::<f64>() / 20.0 } else { 0.0 };
    let rvol = if base > 0.0 { f.v[n - 1] / base } else { 1.0 };
    let vol_term = sign(mom) * clip((rvol - 1.0) / 2.0, 0.0, 1.0);

    let mut ob_term = 0.0;
    for ob in obs.iter().filter(|o| o.active) {
        if ob.side == Side::Bullish && ob.bottom <= price && price - ob.top <= 1.5 * atrv {
            ob_term = 1.0;
        } else if ob.side == Side::Bearish && ob.top >= price && ob.bottom - price <= 1.5 * atrv {
            ob_term = if ob_term == 0.0 { -1.0 } else { 0.0 };
        }
    }

    let score = 100.0 * (0.40 * mom + 0.25 * trend + 0.20 * vol_term + 0.15 * ob_term);
    let label = if score.abs() < 15.0 {
        Label::Choppy
    } else if score > 0.0 {
        Label::Bullish
    } else {
        Label::Bearish
    };
    Strength { score: round(score, 1), label, change_pct: round(ret * 100.0, 2), rvol: round(rvol, 2), atr: round(atrv, 2) }
}

#[derive(Clone, Debug, Serialize)]
pub struct ObOut {
    pub top: f64,
    pub bottom: f64,
    #[serde(rename = "type")]
    pub side: Side,
}

/// Current picture for one market. Serialises to the same JSON as the Python version's `summarize`.
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub price: f64,
    pub bias: Label,
    #[serde(flatten)]
    pub strength: Strength,
    pub support: Option<f64>,
    pub resistance: Option<f64>,
    pub liquidity: Option<f64>,
    pub ob: Option<ObOut>,
}

pub fn summarize(f: &Frame, obs: &[OrderBlock], liq: &[Level], a: &[f64]) -> Summary {
    let price = f.c[f.len() - 1];
    let st = strength(f, obs, a);
    let bias = if st.label != Label::Choppy {
        st.label
    } else if st.score >= 0.0 {
        Label::Bullish
    } else {
        Label::Bearish
    };

    let open_lows: Vec<&Level> = liq.iter().filter(|x| x.kind == LevelKind::Low && x.swept_idx.is_none() && x.price < price).collect();
    let open_highs: Vec<&Level> = liq.iter().filter(|x| x.kind == LevelKind::High && x.swept_idx.is_none() && x.price > price).collect();
    let support = open_lows.iter().map(|x| x.price).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))));
    let resistance = open_highs.iter().map(|x| x.price).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.min(p))));

    let pool_src = if bias == Label::Bullish { &open_lows } else { &open_highs };
    let pool = pool_src
        .iter()
        .min_by(|x, y| y.touches.cmp(&x.touches).then((x.price - price).abs().partial_cmp(&(y.price - price).abs()).unwrap()))
        .map(|x| x.price);

    let ob_pick = if bias == Label::Bullish {
        obs.iter()
            .filter(|o| o.active && o.side == Side::Bullish && o.bottom <= price)
            .min_by(|x, y| (price - x.top).abs().partial_cmp(&(price - y.top).abs()).unwrap())
    } else {
        obs.iter()
            .filter(|o| o.active && o.side == Side::Bearish && o.top >= price)
            .min_by(|x, y| (x.bottom - price).abs().partial_cmp(&(y.bottom - price).abs()).unwrap())
    };

    Summary {
        price: round(price, 2),
        bias,
        strength: st,
        support: support.map(|x| round(x, 2)),
        resistance: resistance.map(|x| round(x, 2)),
        liquidity: pool.map(|x| round(x, 2)),
        ob: ob_pick.map(|o| ObOut { top: round(o.top, 2), bottom: round(o.bottom, 2), side: o.side }),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ZoneOut {
    #[serde(rename = "type")]
    pub side: Side,
    pub top: f64,
    pub bottom: f64,
    pub start: i64,
    pub tested: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LevelOut {
    pub kind: LevelKind,
    pub price: f64,
    pub touches: usize,
    pub start: i64,
}

/// What the chart draws: the 6 nearest active zones, and the 4 open liquidity levels above and below price.
pub fn chart_overlays(f: &Frame, obs: &[OrderBlock], liq: &[Level]) -> (Vec<ZoneOut>, Vec<LevelOut>) {
    let price = f.c[f.len() - 1];
    let mut active: Vec<&OrderBlock> = obs.iter().filter(|o| o.active).collect();
    let dist = |o: &OrderBlock| (price - o.top).abs().min((price - o.bottom).abs());
    active.sort_by(|x, y| dist(x).partial_cmp(&dist(y)).unwrap()); // stable, like Python's sort
    let zones = active
        .iter()
        .take(6)
        .map(|o| ZoneOut { side: o.side, top: round(o.top, 2), bottom: round(o.bottom, 2), start: f.t[o.idx], tested: o.tested_idx.is_some() })
        .collect();

    let open: Vec<&Level> = liq.iter().filter(|x| x.swept_idx.is_none()).collect();
    let mut above: Vec<&Level> = open.iter().copied().filter(|x| x.price > price).collect();
    above.sort_by(|x, y| x.price.partial_cmp(&y.price).unwrap());
    let mut below: Vec<&Level> = open.iter().copied().filter(|x| x.price <= price).collect();
    below.sort_by(|x, y| y.price.partial_cmp(&x.price).unwrap());
    let levels = above
        .iter()
        .take(4)
        .chain(below.iter().take(4))
        .map(|x| LevelOut { kind: x.kind, price: round(x.price, 2), touches: x.touches, start: f.t[x.first_idx] })
        .collect();
    (zones, levels)
}

/// Everything the scanner and chart need for one frame, computed once.
pub struct Analysis {
    pub obs: Vec<OrderBlock>,
    pub liq: Vec<Level>,
    pub atr: Vec<f64>,
}

pub fn analyze(f: &Frame) -> Analysis {
    let a = atr(f, 14);
    let obs = order_blocks(f, 1.5, 3, &a);
    let liq = liquidity_levels(f, &a, 3, 3, 0.15);
    Analysis { obs, liq, atr: a }
}

/// One-call summary for the scanner.
pub fn summary_of(f: &Frame) -> Summary {
    let an = analyze(f);
    summarize(f, &an.obs, &an.liq, &an.atr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 30 quiet bars (range ~2) followed by custom bars: same fixture as test_zones.py.
    fn flat_then(tail: &[[f64; 5]]) -> Frame {
        let mut f = Frame::default();
        let base = [100.0, 101.0, 99.0, 100.5, 1000.0];
        for (i, r) in std::iter::repeat_n(&base, 30).chain(tail.iter()).enumerate() {
            f.push(i as i64 * 86400, r[0], r[1], r[2], r[3], r[4]);
        }
        f
    }

    fn obs_of(f: &Frame) -> Vec<OrderBlock> {
        order_blocks(f, 1.5, 3, &atr(f, 14))
    }

    #[test]
    fn bullish_order_block_found_with_bounds() {
        let f = flat_then(&[[100.0, 100.2, 97.5, 98.0, 1000.0], [98.0, 106.5, 97.8, 106.0, 3000.0], [106.0, 107.0, 105.0, 106.5, 1000.0]]);
        let bull: Vec<_> = obs_of(&f).into_iter().filter(|o| o.side == Side::Bullish).collect();
        assert_eq!(bull.len(), 1);
        let ob = &bull[0];
        assert_eq!((ob.idx, ob.confirm_idx), (30, 31));
        assert_eq!((ob.top, ob.bottom), (100.2, 97.5));
        assert!(ob.active);
    }

    #[test]
    fn order_block_mitigated_when_a_close_breaks_it() {
        let f = flat_then(&[[100.0, 100.2, 97.5, 98.0, 1000.0], [98.0, 106.5, 97.8, 106.0, 3000.0], [106.0, 107.0, 96.0, 96.5, 2000.0]]);
        let ob = obs_of(&f).into_iter().find(|o| o.side == Side::Bullish).unwrap();
        assert!(!ob.active);
        assert_eq!(ob.mitigated_idx, Some(32));
    }

    #[test]
    fn bearish_order_block_is_the_mirror() {
        let f = flat_then(&[[100.0, 102.5, 99.8, 102.0, 1000.0], [102.0, 102.2, 94.0, 94.5, 3000.0], [94.5, 95.0, 93.5, 94.0, 1000.0]]);
        let bear: Vec<_> = obs_of(&f).into_iter().filter(|o| o.side == Side::Bearish).collect();
        assert_eq!(bear.len(), 1);
        assert_eq!((bear[0].top, bear[0].bottom), (102.5, 99.8));
    }

    #[test]
    fn no_order_block_without_displacement() {
        let f = flat_then(&[[100.0, 100.5, 99.5, 99.8, 1000.0], [99.8, 100.6, 99.6, 100.4, 1000.0]]);
        assert!(obs_of(&f).iter().all(|o| o.idx < 30));
    }

    #[test]
    fn equal_highs_cluster_and_get_swept() {
        let path = [100.0, 104.0, 108.0, 110.0, 108.0, 104.0, 102.0, 105.0, 109.0, 110.0, 108.0, 104.0, 101.0, 103.0, 106.0, 108.0, 111.0];
        let mut f = Frame::default();
        for (i, p) in path.iter().enumerate() {
            f.push(i as i64, p - 0.3, p + 0.2, p - 0.6, *p, 1000.0);
        }
        let liq = liquidity_levels(&f, &atr(&f, 14), 2, 2, 0.15);
        let cluster = liq.iter().filter(|x| x.kind == LevelKind::High).max_by_key(|x| x.touches).unwrap();
        assert_eq!(cluster.touches, 2);
        assert!(cluster.swept_idx.is_some()); // the last bar trades above ~110.2
    }

    #[test]
    fn sign_of_zero_is_zero() {
        assert_eq!(sign(0.0), 0.0);
        assert_eq!(sign(-3.0), -1.0);
    }
}
