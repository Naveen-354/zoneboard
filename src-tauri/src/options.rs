//! Options Strategy Lab: theoretical pricing and payoffs. Nothing here trades, quotes a market or fetches an
//! option chain (no options data provider is configured); every input is the owner's, and every output is
//! labelled THEORETICAL.
//!
//! Model: Black-Scholes for European options with a continuous dividend yield `q`. Greeks: delta, gamma, vega
//! (per 1 volatility point), theta (per calendar day), rho (per 1 rate point). Implied volatility by bisection.
//! Multi-leg strategies: payoff at expiry and model value today across a price grid, breakevens, and maximum
//! profit and loss (reported as unbounded when the payoff keeps growing at the grid's edge). These are analytics
//! on hypothetical inputs, not money balances, so `f64` is used.

use serde_json::{json, Value};

fn norm_cdf(x: f64) -> f64 {
    // Abramowitz & Stegun 7.1.26 via erf, |error| < 1.5e-7
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs() / std::f64::consts::SQRT_2);
    let y = 1.0 - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t + 0.254_829_592) * t * (-(x * x) / 2.0).exp();
    if x >= 0.0 {
        0.5 * (1.0 + y)
    } else {
        0.5 * (1.0 - y)
    }
}

fn norm_pdf(x: f64) -> f64 {
    (-(x * x) / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    pub spot: f64,
    pub strike: f64,
    /// Years to expiry.
    pub t: f64,
    /// Annual volatility (0.2 = 20%).
    pub vol: f64,
    /// Continuously compounded risk-free rate (0.05 = 5%).
    pub rate: f64,
    pub div: f64,
    pub call: bool,
}

#[derive(Debug, PartialEq)]
pub struct Greeks {
    pub price: f64,
    pub delta: f64,
    pub gamma: f64,
    pub vega: f64,
    pub theta: f64,
    pub rho: f64,
}

pub fn validate(i: &Inputs) -> Result<(), String> {
    let ok = |x: f64| x.is_finite();
    if !(ok(i.spot) && i.spot > 0.0 && ok(i.strike) && i.strike > 0.0) {
        return Err("spot and strike must be positive".into());
    }
    if !(ok(i.t) && (0.0..=30.0).contains(&i.t)) {
        return Err("time to expiry must be between 0 and 30 years".into());
    }
    if !(ok(i.vol) && (0.0..=5.0).contains(&i.vol)) {
        return Err("volatility must be between 0 and 5 (500%)".into());
    }
    if !(ok(i.rate) && i.rate.abs() <= 1.0 && ok(i.div) && i.div.abs() <= 1.0) {
        return Err("rate and dividend yield must be between -1 and 1".into());
    }
    Ok(())
}

pub fn bs(i: &Inputs) -> Greeks {
    let intrinsic = if i.call { (i.spot - i.strike).max(0.0) } else { (i.strike - i.spot).max(0.0) };
    if i.t <= 0.0 || i.vol <= 0.0 {
        let itm = intrinsic > 0.0;
        let d = if !itm { 0.0 } else if i.call { 1.0 } else { -1.0 };
        let df = (-i.rate * i.t).exp();
        let fwd = i.spot * ((i.rate - i.div) * i.t).exp();
        let px = if i.call { (fwd - i.strike).max(0.0) * df } else { (i.strike - fwd).max(0.0) * df };
        return Greeks { price: if i.t <= 0.0 { intrinsic } else { px }, delta: d, gamma: 0.0, vega: 0.0, theta: 0.0, rho: 0.0 };
    }
    let st = i.t.sqrt();
    let d1 = ((i.spot / i.strike).ln() + (i.rate - i.div + i.vol * i.vol / 2.0) * i.t) / (i.vol * st);
    let d2 = d1 - i.vol * st;
    let (dq, dr) = ((-i.div * i.t).exp(), (-i.rate * i.t).exp());
    let gamma = dq * norm_pdf(d1) / (i.spot * i.vol * st);
    let vega = i.spot * dq * norm_pdf(d1) * st / 100.0;
    let common = -i.spot * dq * norm_pdf(d1) * i.vol / (2.0 * st);
    if i.call {
        Greeks {
            price: i.spot * dq * norm_cdf(d1) - i.strike * dr * norm_cdf(d2),
            delta: dq * norm_cdf(d1),
            gamma,
            vega,
            theta: (common - i.rate * i.strike * dr * norm_cdf(d2) + i.div * i.spot * dq * norm_cdf(d1)) / 365.0,
            rho: i.strike * i.t * dr * norm_cdf(d2) / 100.0,
        }
    } else {
        Greeks {
            price: i.strike * dr * norm_cdf(-d2) - i.spot * dq * norm_cdf(-d1),
            delta: -dq * norm_cdf(-d1),
            gamma,
            vega,
            theta: (common + i.rate * i.strike * dr * norm_cdf(-d2) - i.div * i.spot * dq * norm_cdf(-d1)) / 365.0,
            rho: -i.strike * i.t * dr * norm_cdf(-d2) / 100.0,
        }
    }
}

/// Implied volatility from an option price, by bisection on [0.0001, 5]. `None` when the price is outside the
/// no-arbitrage range.
pub fn implied_vol(i: &Inputs, price: f64) -> Option<f64> {
    let (mut lo, mut hi) = (1e-4, 5.0);
    let f = |v: f64| bs(&Inputs { vol: v, ..*i }).price - price;
    if f(lo) > 0.0 || f(hi) < 0.0 {
        return None;
    }
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if f(mid) > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
        if hi - lo < 1e-9 {
            break;
        }
    }
    Some((lo + hi) / 2.0)
}

#[derive(Clone, Debug)]
pub struct Leg {
    /// "call", "put" or "stock"
    pub kind: String,
    pub strike: f64,
    /// Positive = long, negative = short (contracts or shares, times the multiplier).
    pub qty: f64,
    /// Premium paid (long) or received (short) per unit; for stock, the entry price.
    pub premium: f64,
}

pub const TEMPLATES: [&str; 9] = ["long_call", "long_put", "covered_call", "protective_put", "bull_call_spread", "bear_put_spread", "long_straddle", "long_strangle", "iron_condor"];

/// Legs of a template around `spot`, with strikes `width` apart, priced by the model so the lab has a start.
pub fn template(name: &str, spot: f64, width: f64, base: &Inputs) -> Result<Vec<Leg>, String> {
    let price = |kind: &str, k: f64| match kind {
        "stock" => spot,
        _ => bs(&Inputs { strike: k, call: kind == "call", spot, ..*base }).price,
    };
    let leg = |kind: &str, k: f64, qty: f64| Leg { kind: kind.into(), strike: k, qty, premium: (price(kind, k) * 1e4).round() / 1e4 };
    let (a, w) = ((spot / width).round() * width, width);
    Ok(match name {
        "long_call" => vec![leg("call", a, 1.0)],
        "long_put" => vec![leg("put", a, 1.0)],
        "covered_call" => vec![leg("stock", 0.0, 1.0), leg("call", a + w, -1.0)],
        "protective_put" => vec![leg("stock", 0.0, 1.0), leg("put", a - w, 1.0)],
        "bull_call_spread" => vec![leg("call", a, 1.0), leg("call", a + w, -1.0)],
        "bear_put_spread" => vec![leg("put", a, 1.0), leg("put", a - w, -1.0)],
        "long_straddle" => vec![leg("call", a, 1.0), leg("put", a, 1.0)],
        "long_strangle" => vec![leg("call", a + w, 1.0), leg("put", a - w, 1.0)],
        "iron_condor" => vec![leg("put", a - 2.0 * w, 1.0), leg("put", a - w, -1.0), leg("call", a + w, -1.0), leg("call", a + 2.0 * w, 1.0)],
        _ => return Err(format!("unknown template; one of {}", TEMPLATES.join(", "))),
    })
}

fn expiry_value(l: &Leg, s: f64) -> f64 {
    let v = match l.kind.as_str() {
        "call" => (s - l.strike).max(0.0),
        "put" => (l.strike - s).max(0.0),
        _ => s,
    };
    (v - l.premium) * l.qty
}

/// Payoff analysis of a multi-leg position. `base` carries time, volatility and rates for the model curve.
pub fn analyse(legs: &[Leg], spot: f64, base: &Inputs, multiplier: f64) -> Result<Value, String> {
    if legs.is_empty() || legs.len() > 8 {
        return Err("1 to 8 legs".into());
    }
    for l in legs {
        if !["call", "put", "stock"].contains(&l.kind.as_str()) || !(l.qty.is_finite() && l.qty != 0.0 && l.premium.is_finite() && l.premium >= 0.0) || (l.kind != "stock" && !(l.strike > 0.0 && l.strike.is_finite())) {
            return Err("each leg needs kind call|put|stock, a non-zero qty, a premium >= 0 and (for options) a positive strike".into());
        }
    }
    validate(&Inputs { spot, strike: spot, ..*base })?;
    let hi_k = legs.iter().map(|l| l.strike).fold(spot, f64::max);
    let lo_k = legs.iter().filter(|l| l.kind != "stock").map(|l| l.strike).fold(spot, f64::min);
    let (lo, hi) = ((lo_k * 0.5).max(0.0), hi_k * 1.5);
    let n = 200;
    let grid: Vec<f64> = (0..=n).map(|i| lo + (hi - lo) * i as f64 / n as f64).collect();
    let at_expiry = |s: f64| legs.iter().map(|l| expiry_value(l, s)).sum::<f64>() * multiplier;
    let model_now = |s: f64| {
        legs.iter()
            .map(|l| {
                let v = if l.kind == "stock" { s } else { bs(&Inputs { spot: s, strike: l.strike, call: l.kind == "call", ..*base }).price };
                (v - l.premium) * l.qty
            })
            .sum::<f64>()
            * multiplier
    };
    let curve: Vec<Value> = grid.iter().map(|&s| json!({ "price": (s * 1e4).round() / 1e4, "expiry": (at_expiry(s) * 1e4).round() / 1e4, "now": (model_now(s) * 1e4).round() / 1e4 })).collect();
    let mut breakevens = vec![];
    for w in grid.windows(2) {
        let (a, b) = (at_expiry(w[0]), at_expiry(w[1]));
        if a == 0.0 {
            breakevens.push(w[0]);
        } else if a.signum() != b.signum() && b != 0.0 {
            breakevens.push(w[0] + (w[1] - w[0]) * a.abs() / (a.abs() + b.abs()));
        }
    }
    // slope beyond the highest strike (each long call or share adds +1, short subtracts) tells if it is unbounded
    let up_slope: f64 = legs.iter().filter(|l| l.kind != "put").map(|l| l.qty).sum();
    let vals: Vec<f64> = grid.iter().map(|&s| at_expiry(s)).collect();
    let (mx, mn) = (vals.iter().cloned().fold(f64::MIN, f64::max), vals.iter().cloned().fold(f64::MAX, f64::min));
    let at_zero = at_expiry(0.0);
    let g = legs.iter().filter(|l| l.kind != "stock").fold((0.0, 0.0, 0.0, 0.0), |acc, l| {
        let x = bs(&Inputs { spot, strike: l.strike, call: l.kind == "call", ..*base });
        (acc.0 + x.delta * l.qty, acc.1 + x.gamma * l.qty, acc.2 + x.vega * l.qty, acc.3 + x.theta * l.qty)
    });
    let stock_delta: f64 = legs.iter().filter(|l| l.kind == "stock").map(|l| l.qty).sum();
    let r4 = |x: f64| (x * 1e4).round() / 1e4;
    Ok(json!({
        "label": "THEORETICAL (Black-Scholes, your inputs; not a quote and not advice)",
        "curve": curve,
        "breakevens": breakevens.iter().map(|b| r4(*b)).collect::<Vec<_>>(),
        "max_profit": if up_slope > 0.0 { json!("unbounded") } else { json!(r4(mx.max(at_zero))) },
        "max_loss": if up_slope < 0.0 { json!("unbounded") } else { json!(r4(mn.min(at_zero))) },
        "net_premium": r4(-legs.iter().filter(|l| l.kind != "stock").map(|l| l.premium * l.qty).sum::<f64>() * multiplier),
        "greeks": { "delta": r4((g.0 + stock_delta) * multiplier), "gamma": r4(g.1 * multiplier), "vega": r4(g.2 * multiplier), "theta_per_day": r4(g.3 * multiplier) },
        "assumptions": "European exercise, constant volatility and rates, no early assignment, no transaction costs. American options and real chains differ.",
    }))
}

pub fn inputs_from(b: &Value) -> Result<Inputs, String> {
    let f = |k: &str, d: Option<f64>| b.get(k).and_then(Value::as_f64).or(d).ok_or(format!("{k} is required"));
    let i = Inputs {
        spot: f("spot", None)?,
        strike: f("strike", b.get("spot").and_then(Value::as_f64))?,
        t: f("days", Some(30.0))? / 365.0,
        vol: f("vol_pct", Some(20.0))? / 100.0,
        rate: f("rate_pct", Some(5.0))? / 100.0,
        div: f("div_pct", Some(0.0))? / 100.0,
        call: b.get("type").and_then(Value::as_str).unwrap_or("call") != "put",
    };
    validate(&i)?;
    Ok(i)
}

pub fn legs_from(b: &Value) -> Result<Vec<Leg>, String> {
    b.get("legs").and_then(Value::as_array).ok_or("legs are required")?.iter().map(|l| {
        Ok(Leg {
            kind: l["kind"].as_str().unwrap_or("").to_string(),
            strike: l["strike"].as_f64().unwrap_or(0.0),
            qty: l["qty"].as_f64().ok_or("each leg needs qty")?,
            premium: l["premium"].as_f64().ok_or("each leg needs premium")?,
        })
    }).collect()
}

pub fn leg_json(l: &Leg) -> Value {
    json!({ "kind": l.kind, "strike": l.strike, "qty": l.qty, "premium": l.premium })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Inputs {
        Inputs { spot: 100.0, strike: 100.0, t: 1.0, vol: 0.2, rate: 0.05, div: 0.0, call: true }
    }

    #[test]
    fn black_scholes_matches_known_values() {
        // Hull's textbook values for S=K=100, r=5%, sigma=20%, T=1
        let c = bs(&base());
        let p = bs(&Inputs { call: false, ..base() });
        assert!((c.price - 10.4506).abs() < 1e-3, "{c:?}");
        assert!((p.price - 5.5735).abs() < 1e-3, "{p:?}");
        assert!((c.delta - 0.6368).abs() < 1e-3);
        assert!((p.delta + 0.3632).abs() < 1e-3);
        assert!((c.gamma - 0.01876).abs() < 1e-4);
        assert!((c.vega - 0.3752).abs() < 1e-3);
        assert!((c.theta * 365.0 + 6.414).abs() < 1e-2);
        assert!((c.rho - 0.5323).abs() < 1e-3);
        // put-call parity
        assert!((c.price - p.price - (100.0 - 100.0 * (-0.05f64).exp())).abs() < 1e-6);
        // implied vol recovers the input
        assert!((implied_vol(&base(), c.price).unwrap() - 0.2).abs() < 1e-6);
        assert!(implied_vol(&base(), 200.0).is_none());
        // at expiry: intrinsic
        assert_eq!(bs(&Inputs { t: 0.0, spot: 110.0, ..base() }).price, 10.0);
        assert!(validate(&Inputs { spot: -1.0, ..base() }).is_err());
    }

    #[test]
    fn payoffs_breakevens_and_bounds() {
        let b = Inputs { t: 30.0 / 365.0, ..base() };
        let call = vec![Leg { kind: "call".into(), strike: 100.0, qty: 1.0, premium: 5.0 }];
        let a = analyse(&call, 100.0, &b, 1.0).unwrap();
        assert_eq!(a["max_profit"], "unbounded");
        assert_eq!(a["max_loss"], -5.0);
        assert!((a["breakevens"][0].as_f64().unwrap() - 105.0).abs() < 0.01);
        let spread = template("bull_call_spread", 100.0, 5.0, &b).unwrap();
        let s = analyse(&spread, 100.0, &b, 1.0).unwrap();
        let debit = spread[0].premium - spread[1].premium;
        assert!((s["max_loss"].as_f64().unwrap() + debit).abs() < 1e-3, "{}", s["max_loss"]);
        assert!((s["max_profit"].as_f64().unwrap() - (5.0 - debit)).abs() < 1e-3);
        let naked = vec![Leg { kind: "call".into(), strike: 100.0, qty: -1.0, premium: 5.0 }];
        assert_eq!(analyse(&naked, 100.0, &b, 1.0).unwrap()["max_loss"], "unbounded");
        let ic = analyse(&template("iron_condor", 100.0, 5.0, &b).unwrap(), 100.0, &b, 1.0).unwrap();
        assert_eq!(ic["breakevens"].as_array().unwrap().len(), 2);
        assert!(ic["max_profit"].as_f64().unwrap() > 0.0 && ic["max_loss"].as_f64().unwrap() < 0.0);
        for t in TEMPLATES {
            assert!(analyse(&template(t, 100.0, 5.0, &b).unwrap(), 100.0, &b, 1.0).is_ok(), "{t}");
        }
        assert!(template("moon", 100.0, 5.0, &b).is_err());
    }
}
