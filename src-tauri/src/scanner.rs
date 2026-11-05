//! Market scanner: configurable scans over the crypto or NSE symbol list.
//!
//! Every row says what it measured (the formula with its parameters), on which bar, from which source, and
//! whether that symbol's data is fresh. Scans are computed on **closed** bars only. Results are information:
//! they can open a backtest, never place an order.
//!
//! Not available, and said so: crypto funding and open interest (needs the Binance futures API; ZoneBoard is
//! spot only), options / derivatives analytics (no entitled data feed), news and economic calendars (no
//! reliable source configured).

use serde_json::{json, Value};

use crate::indicators;
use crate::zones::Frame;

pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub formula: &'static str,
    pub params: &'static [(&'static str, f64)],
    pub markets: &'static [&'static str],
}

pub const TEMPLATES: &[Template] = &[
    Template { id: "movers", name: "Price movers", formula: "100 * (close / close[lookback] - 1); |change| >= threshold_pct", params: &[("lookback", 24.0), ("threshold_pct", 3.0)], markets: &["crypto", "india"] },
    Template { id: "rel_volume", name: "Relative volume spike", formula: "volume / mean(volume of the previous period bars) >= ratio", params: &[("period", 20.0), ("ratio", 2.0)], markets: &["crypto", "india"] },
    Template { id: "breakout", name: "Breakout / breakdown", formula: "close > highest high of the previous period bars (breakout) or close < lowest low (breakdown)", params: &[("period", 20.0)], markets: &["crypto", "india"] },
    Template { id: "volatility", name: "Volatility expansion", formula: "ATR(14)/close now divided by ATR(14)/close lookback bars ago >= ratio", params: &[("lookback", 20.0), ("ratio", 1.5)], markets: &["crypto", "india"] },
    Template { id: "trend_momentum", name: "Trend + momentum", formula: "close > EMA(fast) > EMA(slow) and rsi_min <= RSI(14) <= rsi_max", params: &[("fast", 50.0), ("slow", 200.0), ("rsi_min", 50.0), ("rsi_max", 70.0)], markets: &["crypto", "india"] },
    Template { id: "gap", name: "Opening gap (NSE daily)", formula: "100 * (open / previous close - 1); |gap| >= threshold_pct, on daily bars", params: &[("threshold_pct", 2.0)], markets: &["india"] },
    Template { id: "relative_strength", name: "Relative strength", formula: "return over lookback bars minus the median return of the scanned universe", params: &[("lookback", 50.0)], markets: &["crypto", "india"] },
    Template { id: "correlation", name: "Correlation to the first symbol", formula: "Pearson correlation of bar returns with the universe's first symbol over lookback bars", params: &[("lookback", 100.0)], markets: &["crypto", "india"] },
];

pub const UNSUPPORTED: [(&str, &str); 4] = [
    ("Crypto funding rate / open interest", "needs the Binance futures API; ZoneBoard trades and reads spot only (decision D1)"),
    ("Options / derivatives analytics", "no entitled options data feed is connected"),
    ("News and economic calendar", "no reliable source is configured; ZoneBoard does not scrape news"),
    ("Spread / order-book liquidity", "the scanner uses candles only; order-book depth is not collected"),
];

pub fn catalog() -> Value {
    json!({
        "templates": TEMPLATES.iter().map(|t| json!({ "id": t.id, "name": t.name, "formula": t.formula, "markets": t.markets, "params": t.params.iter().map(|(k, v)| json!({ "name": k, "default": v })).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "unsupported": UNSUPPORTED.iter().map(|(a, b)| json!({ "feature": a, "why": b })).collect::<Vec<_>>(),
    })
}

fn param(t: &Template, given: &Value, k: &str) -> f64 {
    given.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or_else(|| t.params.iter().find(|p| p.0 == k).map_or(0.0, |p| p.1))
}

fn returns(c: &[f64]) -> Vec<f64> {
    c.windows(2).map(|w| w[1] / w[0] - 1.0).collect()
}

fn corr(a: &[f64], b: &[f64]) -> Option<f64> {
    let n = a.len().min(b.len());
    if n < 10 {
        return None;
    }
    let (a, b) = (&a[a.len() - n..], &b[b.len() - n..]);
    let (ma, mb) = (a.iter().sum::<f64>() / n as f64, b.iter().sum::<f64>() / n as f64);
    let cov: f64 = a.iter().zip(b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let (va, vb): (f64, f64) = (a.iter().map(|x| (x - ma).powi(2)).sum(), b.iter().map(|y| (y - mb).powi(2)).sum());
    (va > 0.0 && vb > 0.0).then(|| cov / (va * vb).sqrt())
}

/// Run a template over (symbol, closed bars). Returns the matching rows, strongest first.
pub fn scan(template: &str, params: &Value, frames: &[(String, Frame)]) -> Result<Vec<Value>, String> {
    let t = TEMPLATES.iter().find(|t| t.id == template).ok_or_else(|| format!("unknown scan '{template}'"))?;
    let p = |k: &str| param(t, params, k);
    let mut rows: Vec<(f64, Value)> = vec![];
    let median_ret = if t.id == "relative_strength" {
        let lb = p("lookback").max(1.0) as usize;
        let mut r: Vec<f64> = frames.iter().filter(|(_, f)| f.len() > lb).map(|(_, f)| f.c[f.len() - 1] / f.c[f.len() - 1 - lb] - 1.0).collect();
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (!r.is_empty()).then(|| r[r.len() / 2])
    } else {
        None
    };
    let bench = frames.first().map(|(_, f)| returns(&f.c));
    for (sym, f) in frames {
        let n = f.len();
        if n < 3 {
            continue;
        }
        let last = n - 1;
        let hit: Option<(f64, String)> = match t.id {
            "movers" => {
                let lb = p("lookback").max(1.0) as usize;
                (n > lb).then(|| 100.0 * (f.c[last] / f.c[last - lb] - 1.0)).filter(|x| x.abs() >= p("threshold_pct")).map(|x| (x.abs(), format!("{x:+.2}% over {lb} bars")))
            }
            "rel_volume" => {
                let r = indicators::volume_ratio(f, p("period").max(1.0) as usize)[last];
                (r.is_finite() && r >= p("ratio")).then(|| (r, format!("volume {r:.2}x its average")))
            }
            "breakout" => {
                let (u, l, _) = indicators::donchian(f, p("period").max(1.0) as usize);
                if u[last].is_finite() && f.c[last] > u[last] {
                    Some((100.0 * (f.c[last] / u[last] - 1.0), format!("breakout: close above the {}-bar high by {:.2}%", p("period"), 100.0 * (f.c[last] / u[last] - 1.0))))
                } else if l[last].is_finite() && f.c[last] < l[last] {
                    Some((100.0 * (1.0 - f.c[last] / l[last]), format!("breakdown: close below the {}-bar low by {:.2}%", p("period"), 100.0 * (1.0 - f.c[last] / l[last]))))
                } else {
                    None
                }
            }
            "volatility" => {
                let lb = p("lookback").max(1.0) as usize;
                let a = indicators::atr(f, 14);
                (n > lb && a[last - lb].is_finite() && a[last].is_finite()).then(|| (a[last] / f.c[last]) / (a[last - lb] / f.c[last - lb])).filter(|r| r.is_finite() && *r >= p("ratio")).map(|r| (r, format!("ATR% is {r:.2}x what it was {lb} bars ago")))
            }
            "trend_momentum" => {
                let (ef, es, r) = (indicators::ema(&f.c, p("fast") as usize), indicators::ema(&f.c, p("slow") as usize), indicators::rsi(&f.c, 14));
                (ef[last].is_finite() && es[last].is_finite() && r[last].is_finite() && f.c[last] > ef[last] && ef[last] > es[last] && r[last] >= p("rsi_min") && r[last] <= p("rsi_max")).then(|| (r[last], format!("uptrend, RSI {:.1}", r[last])))
            }
            "gap" => {
                let g = 100.0 * (f.o[last] / f.c[last - 1] - 1.0);
                (g.abs() >= p("threshold_pct")).then(|| (g.abs(), format!("gap {g:+.2}%")))
            }
            "relative_strength" => {
                let lb = p("lookback").max(1.0) as usize;
                match (n > lb, median_ret) {
                    (true, Some(m)) => {
                        let rs = 100.0 * ((f.c[last] / f.c[last - lb] - 1.0) - m);
                        Some((rs, format!("{rs:+.2} points vs the universe median over {lb} bars")))
                    }
                    _ => None,
                }
            }
            "correlation" => {
                let lb = p("lookback").max(10.0) as usize;
                let r = returns(&f.c);
                bench.as_ref().and_then(|b| {
                    let take = |v: &[f64]| v[v.len().saturating_sub(lb)..].to_vec();
                    corr(&take(&r), &take(b)).map(|c| (c, format!("correlation {c:.2} over {lb} bars")))
                })
            }
            _ => None,
        };
        if let Some((score, text)) = hit {
            rows.push((score, json!({ "symbol": sym, "score": (score * 1000.0).round() / 1000.0, "detail": text, "close": f.c[last], "bar_time": f.t[last] })));
        }
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(rows.into_iter().map(|r| r.1).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(c: &[f64], v: &[f64]) -> Frame {
        let mut f = Frame::default();
        for i in 0..c.len() {
            let o = if i == 0 { c[0] } else { c[i - 1] };
            f.push(i as i64 * 3600, o, o.max(c[i]) + 0.1, o.min(c[i]) - 0.1, c[i], v[i]);
        }
        f
    }

    #[test]
    fn formulas_and_thresholds() {
        let flat: Vec<f64> = vec![100.0; 30];
        let mut up = flat.clone();
        up[29] = 110.0;
        let vol = vec![10.0; 30];
        let mut spike = vol.clone();
        spike[29] = 50.0;
        let fr = vec![("FLATUSDT".to_string(), frame(&flat, &vol)), ("UPUSDT".to_string(), frame(&up, &spike))];
        let m = scan("movers", &json!({ "lookback": 1, "threshold_pct": 5 }), &fr).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0]["symbol"], "UPUSDT");
        assert!(m[0]["detail"].as_str().unwrap().contains("+10.00%"));
        let v = scan("rel_volume", &json!({ "period": 20, "ratio": 3 }), &fr).unwrap();
        assert_eq!((v.len(), v[0]["symbol"].as_str()), (1, Some("UPUSDT")));
        let b = scan("breakout", &json!({ "period": 20 }), &fr).unwrap();
        assert_eq!(b[0]["symbol"], "UPUSDT");
        assert!(scan("nope", &json!({}), &fr).is_err());
        // a symbol with too little data is skipped, not an error
        let short = vec![("XUSDT".to_string(), frame(&[1.0, 2.0], &[1.0, 1.0]))];
        assert!(scan("volatility", &json!({}), &short).unwrap().is_empty());
    }

    #[test]
    fn correlation_and_relative_strength() {
        let a: Vec<f64> = (0..120).map(|i| 100.0 + (i as f64 * 0.3).sin() * 5.0 + i as f64 * 0.1).collect();
        let vol = vec![1.0; 120];
        let fr = vec![("A".to_string(), frame(&a, &vol)), ("B".to_string(), frame(&a, &vol))];
        let c = scan("correlation", &json!({ "lookback": 100 }), &fr).unwrap();
        assert!((c[1]["score"].as_f64().unwrap() - 1.0).abs() < 1e-6, "identical series correlate perfectly");
        let rs = scan("relative_strength", &json!({ "lookback": 50 }), &fr).unwrap();
        assert!(rs.iter().all(|r| r["score"].as_f64().unwrap().abs() < 1e-9), "same returns, zero relative strength");
    }
}
