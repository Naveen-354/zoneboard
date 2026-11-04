//! Technical indicators for the bot engine and the backtester.
//!
//! Every function is **causal**: the value at bar `i` uses bars `0..=i` only, so a strategy that reads it on
//! bar `i`'s close cannot see the future. `prefix_equals_full` in the tests checks this for every indicator.
//! Warm-up bars are `NaN`; conditions treat `NaN` as "not true", so nothing fires before an indicator is ready.
//!
//! Conventions follow the common charting definitions (Wilder smoothing for RSI, ATR and ADX; population
//! standard deviation for Bollinger Bands and Z-Score). Two deliberate choices, both to keep signals honest:
//!  * Donchian bands are the high / low of the **previous** `n` bars, so "close above the upper band" is a
//!    breakout of the prior range. Including the current bar would make the band always contain the close.
//!  * Ichimoku spans are the values plotted *at* bar `i` (computed `displacement` bars earlier). The lagging
//!    span (Chikou) plots today's close in the past, which a live strategy cannot use, so it is not offered.
//!
//! Prices are `f64` here: these are measurements, not money. Money (fills, fees, P&L) is `Decimal` elsewhere.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::zones::Frame;

pub type Series = Vec<f64>;
pub type Outputs = BTreeMap<&'static str, Series>;

const NAN: f64 = f64::NAN;

fn first_valid(x: &[f64]) -> Option<usize> {
    x.iter().position(|v| v.is_finite())
}

// ------------------------------------------------------------------ building blocks

/// Simple moving average over `n` bars. `NaN` until `n` finite values are in the window.
pub fn sma(x: &[f64], n: usize) -> Series {
    let mut out = vec![NAN; x.len()];
    if n == 0 {
        return out;
    }
    for i in (n - 1)..x.len() {
        let w = &x[i + 1 - n..=i];
        if w.iter().all(|v| v.is_finite()) {
            out[i] = w.iter().sum::<f64>() / n as f64;
        }
    }
    out
}

/// Exponential average with smoothing `alpha`, seeded by the SMA of the first `n` finite values.
fn smoothed(x: &[f64], n: usize, alpha: f64) -> Series {
    let mut out = vec![NAN; x.len()];
    let Some(start) = first_valid(x) else { return out };
    if n == 0 || start + n > x.len() {
        return out;
    }
    let seed = &x[start..start + n];
    if !seed.iter().all(|v| v.is_finite()) {
        return out;
    }
    let mut prev = seed.iter().sum::<f64>() / n as f64;
    out[start + n - 1] = prev;
    for i in start + n..x.len() {
        if !x[i].is_finite() {
            break; // a gap after the seed: stop rather than smooth over garbage
        }
        prev = alpha * x[i] + (1.0 - alpha) * prev;
        out[i] = prev;
    }
    out
}

/// Exponential moving average, `alpha = 2 / (n + 1)`.
pub fn ema(x: &[f64], n: usize) -> Series {
    smoothed(x, n, 2.0 / (n as f64 + 1.0))
}

/// Wilder's moving average (RMA), `alpha = 1 / n`. Used by RSI, ATR and ADX.
pub fn rma(x: &[f64], n: usize) -> Series {
    smoothed(x, n, 1.0 / n.max(1) as f64)
}

/// Population standard deviation over `n` bars.
pub fn stdev(x: &[f64], n: usize) -> Series {
    let m = sma(x, n);
    let mut out = vec![NAN; x.len()];
    for i in 0..x.len() {
        if m[i].is_finite() {
            let w = &x[i + 1 - n..=i];
            out[i] = (w.iter().map(|v| (v - m[i]).powi(2)).sum::<f64>() / n as f64).sqrt();
        }
    }
    out
}

pub fn highest(x: &[f64], n: usize) -> Series {
    window(x, n, f64::max)
}

pub fn lowest(x: &[f64], n: usize) -> Series {
    window(x, n, f64::min)
}

fn window(x: &[f64], n: usize, f: fn(f64, f64) -> f64) -> Series {
    let mut out = vec![NAN; x.len()];
    if n == 0 {
        return out;
    }
    for i in (n - 1)..x.len() {
        let w = &x[i + 1 - n..=i];
        if w.iter().all(|v| v.is_finite()) {
            out[i] = w.iter().copied().reduce(f).unwrap();
        }
    }
    out
}

/// `x` shifted `k` bars later: `out[i] = x[i - k]`. Only ever backwards in time.
pub fn lag(x: &[f64], k: usize) -> Series {
    (0..x.len()).map(|i| if i >= k { x[i - k] } else { NAN }).collect()
}

pub fn true_range(f: &Frame) -> Series {
    (0..f.len())
        .map(|i| {
            if i == 0 {
                f.h[0] - f.l[0]
            } else {
                (f.h[i] - f.l[i]).max((f.h[i] - f.c[i - 1]).abs()).max((f.l[i] - f.c[i - 1]).abs())
            }
        })
        .collect()
}

// ------------------------------------------------------------------ trend

pub fn atr(f: &Frame, n: usize) -> Series {
    rma(&true_range(f), n)
}

/// MACD line, signal line and histogram.
pub fn macd(c: &[f64], fast: usize, slow: usize, signal: usize) -> (Series, Series, Series) {
    let (ef, es) = (ema(c, fast), ema(c, slow));
    let line: Series = ef.iter().zip(&es).map(|(a, b)| a - b).collect();
    let sig = ema(&line, signal);
    let hist = line.iter().zip(&sig).map(|(a, b)| a - b).collect();
    (line, sig, hist)
}

/// ADX with +DI and -DI (Wilder).
pub fn adx(f: &Frame, n: usize) -> (Series, Series, Series) {
    let len = f.len();
    let (mut pdm, mut mdm, mut tr) = (vec![NAN; len], vec![NAN; len], vec![NAN; len]);
    let tr_all = true_range(f);
    for i in 1..len {
        let up = f.h[i] - f.h[i - 1];
        let down = f.l[i - 1] - f.l[i];
        pdm[i] = if up > down && up > 0.0 { up } else { 0.0 };
        mdm[i] = if down > up && down > 0.0 { down } else { 0.0 };
        tr[i] = tr_all[i];
    }
    let (sp, sm, st) = (rma(&pdm, n), rma(&mdm, n), rma(&tr, n));
    let mut pdi = vec![NAN; len];
    let mut mdi = vec![NAN; len];
    let mut dx = vec![NAN; len];
    for i in 0..len {
        if st[i].is_finite() && st[i] > 0.0 {
            pdi[i] = 100.0 * sp[i] / st[i];
            mdi[i] = 100.0 * sm[i] / st[i];
            let s = pdi[i] + mdi[i];
            dx[i] = if s > 0.0 { 100.0 * (pdi[i] - mdi[i]).abs() / s } else { 0.0 };
        }
    }
    (rma(&dx, n), pdi, mdi)
}

/// Supertrend line and direction (+1 up, -1 down).
pub fn supertrend(f: &Frame, n: usize, mult: f64) -> (Series, Series) {
    let len = f.len();
    let a = atr(f, n);
    let (mut line, mut dir) = (vec![NAN; len], vec![NAN; len]);
    let (mut up_band, mut lo_band) = (NAN, NAN);
    let mut d = -1.0;
    for i in 0..len {
        if !a[i].is_finite() {
            continue;
        }
        let mid = (f.h[i] + f.l[i]) / 2.0;
        let (bu, bl) = (mid + mult * a[i], mid - mult * a[i]);
        let first = !up_band.is_finite();
        let prev_c = if i > 0 { f.c[i - 1] } else { f.c[i] };
        let nu = if first || bu < up_band || prev_c > up_band { bu } else { up_band };
        let nl = if first || bl > lo_band || prev_c < lo_band { bl } else { lo_band };
        if first {
            d = if f.c[i] > nu { 1.0 } else { -1.0 };
        } else if d < 0.0 && f.c[i] > nu {
            d = 1.0;
        } else if d > 0.0 && f.c[i] < nl {
            d = -1.0;
        }
        up_band = nu;
        lo_band = nl;
        line[i] = if d > 0.0 { nl } else { nu };
        dir[i] = d;
    }
    (line, dir)
}

/// Ichimoku: conversion line, base line, and the two cloud spans as plotted at each bar.
pub fn ichimoku(f: &Frame, conv: usize, base: usize, span_b: usize, displacement: usize) -> (Series, Series, Series, Series) {
    let mid = |n: usize| -> Series { highest(&f.h, n).iter().zip(lowest(&f.l, n)).map(|(h, l)| (h + l) / 2.0).collect() };
    let (tenkan, kijun, sb) = (mid(conv), mid(base), mid(span_b));
    let a_raw: Series = tenkan.iter().zip(&kijun).map(|(t, k)| (t + k) / 2.0).collect();
    (tenkan.clone(), kijun.clone(), lag(&a_raw, displacement), lag(&sb, displacement))
}

/// Donchian channel of the **previous** `n` bars (see the module note).
pub fn donchian(f: &Frame, n: usize) -> (Series, Series, Series) {
    let (u, l) = (lag(&highest(&f.h, n), 1), lag(&lowest(&f.l, n), 1));
    let m = u.iter().zip(&l).map(|(a, b)| (a + b) / 2.0).collect();
    (u, l, m)
}

// ------------------------------------------------------------------ momentum

/// Wilder RSI. 100 when there were no losses in the window, 50 on a completely flat window.
pub fn rsi(c: &[f64], n: usize) -> Series {
    let len = c.len();
    let (mut gain, mut loss) = (vec![NAN; len], vec![NAN; len]);
    for i in 1..len {
        let d = c[i] - c[i - 1];
        gain[i] = d.max(0.0);
        loss[i] = (-d).max(0.0);
    }
    let (ag, al) = (rma(&gain, n), rma(&loss, n));
    (0..len)
        .map(|i| match (ag[i], al[i]) {
            (g, l) if !g.is_finite() || !l.is_finite() => NAN,
            (g, l) if l == 0.0 => if g == 0.0 { 50.0 } else { 100.0 },
            (g, l) => 100.0 - 100.0 / (1.0 + g / l),
        })
        .collect()
}

/// Stochastic RSI %K and %D, 0..100.
pub fn stoch_rsi(c: &[f64], rsi_n: usize, stoch_n: usize, k: usize, d: usize) -> (Series, Series) {
    let r = rsi(c, rsi_n);
    let (hi, lo) = (highest(&r, stoch_n), lowest(&r, stoch_n));
    let raw: Series = (0..c.len())
        .map(|i| {
            let span = hi[i] - lo[i];
            if !span.is_finite() {
                NAN
            } else if span == 0.0 {
                50.0
            } else {
                100.0 * (r[i] - lo[i]) / span
            }
        })
        .collect();
    let kk = sma(&raw, k);
    let dd = sma(&kk, d);
    (kk, dd)
}

/// Commodity Channel Index on the typical price.
pub fn cci(f: &Frame, n: usize) -> Series {
    let tp: Series = (0..f.len()).map(|i| (f.h[i] + f.l[i] + f.c[i]) / 3.0).collect();
    let m = sma(&tp, n);
    (0..f.len())
        .map(|i| {
            if !m[i].is_finite() {
                return NAN;
            }
            let md = tp[i + 1 - n..=i].iter().map(|v| (v - m[i]).abs()).sum::<f64>() / n as f64;
            if md == 0.0 { 0.0 } else { (tp[i] - m[i]) / (0.015 * md) }
        })
        .collect()
}

/// Rate of change in percent over `n` bars.
pub fn roc(c: &[f64], n: usize) -> Series {
    (0..c.len()).map(|i| if i >= n && c[i - n] != 0.0 { 100.0 * (c[i] - c[i - n]) / c[i - n] } else { NAN }).collect()
}

// ------------------------------------------------------------------ mean reversion

/// Bollinger Bands: (upper, middle, lower, %B, bandwidth).
pub fn bollinger(c: &[f64], n: usize, k: f64) -> (Series, Series, Series, Series, Series) {
    let (m, sd) = (sma(c, n), stdev(c, n));
    let len = c.len();
    let (mut u, mut l, mut pb, mut w) = (vec![NAN; len], vec![NAN; len], vec![NAN; len], vec![NAN; len]);
    for i in 0..len {
        if m[i].is_finite() {
            u[i] = m[i] + k * sd[i];
            l[i] = m[i] - k * sd[i];
            let span = u[i] - l[i];
            pb[i] = if span > 0.0 { (c[i] - l[i]) / span } else { 0.5 };
            w[i] = if m[i] != 0.0 { span / m[i] } else { NAN };
        }
    }
    (u, m, l, pb, w)
}

/// Rolling VWAP over the last `n` bars on the typical price, and the close's distance from it in percent.
pub fn vwap(f: &Frame, n: usize) -> (Series, Series) {
    let len = f.len();
    let (mut v, mut dev) = (vec![NAN; len], vec![NAN; len]);
    if n == 0 {
        return (v, dev);
    }
    for i in (n - 1)..len {
        let (mut pv, mut vol) = (0.0, 0.0);
        for j in i + 1 - n..=i {
            pv += (f.h[j] + f.l[j] + f.c[j]) / 3.0 * f.v[j];
            vol += f.v[j];
        }
        if vol > 0.0 {
            v[i] = pv / vol;
            dev[i] = 100.0 * (f.c[i] - v[i]) / v[i];
        }
    }
    (v, dev)
}

/// Z-Score of the close against its `n`-bar mean. 0 on a flat window.
pub fn zscore(c: &[f64], n: usize) -> Series {
    let (m, sd) = (sma(c, n), stdev(c, n));
    (0..c.len()).map(|i| if !m[i].is_finite() { NAN } else if sd[i] == 0.0 { 0.0 } else { (c[i] - m[i]) / sd[i] }).collect()
}

// ------------------------------------------------------------------ volume

pub fn obv(f: &Frame) -> Series {
    let mut out = vec![0.0; f.len()];
    for i in 1..f.len() {
        out[i] = out[i - 1] + if f.c[i] > f.c[i - 1] { f.v[i] } else if f.c[i] < f.c[i - 1] { -f.v[i] } else { 0.0 };
    }
    out
}

/// This bar's volume divided by the average of the **previous** `n` bars (a spike is judged against the past).
pub fn volume_ratio(f: &Frame, n: usize) -> Series {
    let avg = lag(&sma(&f.v, n), 1);
    (0..f.len()).map(|i| if avg[i].is_finite() && avg[i] > 0.0 { f.v[i] / avg[i] } else { NAN }).collect()
}

/// Volume-weighted momentum: the volume-weighted mean bar return over `n` bars, in percent.
pub fn vw_momentum(f: &Frame, n: usize) -> Series {
    let len = f.len();
    let mut out = vec![NAN; len];
    for i in n..len {
        let (mut rv, mut vol) = (0.0, 0.0);
        for j in i + 1 - n..=i {
            rv += (f.c[j] / f.c[j - 1] - 1.0) * f.v[j];
            vol += f.v[j];
        }
        if vol > 0.0 {
            out[i] = 100.0 * rv / vol;
        }
    }
    out
}

// ------------------------------------------------------------------ registry

/// One parameter of an indicator: name, default, allowed range, whole number or not.
pub struct Param {
    pub name: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub integer: bool,
}

const fn p(name: &'static str, default: f64, min: f64, max: f64) -> Param {
    Param { name, default, min, max, integer: true }
}

const fn pf(name: &'static str, default: f64, min: f64, max: f64) -> Param {
    Param { name, default, min, max, integer: false }
}

pub struct Spec {
    pub kind: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub params: &'static [Param],
    pub outputs: &'static [&'static str],
    pub about: &'static str,
}

pub const SPECS: &[Spec] = &[
    Spec { kind: "sma", name: "Simple moving average", group: "trend", params: &[p("period", 20.0, 1.0, 500.0)], outputs: &["value"], about: "Mean close of the last N bars." },
    Spec { kind: "ema", name: "Exponential moving average", group: "trend", params: &[p("period", 20.0, 1.0, 500.0)], outputs: &["value"], about: "Weighted average that reacts faster to recent closes." },
    Spec { kind: "macd", name: "MACD", group: "trend", params: &[p("fast", 12.0, 1.0, 200.0), p("slow", 26.0, 2.0, 400.0), p("signal", 9.0, 1.0, 100.0)], outputs: &["macd", "signal", "hist"], about: "Fast EMA minus slow EMA, with a signal line." },
    Spec { kind: "adx", name: "ADX / DMI", group: "trend", params: &[p("period", 14.0, 2.0, 200.0)], outputs: &["adx", "plus_di", "minus_di"], about: "Trend strength (ADX) and direction (+DI vs -DI), Wilder." },
    Spec { kind: "supertrend", name: "Supertrend", group: "trend", params: &[p("period", 10.0, 2.0, 200.0), pf("multiplier", 3.0, 0.1, 20.0)], outputs: &["line", "direction"], about: "ATR trailing band; direction is +1 in an uptrend and -1 in a downtrend." },
    Spec { kind: "ichimoku", name: "Ichimoku Cloud", group: "advanced", params: &[p("conversion", 9.0, 1.0, 200.0), p("base", 26.0, 1.0, 300.0), p("span_b", 52.0, 1.0, 400.0), p("displacement", 26.0, 0.0, 300.0)], outputs: &["tenkan", "kijun", "span_a", "span_b"], about: "Conversion and base lines plus the cloud as plotted at each bar. No lagging span: it needs future bars." },
    Spec { kind: "donchian", name: "Donchian Channel", group: "advanced", params: &[p("period", 20.0, 1.0, 500.0)], outputs: &["upper", "lower", "middle"], about: "High and low of the previous N bars (excludes the current bar)." },
    Spec { kind: "atr", name: "Average True Range", group: "advanced", params: &[p("period", 14.0, 1.0, 200.0)], outputs: &["value"], about: "Wilder-smoothed true range: how far price usually moves per bar." },
    Spec { kind: "rsi", name: "RSI", group: "momentum", params: &[p("period", 14.0, 2.0, 200.0)], outputs: &["value"], about: "Wilder relative strength, 0 to 100." },
    Spec { kind: "stoch_rsi", name: "Stochastic RSI", group: "momentum", params: &[p("rsi_period", 14.0, 2.0, 200.0), p("stoch_period", 14.0, 2.0, 200.0), p("k", 3.0, 1.0, 50.0), p("d", 3.0, 1.0, 50.0)], outputs: &["k", "d"], about: "Where RSI sits in its recent range, smoothed, 0 to 100." },
    Spec { kind: "cci", name: "CCI", group: "momentum", params: &[p("period", 20.0, 2.0, 200.0)], outputs: &["value"], about: "Typical price versus its mean, in units of mean deviation." },
    Spec { kind: "roc", name: "Rate of Change", group: "momentum", params: &[p("period", 10.0, 1.0, 500.0)], outputs: &["value"], about: "Percent change over N bars." },
    Spec { kind: "bollinger", name: "Bollinger Bands", group: "mean_reversion", params: &[p("period", 20.0, 2.0, 500.0), pf("stddev", 2.0, 0.1, 10.0)], outputs: &["upper", "middle", "lower", "pct_b", "width"], about: "SMA plus and minus K population standard deviations." },
    Spec { kind: "vwap", name: "Rolling VWAP", group: "mean_reversion", params: &[p("period", 20.0, 1.0, 500.0)], outputs: &["value", "deviation_pct"], about: "Volume-weighted typical price of the last N bars, and the close's distance from it in percent." },
    Spec { kind: "zscore", name: "Z-Score", group: "mean_reversion", params: &[p("period", 20.0, 2.0, 500.0)], outputs: &["value"], about: "How many standard deviations the close is from its N-bar mean." },
    Spec { kind: "obv", name: "On-Balance Volume", group: "volume", params: &[], outputs: &["value"], about: "Running total of volume, added on up closes and subtracted on down closes." },
    Spec { kind: "obv_ema", name: "OBV signal (EMA of OBV)", group: "volume", params: &[p("period", 20.0, 1.0, 500.0)], outputs: &["value"], about: "EMA of OBV, to compare OBV against." },
    Spec { kind: "volume_ratio", name: "Volume breakout ratio", group: "volume", params: &[p("period", 20.0, 1.0, 500.0)], outputs: &["value"], about: "This bar's volume divided by the average of the previous N bars." },
    Spec { kind: "vw_momentum", name: "Volume-weighted momentum", group: "volume", params: &[p("period", 14.0, 1.0, 500.0)], outputs: &["value"], about: "Volume-weighted mean bar return over N bars, percent." },
];

pub fn spec(kind: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.kind == kind)
}

/// Checked parameter values in declaration order, defaults filled in. Unknown names are rejected.
pub fn resolve_params(s: &Spec, given: Option<&Map<String, Value>>) -> Result<Vec<f64>, String> {
    if let Some(g) = given {
        if let Some(k) = g.keys().find(|k| !s.params.iter().any(|p| p.name == k.as_str())) {
            return Err(format!("{}: unknown parameter '{k}'", s.kind));
        }
    }
    let mut out = Vec::with_capacity(s.params.len());
    for p in s.params {
        let v = match given.and_then(|g| g.get(p.name)) {
            None | Some(Value::Null) => p.default,
            Some(v) => v.as_f64().ok_or_else(|| format!("{}.{} must be a number", s.kind, p.name))?,
        };
        if !v.is_finite() || v < p.min || v > p.max {
            return Err(format!("{}.{} must be between {} and {}", s.kind, p.name, p.min, p.max));
        }
        if p.integer && v.fract() != 0.0 {
            return Err(format!("{}.{} must be a whole number", s.kind, p.name));
        }
        out.push(v);
    }
    if s.kind == "macd" && out[0] >= out[1] {
        return Err("macd.fast must be smaller than macd.slow".into());
    }
    Ok(out)
}

/// Compute an indicator by name. `params` must come from [`resolve_params`].
pub fn compute(kind: &str, params: &[f64], f: &Frame) -> Result<Outputs, String> {
    let u = |i: usize| params[i] as usize;
    let mut o = Outputs::new();
    match kind {
        "sma" => {
            o.insert("value", sma(&f.c, u(0)));
        }
        "ema" => {
            o.insert("value", ema(&f.c, u(0)));
        }
        "macd" => {
            let (a, b, c) = macd(&f.c, u(0), u(1), u(2));
            o.insert("macd", a);
            o.insert("signal", b);
            o.insert("hist", c);
        }
        "adx" => {
            let (a, p, m) = adx(f, u(0));
            o.insert("adx", a);
            o.insert("plus_di", p);
            o.insert("minus_di", m);
        }
        "supertrend" => {
            let (l, d) = supertrend(f, u(0), params[1]);
            o.insert("line", l);
            o.insert("direction", d);
        }
        "ichimoku" => {
            let (t, k, a, b) = ichimoku(f, u(0), u(1), u(2), u(3));
            o.insert("tenkan", t);
            o.insert("kijun", k);
            o.insert("span_a", a);
            o.insert("span_b", b);
        }
        "donchian" => {
            let (a, b, c) = donchian(f, u(0));
            o.insert("upper", a);
            o.insert("lower", b);
            o.insert("middle", c);
        }
        "atr" => {
            o.insert("value", atr(f, u(0)));
        }
        "rsi" => {
            o.insert("value", rsi(&f.c, u(0)));
        }
        "stoch_rsi" => {
            let (k, d) = stoch_rsi(&f.c, u(0), u(1), u(2), u(3));
            o.insert("k", k);
            o.insert("d", d);
        }
        "cci" => {
            o.insert("value", cci(f, u(0)));
        }
        "roc" => {
            o.insert("value", roc(&f.c, u(0)));
        }
        "bollinger" => {
            let (a, b, c, d, e) = bollinger(&f.c, u(0), params[1]);
            o.insert("upper", a);
            o.insert("middle", b);
            o.insert("lower", c);
            o.insert("pct_b", d);
            o.insert("width", e);
        }
        "vwap" => {
            let (v, d) = vwap(f, u(0));
            o.insert("value", v);
            o.insert("deviation_pct", d);
        }
        "zscore" => {
            o.insert("value", zscore(&f.c, u(0)));
        }
        "obv" => {
            o.insert("value", obv(f));
        }
        "obv_ema" => {
            o.insert("value", ema(&obv(f), u(0)));
        }
        "volume_ratio" => {
            o.insert("value", volume_ratio(f, u(0)));
        }
        "vw_momentum" => {
            o.insert("value", vw_momentum(f, u(0)));
        }
        other => return Err(format!("unknown indicator '{other}'")),
    }
    Ok(o)
}

/// The registry as JSON, for the Strategy Library page.
pub fn catalog() -> Value {
    Value::Array(
        SPECS
            .iter()
            .map(|s| {
                json!({
                    "kind": s.kind, "name": s.name, "group": s.group, "about": s.about, "outputs": s.outputs,
                    "params": s.params.iter().map(|p| json!({ "name": p.name, "default": p.default, "min": p.min, "max": p.max, "integer": p.integer })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(eq: &[f64], want: &[f64], tol: f64) {
        assert_eq!(eq.len(), want.len());
        for (i, (a, b)) in eq.iter().zip(want).enumerate() {
            if b.is_nan() {
                assert!(a.is_nan(), "index {i}: expected NaN, got {a}");
            } else {
                assert!((a - b).abs() <= tol, "index {i}: got {a}, expected {b}");
            }
        }
    }

    fn frame(c: &[f64]) -> Frame {
        let mut f = Frame::default();
        for (i, &x) in c.iter().enumerate() {
            let o = if i == 0 { x } else { c[i - 1] };
            f.push(i as i64 * 60, o, o.max(x) + 1.0, o.min(x) - 1.0, x, 100.0 + (i % 7) as f64 * 10.0);
        }
        f
    }

    /// Deterministic wiggly prices for the property tests.
    fn walk(n: usize) -> Frame {
        let mut s: u64 = 42;
        let mut x = 100.0;
        let mut f = Frame::default();
        for i in 0..n {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let r = (s % 1000) as f64 / 1000.0 - 0.5;
            let o = x;
            x *= 1.0 + r * 0.04;
            let span = ((s >> 20) % 100) as f64 / 100.0 * 1.5;
            f.push(i as i64 * 60, o, o.max(x) + span, o.min(x) - span, x, 1000.0 + ((s >> 30) % 5000) as f64);
        }
        f
    }

    #[test]
    fn sma_ema_rma_by_hand() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        close(&sma(&x, 3), &[NAN, NAN, 2.0, 3.0, 4.0], 1e-12);
        // alpha 0.5, seed = mean(1,2,3) = 2
        close(&ema(&x, 3), &[NAN, NAN, 2.0, 3.0, 4.0], 1e-12);
        // alpha 1/3: 2 + (4-2)/3, then + (5 - that)/3
        let r3 = 2.0 + 2.0 / 3.0;
        close(&rma(&x, 3), &[NAN, NAN, 2.0, r3, r3 + (5.0 - r3) / 3.0], 1e-12);
        // leading NaNs (a MACD line) are skipped, not propagated forever
        close(&ema(&[NAN, NAN, 1.0, 2.0, 3.0, 4.0], 2), &[NAN, NAN, NAN, 1.5, 1.5 + (3.0 - 1.5) * 2.0 / 3.0, 2.5 + (4.0 - 2.5) * 2.0 / 3.0], 1e-12);
    }

    #[test]
    fn rsi_matches_the_published_wilder_example() {
        // The 14-period worked example published by StockCharts ("RSI" in ChartSchool).
        let c = [44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89, 46.03, 45.61, 46.28, 46.28, 46.00, 46.03, 46.41, 46.22, 45.64];
        let r = rsi(&c, 14);
        assert!(r[13].is_nan());
        let want = [70.53, 66.32, 66.55, 69.41, 66.36, 57.97];
        for (k, w) in want.iter().enumerate() {
            assert!((r[14 + k] - w).abs() < 0.1, "rsi[{}] = {} want {w}", 14 + k, r[14 + k]);
        }
        assert_eq!(rsi(&[1.0, 2.0, 3.0, 4.0], 2)[3], 100.0);
        assert_eq!(rsi(&[5.0, 5.0, 5.0, 5.0], 2)[3], 50.0);
    }

    #[test]
    fn bollinger_and_zscore_use_population_stdev() {
        let c = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]; // mean 5, population sd 2
        let (u, m, l, pb, _) = bollinger(&c, 8, 2.0);
        assert!((m[7] - 5.0).abs() < 1e-12 && (u[7] - 9.0).abs() < 1e-12 && (l[7] - 1.0).abs() < 1e-12);
        assert!((pb[7] - 1.0).abs() < 1e-12); // close 9 is on the upper band
        assert!((zscore(&c, 8)[7] - 2.0).abs() < 1e-12);
        assert_eq!(zscore(&[3.0; 5], 5)[4], 0.0);
    }

    #[test]
    fn atr_true_range_and_obv_by_hand() {
        let mut f = Frame::default();
        f.push(0, 10.0, 12.0, 9.0, 11.0, 100.0); // TR 3
        f.push(1, 11.0, 15.0, 10.0, 14.0, 200.0); // TR max(5, 4, 1) = 5
        f.push(2, 14.0, 14.5, 8.0, 9.0, 300.0); // TR max(6.5, 0.5, 6) = 6.5
        close(&true_range(&f), &[3.0, 5.0, 6.5], 1e-12);
        close(&atr(&f, 2), &[NAN, 4.0, 4.0 + (6.5 - 4.0) / 2.0], 1e-12);
        close(&obv(&f), &[0.0, 200.0, -100.0], 1e-12);
    }

    #[test]
    fn roc_cci_donchian_volume_by_hand() {
        close(&roc(&[100.0, 110.0, 121.0], 1), &[NAN, 10.0, 10.0], 1e-9);
        let mut f = Frame::default();
        for (i, (h, l)) in [(10.0, 5.0), (12.0, 6.0), (11.0, 4.0), (13.0, 9.0)].iter().enumerate() {
            f.push(i as i64, *l, *h, *l, *h, if i == 3 { 400.0 } else { 100.0 });
        }
        let (u, lo, _) = donchian(&f, 2);
        close(&u, &[NAN, NAN, 12.0, 12.0], 0.0); // the previous two bars, never the current one
        close(&lo, &[NAN, NAN, 5.0, 4.0], 0.0);
        close(&volume_ratio(&f, 2), &[NAN, NAN, 1.0, 4.0], 1e-12);
        // CCI on a flat typical price is 0, not a division by zero
        let flat = frame(&[5.0; 25]);
        assert!(cci(&flat, 20)[24].abs() < 1e-9);
    }

    #[test]
    fn supertrend_flips_with_the_trend() {
        let mut c: Vec<f64> = (0..60).map(|i| 100.0 + i as f64).collect();
        c.extend((0..60).map(|i| 160.0 - 2.0 * i as f64));
        let (line, dir) = supertrend(&frame(&c), 10, 3.0);
        assert_eq!(dir[55], 1.0);
        assert!(line[55] < c[55]);
        assert_eq!(dir[115], -1.0);
        assert!(line[115] > c[115]);
    }

    #[test]
    fn adx_is_high_in_a_clean_trend_and_di_points_the_right_way() {
        let c: Vec<f64> = (0..100).map(|i| 100.0 + i as f64).collect();
        let (a, pdi, mdi) = adx(&frame(&c), 14);
        assert!(a[26].is_nan() && a[27].is_finite(), "first ADX at 2n-1");
        assert!(a[99] > 50.0 && pdi[99] > mdi[99]);
    }

    #[test]
    fn macd_and_stoch_rsi_shapes() {
        let f = walk(300);
        let (m, s, h) = macd(&f.c, 12, 26, 9);
        assert!(m[24].is_nan() && m[25].is_finite() && s[32].is_nan() && s[33].is_finite());
        assert!((h[200] - (m[200] - s[200])).abs() < 1e-12);
        let (k, d) = stoch_rsi(&f.c, 14, 14, 3, 3);
        assert!(k.iter().chain(&d).filter(|v| v.is_finite()).all(|v| (0.0..=100.0).contains(v)));
    }

    #[test]
    fn ichimoku_spans_are_lagged_not_shifted_forward() {
        let f = walk(200);
        let (t, k, a, b) = ichimoku(&f, 9, 26, 52, 26);
        let i = 150;
        assert!((a[i] - (t[i - 26] + k[i - 26]) / 2.0).abs() < 1e-12);
        let hb = f.h[i - 26 - 51..=i - 26].iter().copied().fold(f64::MIN, f64::max);
        let lb = f.l[i - 26 - 51..=i - 26].iter().copied().fold(f64::MAX, f64::min);
        assert!((b[i] - (hb + lb) / 2.0).abs() < 1e-12);
    }

    /// The anti-look-ahead property: computing on the first `k` bars gives exactly the values the full run
    /// gives for those bars. Any indicator that peeks ahead (a centred window, a forward shift) fails it.
    #[test]
    fn prefix_equals_full_for_every_indicator() {
        let full = walk(400);
        for s in SPECS {
            let params = resolve_params(s, None).unwrap();
            let all = compute(s.kind, &params, &full).unwrap();
            for k in [60, 137, 250] {
                let mut pre = Frame::default();
                for i in 0..k {
                    pre.push(full.t[i], full.o[i], full.h[i], full.l[i], full.c[i], full.v[i]);
                }
                let part = compute(s.kind, &params, &pre).unwrap();
                for (name, series) in &part {
                    for i in 0..k {
                        let (x, y) = (series[i], all[name][i]);
                        assert!((x.is_nan() && y.is_nan()) || x == y, "{}.{name} differs at {i} with prefix {k}: {x} vs {y}", s.kind);
                    }
                }
            }
        }
    }

    #[test]
    fn params_are_validated() {
        let m = spec("macd").unwrap();
        assert_eq!(resolve_params(m, None).unwrap(), vec![12.0, 26.0, 9.0]);
        let bad = |v: Value| resolve_params(m, v.as_object()).unwrap_err();
        assert!(bad(json!({"fast": 30})).contains("smaller"));
        assert!(bad(json!({"fast": 2.5})).contains("whole"));
        assert!(bad(json!({"nope": 1})).contains("unknown"));
        assert!(bad(json!({"slow": 0})).contains("between"));
        assert!(compute("nope", &[], &walk(10)).is_err());
    }
}
