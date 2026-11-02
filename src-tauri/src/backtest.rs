//! Backtest a simple order-block retest rule. Port of `backtest.py`.
//!
//! Rule (long side; the short side is the mirror image):
//!  * A bullish order block is confirmed on bar `confirm_idx`.
//!  * From the next bar, wait up to WAIT bars for price to retrace into the zone.
//!  * Enter at the zone top (or the open if it gaps lower inside the zone). Skip the setup if price
//!    gaps below the whole zone.
//!  * Stop at the zone bottom. Target at RR x risk. Exit at the close after MAX_HOLD bars.
//!  * Stop is checked before target on the same bar (pessimistic). No target on the entry bar itself.
//!  * A round-trip cost (fees, taxes, slippage) is charged in R terms.

use std::collections::BTreeMap;

use crate::zones::{self, Frame, Side};

const WAIT: usize = 30;
const MAX_HOLD: usize = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct Trade {
    pub idx: usize,
    pub long: bool,
    pub net_r: f64,
    pub gross_r: f64,
}

struct Zone {
    top: f64,
    bottom: f64,
    confirm_idx: usize,
    idx: usize,
}

/// Simulate long trades on the given arrays. Returns (entry_idx, net_R, gross_R).
#[allow(clippy::too_many_arguments)]
fn simulate_long(o: &[f64], h: &[f64], l: &[f64], c: &[f64], a: &[f64], zones: &[Zone], rr: f64, cost_pct: f64) -> Vec<(usize, f64, f64)> {
    let n = o.len();
    let mut trades = vec![];
    for z in zones {
        let (top, bot) = (z.top, z.bottom);
        let start = z.confirm_idx + 1;
        let mut entry_idx = None;
        for j in start..n.min(start + WAIT) {
            if o[j] <= bot {
                break; // gapped through the whole zone: skip
            }
            if l[j] <= top {
                entry_idx = Some(j);
                break;
            }
            if c[j] < bot {
                break; // invalidated before the retest
            }
        }
        let Some(j) = entry_idx else { continue };
        let entry = o[j].min(top);
        let risk = entry - bot;
        if risk < 0.2 * a[z.idx] || risk <= 0.0 {
            continue;
        }
        let (stop, target) = (bot, entry + rr * risk);
        let mut exit_price = None;
        if l[j] <= stop {
            exit_price = Some(stop); // entered and stopped on the same bar (pessimistic)
        } else {
            let last = (n - 1).min(j + MAX_HOLD);
            for m in j + 1..=last {
                exit_price = if o[m] <= stop {
                    Some(o[m])
                } else if l[m] <= stop {
                    Some(stop)
                } else if o[m] >= target {
                    Some(o[m])
                } else if h[m] >= target {
                    Some(target)
                } else {
                    None
                };
                if exit_price.is_some() {
                    break;
                }
            }
            if exit_price.is_none() {
                exit_price = Some(c[last]);
            }
        }
        let gross = (exit_price.unwrap() - entry) / risk;
        let net = gross - cost_pct * entry.abs() / risk; // abs(): shorts run on negated prices
        trades.push((j, net, gross));
    }
    trades
}

/// Longs first, then shorts, each in zone order.
pub fn backtest_symbol(f: &Frame, rr: f64, cost_pct: f64) -> Vec<Trade> {
    let a = zones::atr(f, 14);
    let obs = zones::order_blocks(f, 1.5, 3, &a);
    let zone = |ob: &zones::OrderBlock, mirrored: bool| {
        if mirrored {
            Zone { top: -ob.bottom, bottom: -ob.top, confirm_idx: ob.confirm_idx, idx: ob.idx }
        } else {
            Zone { top: ob.top, bottom: ob.bottom, confirm_idx: ob.confirm_idx, idx: ob.idx }
        }
    };
    let bull: Vec<Zone> = obs.iter().filter(|o| o.side == Side::Bullish).map(|o| zone(o, false)).collect();
    let bear: Vec<Zone> = obs.iter().filter(|o| o.side == Side::Bearish).map(|o| zone(o, true)).collect();

    let mut out: Vec<Trade> = simulate_long(&f.o, &f.h, &f.l, &f.c, &a, &bull, rr, cost_pct)
        .into_iter()
        .map(|(idx, net_r, gross_r)| Trade { idx, long: true, net_r, gross_r })
        .collect();
    // Short side: negate prices, so a short becomes a long on the mirrored series.
    let neg = |v: &[f64]| v.iter().map(|x| -x).collect::<Vec<_>>();
    let (no, nh, nl, nc) = (neg(&f.o), neg(&f.l), neg(&f.h), neg(&f.c)); // note: negated low is the new high
    out.extend(simulate_long(&no, &nh, &nl, &nc, &a, &bear, rr, cost_pct).into_iter().map(|(idx, net_r, gross_r)| Trade { idx, long: false, net_r, gross_r }));
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub trades: usize,
    pub win_rate: f64,
    pub avg_r: f64,
    pub total_r: f64,
    pub profit_factor: f64,
}

fn round(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    (x * m).round() / m
}

pub fn stats(rs: &[f64]) -> Option<Stats> {
    if rs.is_empty() {
        return None;
    }
    let wins: f64 = rs.iter().filter(|&&r| r > 0.0).sum();
    let nwins = rs.iter().filter(|&&r| r > 0.0).count();
    let losses: f64 = rs.iter().filter(|&&r| r <= 0.0).sum();
    let pf = if losses < 0.0 { wins / -losses } else { f64::INFINITY };
    let total: f64 = rs.iter().sum();
    Some(Stats {
        trades: rs.len(),
        win_rate: round(100.0 * nwins as f64 / rs.len() as f64, 1),
        avg_r: round(total / rs.len() as f64, 3),
        total_r: round(total, 1),
        profit_factor: if pf.is_finite() { round(pf, 2) } else { pf },
    })
}

pub struct Row {
    pub time: i64,
    pub long: bool,
    pub net_r: f64,
    pub gross_r: f64,
}

/// Run every frame and build the report: all trades after and before costs, older vs newer half, longs vs shorts.
pub fn run(frames: &BTreeMap<String, Frame>, rr: f64, cost_pct: f64) -> Vec<(&'static str, Option<Stats>)> {
    let mut rows: Vec<Row> = vec![];
    for f in frames.values() {
        for t in backtest_symbol(f, rr, cost_pct) {
            rows.push(Row { time: f.t[t.idx], long: t.long, net_r: t.net_r, gross_r: t.gross_r });
        }
    }
    if rows.is_empty() {
        return vec![];
    }
    rows.sort_by_key(|r| r.time);
    let half = rows.len() / 2;
    let net: Vec<f64> = rows.iter().map(|r| r.net_r).collect();
    let gross: Vec<f64> = rows.iter().map(|r| r.gross_r).collect();
    let pick = |long: bool| rows.iter().filter(|r| r.long == long).map(|r| r.net_r).collect::<Vec<_>>();
    vec![
        ("ALL (after costs)", stats(&net)),
        ("ALL (before costs)", stats(&gross)),
        ("First half (older)", stats(&net[..half])),
        ("Second half (newer)", stats(&net[half..])),
        ("Longs", stats(&pick(true))),
        ("Shorts", stats(&pick(false))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_then(tail: &[[f64; 5]]) -> Frame {
        let mut f = Frame::default();
        let base = [100.0, 101.0, 99.0, 100.5, 1000.0];
        for (i, r) in std::iter::repeat_n(&base, 30).chain(tail.iter()).enumerate() {
            f.push(i as i64 * 86400, r[0], r[1], r[2], r[3], r[4]);
        }
        f
    }

    #[test]
    fn hand_example_hits_the_target() {
        // bullish OB zone 97.5..100.2 confirmed at bar 31; bar 32 retraces to 100 then rallies
        let f = flat_then(&[
            [100.0, 100.2, 97.5, 98.0, 1000.0],
            [98.0, 106.5, 97.8, 106.0, 3000.0],
            [106.0, 106.2, 100.0, 105.0, 1000.0], // entry at 100.2 (zone top), no stop (low 100.0 > 97.5)
            [105.0, 112.0, 104.0, 111.0, 1000.0], // target = 100.2 + 2 * 2.7 = 105.6 -> hit
        ]);
        let longs: Vec<_> = backtest_symbol(&f, 2.0, 0.0).into_iter().filter(|t| t.long).collect();
        assert_eq!(longs.len(), 1);
        assert!((longs[0].gross_r - 2.0).abs() < 1e-9);
    }

    #[test]
    fn costs_only_ever_reduce_the_result() {
        let f = crate::data::synthetic("ABC", "1d");
        let trades = backtest_symbol(&f, 2.0, 0.001);
        assert!(!trades.is_empty());
        assert!(trades.iter().all(|t| t.net_r <= t.gross_r));
    }

    #[test]
    fn stats_handle_no_losses_and_no_trades() {
        assert_eq!(stats(&[]), None);
        let s = stats(&[1.0, 2.0]).unwrap();
        assert!(s.profit_factor.is_infinite() && s.win_rate == 100.0 && s.total_r == 3.0);
    }
}
