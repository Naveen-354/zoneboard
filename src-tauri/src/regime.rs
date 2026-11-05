//! Market regime classification, rule based and versioned (`rules-v1`), explainable feature by feature.
//!
//! At bar `i` it uses only bars `0..=i` (tested: classifying a prefix gives the same answer):
//!  * trend: ADX(14), the 20-bar return, close versus EMA(50);
//!  * volatility: 20-bar realised volatility of log returns, ranked against its own previous 100 values.
//! Trend regime: `trending_up` (ADX >= 25, positive return, close above EMA), `trending_down` (mirror),
//! `range_bound` (ADX < 20), otherwise `transition`. Volatility regime: `high_volatility` (rank >= 0.8),
//! `low_volatility` (rank <= 0.2), else `normal`. Warm-up bars are `unknown`. Confidence is how far the
//! deciding features sit from their thresholds (0 to 1). A regime is an estimate, never an instruction:
//! nothing here can place an order.

use serde_json::{json, Value};

use crate::indicators;
use crate::zones::Frame;

pub const RULE_VERSION: &str = "rules-v1";

#[derive(Clone, Debug, PartialEq)]
pub struct Obs {
    pub regime: &'static str,
    pub volatility: &'static str,
    pub confidence: f64,
    pub features: Value,
}

fn rank(v: &[f64], i: usize, n: usize) -> f64 {
    let lo = i.saturating_sub(n);
    let w: Vec<f64> = v[lo..i].iter().copied().filter(|x| x.is_finite()).collect();
    if w.len() < 20 || !v[i].is_finite() {
        return f64::NAN;
    }
    w.iter().filter(|x| **x <= v[i]).count() as f64 / w.len() as f64
}

/// Classify every bar.
pub fn classify(f: &Frame) -> Vec<Obs> {
    let n = f.len();
    let (adx, _, _) = indicators::adx(f, 14);
    let ema = indicators::ema(&f.c, 50);
    let lr: Vec<f64> = (0..n).map(|i| if i == 0 { f64::NAN } else { (f.c[i] / f.c[i - 1]).ln() }).collect();
    let vol: Vec<f64> = (0..n).map(|i| if i < 20 { f64::NAN } else { let w = &lr[i - 19..=i]; let m = w.iter().sum::<f64>() / 20.0; (w.iter().map(|x| (x - m).powi(2)).sum::<f64>() / 20.0).sqrt() }).collect();
    (0..n)
        .map(|i| {
            let ret = if i >= 20 { f.c[i] / f.c[i - 20] - 1.0 } else { f64::NAN };
            let vr = rank(&vol, i, 100);
            let ready = adx[i].is_finite() && ema[i].is_finite() && ret.is_finite();
            let features = json!({ "adx14": adx[i], "return_20_pct": ret * 100.0, "close_vs_ema50_pct": (f.c[i] / ema[i] - 1.0) * 100.0, "realised_vol_20": vol[i], "vol_rank_100": vr });
            if !ready {
                return Obs { regime: "unknown", volatility: "unknown", confidence: 0.0, features };
            }
            let a = adx[i];
            let (regime, conf) = if a >= 25.0 && ret > 0.0 && f.c[i] > ema[i] {
                ("trending_up", ((a - 25.0) / 25.0).min(1.0) * 0.6 + (ret.abs() * 10.0).min(1.0) * 0.4)
            } else if a >= 25.0 && ret < 0.0 && f.c[i] < ema[i] {
                ("trending_down", ((a - 25.0) / 25.0).min(1.0) * 0.6 + (ret.abs() * 10.0).min(1.0) * 0.4)
            } else if a < 20.0 {
                ("range_bound", ((20.0 - a) / 20.0).min(1.0))
            } else {
                ("transition", 0.0)
            };
            let volatility = if !vr.is_finite() { "unknown" } else if vr >= 0.8 { "high_volatility" } else if vr <= 0.2 { "low_volatility" } else { "normal" };
            Obs { regime, volatility, confidence: (conf * 1000.0).round() / 1000.0, features }
        })
        .collect()
}

pub fn to_json(f: &Frame, obs: &[Obs], last: usize) -> Value {
    let from = obs.len().saturating_sub(last);
    let series: Vec<Value> = (from..obs.len()).map(|i| json!({ "t": f.t[i], "close": f.c[i], "regime": obs[i].regime, "volatility": obs[i].volatility, "confidence": obs[i].confidence })).collect();
    let cur = obs.last();
    let mut counts = std::collections::BTreeMap::new();
    for o in &obs[from..] {
        *counts.entry(o.regime).or_insert(0usize) += 1;
    }
    json!({
        "rule_version": RULE_VERSION, "series": series, "distribution": counts,
        "current": cur.map(|o| json!({ "regime": o.regime, "volatility": o.volatility, "confidence": o.confidence, "features": o.features, "bar_time": f.t.last() })),
        "method": "ADX(14) >= 25 with the 20-bar return and close vs EMA(50) for trend; ADX < 20 for range; 20-bar realised volatility ranked against its previous 100 values for volatility. Estimates, not predictions.",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(c: &[f64]) -> Frame {
        let mut f = Frame::default();
        for (i, x) in c.iter().enumerate() {
            let o = if i == 0 { *x } else { c[i - 1] };
            f.push(i as i64 * 3600, o, o.max(*x) * 1.002, o.min(*x) * 0.998, *x, 1000.0);
        }
        f
    }

    #[test]
    fn trends_ranges_and_warm_up() {
        let up: Vec<f64> = (0..200).map(|i| 100.0 * 1.01f64.powi(i)).collect();
        let o = classify(&frame(&up));
        assert_eq!(o[5].regime, "unknown");
        assert_eq!(o[199].regime, "trending_up");
        assert!(o[199].confidence > 0.5);
        let down: Vec<f64> = (0..200).map(|i| 100.0 * 0.99f64.powi(i)).collect();
        assert_eq!(classify(&frame(&down))[199].regime, "trending_down");
        // a market that keeps swinging back and forth inside a band
        let range: Vec<f64> = (0..200).map(|i| 100.0 + 2.0 * (i as f64 * 1.0).sin()).collect();
        let o = classify(&frame(&range));
        assert_eq!(o[199].regime, "range_bound", "{:?}", o[199]);
    }

    #[test]
    fn no_future_data_is_used() {
        let mut s = 7u64;
        let c: Vec<f64> = (0..300)
            .scan(100.0, |x, _| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *x *= 1.0 + ((s % 1000) as f64 / 1000.0 - 0.5) * 0.03;
                Some(*x)
            })
            .collect();
        let full = classify(&frame(&c));
        let part = classify(&frame(&c[..180]));
        for i in 0..180 {
            assert_eq!(full[i], part[i], "bar {i} changed when later bars were added");
        }
    }
}
