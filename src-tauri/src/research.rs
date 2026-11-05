//! Advanced quantitative research: Monte Carlo v2, walk-forward v2, robustness score, A/B experiments.
//!
//! Everything is historical simulation through the same engine as backtests (`backtest2::run`). None of it has
//! an order path. Long runs check `cancelled()` between units and report `progress()`.

use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::{json, Value};

use crate::backtest2::{self, Axis, Config};
use crate::strategy::{self, Strategy};
use crate::zones::Frame;

fn r(x: f64, d: i32) -> f64 {
    let m = 10f64.powi(d);
    if x.is_finite() { (x * m).round() / m } else { x }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

// ------------------------------------------------------------------ Monte Carlo v2

#[derive(Clone, Debug)]
pub struct McParams {
    /// "bootstrap" (with replacement), "block" (contiguous blocks, keeps streaks), "shuffle" (reorder only)
    pub method: String,
    pub sims: usize,
    pub seed: u64,
    pub block: usize,
    /// trades per simulated path; default: as many as the source
    pub horizon: Option<usize>,
    /// extra cost subtracted from every trade (fees/slippage stress), in money
    pub extra_cost: f64,
    /// ruin = equity at or below this percent of the start
    pub ruin_pct: f64,
    pub dd_thresholds: Vec<f64>,
}

pub fn monte_carlo(pnls: &[f64], capital: f64, p: &McParams, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> Result<Value, String> {
    if pnls.len() < 5 {
        return Err("at least 5 trades are needed to resample".into());
    }
    if !["bootstrap", "block", "shuffle"].contains(&p.method.as_str()) {
        return Err("method must be bootstrap, block or shuffle".into());
    }
    if !(100..=100_000).contains(&p.sims) {
        return Err("simulations must be 100 to 100000".into());
    }
    if !(capital > 0.0) {
        return Err("capital must be above 0".into());
    }
    let n = pnls.len();
    let h = p.horizon.unwrap_or(n).clamp(1, 100_000);
    let block = p.block.clamp(1, n);
    let mut rng = StdRng::seed_from_u64(p.seed);
    let checkpoints = 20usize.min(h);
    let mut bands: Vec<Vec<f64>> = vec![vec![]; checkpoints];
    let (mut ends, mut dds, mut streaks) = (Vec::with_capacity(p.sims), Vec::with_capacity(p.sims), Vec::with_capacity(p.sims));
    let mut ruined = 0usize;
    let mut over: Vec<usize> = vec![0; p.dd_thresholds.len()];
    for s in 0..p.sims {
        if s % 250 == 0 {
            if cancelled() {
                return Err("cancelled".into());
            }
            progress(s as f64 / p.sims as f64);
        }
        let path: Vec<f64> = match p.method.as_str() {
            "shuffle" => {
                let mut v = pnls.to_vec();
                for i in (1..v.len()).rev() {
                    v.swap(i, rng.gen_range(0..=i));
                }
                v.into_iter().cycle().take(h).collect()
            }
            "block" => {
                let mut v = Vec::with_capacity(h);
                while v.len() < h {
                    let start = rng.gen_range(0..=n - block);
                    v.extend_from_slice(&pnls[start..start + block]);
                }
                v.truncate(h);
                v
            }
            _ => (0..h).map(|_| pnls[rng.gen_range(0..n)]).collect(),
        };
        let (mut eq, mut peak, mut dd, mut streak, mut worst) = (capital, capital, 0.0f64, 0usize, 0usize);
        let mut hit_ruin = false;
        for (k, x) in path.iter().enumerate() {
            eq += x - p.extra_cost;
            peak = peak.max(eq);
            if peak > 0.0 {
                dd = dd.max((peak - eq) / peak * 100.0);
            }
            if x - p.extra_cost <= 0.0 {
                streak += 1;
                worst = worst.max(streak);
            } else {
                streak = 0;
            }
            if eq <= capital * p.ruin_pct / 100.0 {
                hit_ruin = true;
            }
            let cp = (k + 1) * checkpoints / h;
            if (k + 1) * checkpoints % h == 0 && cp >= 1 {
                bands[cp - 1].push((eq / capital - 1.0) * 100.0);
            }
        }
        ruined += hit_ruin as usize;
        for (i, t) in p.dd_thresholds.iter().enumerate() {
            over[i] += (dd >= *t) as usize;
        }
        ends.push((eq / capital - 1.0) * 100.0);
        dds.push(dd);
        streaks.push(worst as f64);
    }
    let q = |v: &mut Vec<f64>| json!({ "p5": r(pct(v, 0.05), 3), "p25": r(pct(v, 0.25), 3), "p50": r(pct(v, 0.5), 3), "p75": r(pct(v, 0.75), 3), "p95": r(pct(v, 0.95), 3) });
    let losses = ends.iter().filter(|x| **x < 0.0).count();
    let band_rows: Vec<Value> = bands.iter_mut().enumerate().map(|(i, b)| json!({ "trade": (i + 1) * h / checkpoints, "p5": r(pct(b, 0.05), 3), "p50": r(pct(b, 0.5), 3), "p95": r(pct(b, 0.95), 3) })).collect();
    progress(1.0);
    Ok(json!({
        "method": p.method, "simulations": p.sims, "seed": p.seed, "block": block, "trades_per_path": h, "source_trades": n, "extra_cost_per_trade": p.extra_cost,
        "return_pct": q(&mut ends), "max_drawdown_pct": q(&mut dds), "longest_losing_streak": q(&mut streaks),
        "probability_of_loss_pct": r(losses as f64 / p.sims as f64 * 100.0, 2),
        "risk_of_ruin_pct": r(ruined as f64 / p.sims as f64 * 100.0, 2), "ruin_level_pct_of_capital": p.ruin_pct,
        "drawdown_exceedance": p.dd_thresholds.iter().zip(&over).map(|(t, c)| json!({ "threshold_pct": t, "probability_pct": r(*c as f64 / p.sims as f64 * 100.0, 2) })).collect::<Vec<_>>(),
        "equity_bands": band_rows,
        "assumptions": "Trades are resampled as if independent (block resampling keeps short runs together). Past trades do not have to repeat. These are simulated frequencies, not guarantees.",
        "simulated_utc": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
}

// ------------------------------------------------------------------ walk-forward v2

#[derive(Clone, Debug)]
pub struct WfParams {
    /// "anchored" (training grows from the start) or "rolling" (fixed-size training window)
    pub mode: String,
    pub train: usize,
    pub test: usize,
    pub step: usize,
    pub objective: String,
}

fn combos(spec: &Value, axes: &[Axis]) -> Result<Vec<(Value, Value)>, String> {
    let total: usize = axes.iter().map(|x| x.values.len().max(1)).product();
    let mut idx = vec![0usize; axes.len()];
    let mut out = vec![];
    for _ in 0..total.max(1) {
        let mut v = spec.clone();
        let mut chosen = serde_json::Map::new();
        for (q, ax) in axes.iter().enumerate() {
            *v.pointer_mut(&ax.path).ok_or_else(|| format!("{} is not a field of this strategy", ax.path))? = ax.values[idx[q]].clone();
            chosen.insert(ax.path.clone(), ax.values[idx[q]].clone());
        }
        for q in (0..axes.len()).rev() {
            idx[q] += 1;
            if idx[q] < axes[q].values.len() {
                break;
            }
            idx[q] = 0;
        }
        out.push((v, Value::Object(chosen)));
    }
    Ok(out)
}

fn score(m: &backtest2::Metrics, objective: &str) -> f64 {
    if m.trades == 0 {
        return f64::NEG_INFINITY;
    }
    match objective {
        "sharpe" => m.sharpe.unwrap_or(f64::NEG_INFINITY),
        "profit_factor" => m.profit_factor.unwrap_or(1e6),
        "max_drawdown" => -m.max_drawdown_pct,
        _ => m.return_pct,
    }
}

/// Folds are built from bar indices so training always ends before testing starts; the test run uses the whole
/// history up to the fold's end for indicator warm-up but opens trades only inside the test window.
pub fn walk_forward(spec: &Value, axes: &[Axis], a: &Frame, cfg: &Config, p: &WfParams, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> Result<Value, String> {
    if !["anchored", "rolling"].contains(&p.mode.as_str()) {
        return Err("mode must be anchored or rolling".into());
    }
    if p.train < 50 || p.test < 20 || p.step < 1 {
        return Err("training needs 50+ bars, testing 20+, step 1+".into());
    }
    let cs = combos(spec, axes)?;
    let n = a.len();
    let mut bounds = vec![];
    let mut start = 0usize;
    while start + p.train + p.test <= n {
        let tr_from = if p.mode == "anchored" { 0 } else { start };
        bounds.push((tr_from, start + p.train, (start + p.train + p.test).min(n)));
        start += p.step;
        if bounds.len() >= 30 {
            break;
        }
    }
    if bounds.is_empty() {
        return Err(format!("{n} bars cannot hold one fold of {} + {} bars", p.train, p.test));
    }
    if cs.len() * bounds.len() > 600 {
        return Err(format!("{} runs (combinations x folds); at most 600", cs.len() * bounds.len()));
    }
    let mut folds = vec![];
    let mut chosen_count: std::collections::BTreeMap<String, usize> = Default::default();
    let mut oos_returns = vec![];
    for (k, (tf, te, se)) in bounds.iter().enumerate() {
        if cancelled() {
            return Err("cancelled".into());
        }
        progress(k as f64 / bounds.len() as f64);
        let train = backtest2::slice(a, *tf, *te);
        let mut best: Option<(f64, Value, Strategy)> = None;
        let mut errors = 0;
        for (v, chosen) in &cs {
            let Ok(st) = strategy::parse(v) else {
                errors += 1;
                continue;
            };
            match backtest2::run(&st, &train, None, &Config { trade_from: 0, ..*cfg }) {
                Ok(rep) => {
                    let sc = score(&rep.metrics, &p.objective);
                    if best.as_ref().is_none_or(|b| sc > b.0) {
                        best = Some((sc, chosen.clone(), st));
                    }
                }
                Err(_) => errors += 1,
            }
        }
        let fold = match best {
            Some((sc, params, st)) if sc.is_finite() => {
                let test = backtest2::slice(a, 0, *se);
                match backtest2::run(&st, &test, None, &Config { trade_from: *te, ..*cfg }) {
                    Ok(oos) => {
                        oos_returns.push(oos.metrics.return_pct);
                        *chosen_count.entry(params.to_string()).or_default() += 1;
                        json!({ "fold": k + 1, "status": "ok", "train": { "from": a.t[*tf], "to": a.t[*te - 1], "bars": te - tf }, "test": { "from": a.t[*te], "to": a.t[*se - 1], "bars": se - te }, "chosen": params, "in_sample_score": sc, "out_of_sample": oos.metrics, "invalid_combinations": errors })
                    }
                    Err(e) => json!({ "fold": k + 1, "status": "failed", "reason": e }),
                }
            }
            _ => json!({ "fold": k + 1, "status": "failed", "reason": "no parameter combination traded in the training window", "train": { "from": a.t[*tf], "to": a.t[*te - 1] } }),
        };
        folds.push(fold);
    }
    let pos = oos_returns.iter().filter(|x| **x > 0.0).count();
    let mean = if oos_returns.is_empty() { f64::NAN } else { oos_returns.iter().sum::<f64>() / oos_returns.len() as f64 };
    let sd = if oos_returns.len() < 2 { f64::NAN } else { (oos_returns.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (oos_returns.len() - 1) as f64).sqrt() };
    let modal = chosen_count.values().copied().max().unwrap_or(0);
    let full = strategy::parse(spec).ok().and_then(|st| backtest2::run(&st, a, None, &Config { trade_from: 0, ..*cfg }).ok()).map(|x| x.metrics);
    progress(1.0);
    Ok(json!({
        "mode": p.mode, "train_bars": p.train, "test_bars": p.test, "step": p.step, "objective": p.objective, "combinations": cs.len(),
        "folds": folds, "failed_folds": folds.iter().filter(|f| f["status"] == "failed").count(),
        "stability": { "oos_folds": oos_returns.len(), "positive_folds_pct": if oos_returns.is_empty() { None } else { Some(r(pos as f64 / oos_returns.len() as f64 * 100.0, 1)) }, "mean_oos_return_pct": r(mean, 3), "sd_oos_return_pct": r(sd, 3),
            "parameter_consistency_pct": if oos_returns.is_empty() { None } else { Some(r(modal as f64 / oos_returns.len() as f64 * 100.0, 1)) }, "chosen_counts": chosen_count },
        "full_period_backtest_of_given_spec": full,
        "note": "Each fold chooses on bars that end before its test window starts. The best in-sample choice is not the robust result: read the out-of-sample columns and the stability block.",
    }))
}

// ------------------------------------------------------------------ robustness score

pub const SCORE_VERSION: &str = "robustness-v1";
const WEIGHTS: [(&str, f64); 9] = [
    ("out_of_sample", 20.0), ("walk_forward_stability", 15.0), ("parameter_sensitivity", 15.0), ("monte_carlo_drawdown", 15.0), ("sample_size", 10.0),
    ("cost_sensitivity", 10.0), ("regime_dependency", 5.0), ("profit_concentration", 5.0), ("data_quality", 5.0),
];

fn numeric_params(spec: &Value) -> Vec<(String, f64, bool)> {
    let mut out = vec![];
    for (i, ind) in spec["indicators"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        for (k, v) in ind["params"].as_object().cloned().unwrap_or_default() {
            if let Some(x) = v.as_f64() {
                out.push((format!("/indicators/{i}/params/{k}"), x, x.fract() == 0.0));
            }
        }
    }
    out
}

/// Score a strategy on a dataset. Every dimension carries its evidence; missing evidence lowers nothing and is
/// shown as unavailable. A score never approves anything: live deployment still needs a person.
pub fn robustness(spec: &Value, a: &Frame, cfg: &Config, data_warnings: usize, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> Result<Value, String> {
    let st = strategy::parse(spec)?;
    let base = backtest2::run(&st, a, None, cfg)?;
    let pnls: Vec<f64> = base.trades.iter().filter_map(|t| t.pnl.parse().ok()).collect();
    let mut dims = serde_json::Map::new();
    let clamp = |x: f64| x.clamp(0.0, 100.0);
    // sample size
    let n = base.metrics.trades as f64;
    dims.insert("sample_size".into(), json!({ "score": clamp((n - 20.0) / 80.0 * 100.0), "evidence": format!("{n} trades (0 at 20 or fewer, 100 at 100 or more)") }));
    if n < 20.0 {
        return Ok(json!({ "score_version": SCORE_VERSION, "status": "insufficient_data", "score": null, "trades": n, "dimensions": dims, "weights": WEIGHTS.iter().map(|(k, w)| json!({ "dimension": k, "weight": w })).collect::<Vec<_>>(), "note": "Fewer than 20 trades: no score, because any score would mostly be noise." }));
    }
    progress(0.1);
    // out of sample: last 30% traded only after the first 70% (indicators warm)
    let cut = (a.len() as f64 * 0.7) as usize;
    let is = backtest2::run(&st, &backtest2::slice(a, 0, cut), None, &Config { trade_from: 0, ..*cfg })?;
    let oos = backtest2::run(&st, a, None, &Config { trade_from: cut, ..*cfg })?;
    let oos_s = if oos.metrics.trades == 0 { 0.0 } else { clamp(50.0 + oos.metrics.return_pct * 5.0 + if oos.metrics.profit_factor.unwrap_or(0.0) > 1.0 { 20.0 } else { -20.0 }) };
    dims.insert("out_of_sample".into(), json!({ "score": oos_s, "evidence": { "in_sample_return_pct": is.metrics.return_pct, "out_of_sample_return_pct": oos.metrics.return_pct, "out_of_sample_trades": oos.metrics.trades, "out_of_sample_profit_factor": oos.metrics.profit_factor } }));
    if cancelled() {
        return Err("cancelled".into());
    }
    progress(0.3);
    // walk-forward stability of the fixed parameters: share of positive folds over 4 rolling windows
    let fold = a.len() / 5;
    let mut fold_ret = vec![];
    for k in 1..=4 {
        if let Ok(x) = backtest2::run(&st, &backtest2::slice(a, 0, ((k + 1) * fold).min(a.len())), None, &Config { trade_from: k * fold, ..*cfg }) {
            fold_ret.push(x.metrics.return_pct);
        }
    }
    let posf = fold_ret.iter().filter(|x| **x > 0.0).count() as f64;
    dims.insert("walk_forward_stability".into(), json!({ "score": if fold_ret.is_empty() { 0.0 } else { posf / fold_ret.len() as f64 * 100.0 }, "evidence": { "fold_returns_pct": fold_ret } }));
    progress(0.45);
    // parameter sensitivity: each indicator parameter +/-20%
    let mut variants = vec![];
    for (path, x, int) in numeric_params(spec).into_iter().take(6) {
        for f in [0.8, 1.2] {
            if cancelled() {
                return Err("cancelled".into());
            }
            let mut v = spec.clone();
            let nv = if int { json!((x * f).round().max(1.0)) } else { json!(x * f) };
            *v.pointer_mut(&path).unwrap() = nv.clone();
            if let Ok(s2) = strategy::parse(&v) {
                if let Ok(rep) = backtest2::run(&s2, a, None, cfg) {
                    variants.push(json!({ "param": path, "value": nv, "return_pct": rep.metrics.return_pct }));
                }
            }
        }
    }
    let same_sign = variants.iter().filter(|x| (x["return_pct"].as_f64().unwrap_or(0.0) > 0.0) == (base.metrics.return_pct > 0.0)).count();
    dims.insert("parameter_sensitivity".into(), if variants.is_empty() { json!({ "score": null, "evidence": "no numeric indicator parameters to vary" }) } else { json!({ "score": same_sign as f64 / variants.len() as f64 * 100.0, "evidence": { "base_return_pct": base.metrics.return_pct, "variants": variants } }) });
    progress(0.7);
    // Monte Carlo drawdown
    let mc = monte_carlo(&pnls, cfg.capital, &McParams { method: "bootstrap".into(), sims: 1000, seed: 42, block: 1, horizon: None, extra_cost: 0.0, ruin_pct: 50.0, dd_thresholds: vec![20.0] }, &|_| {}, cancelled)?;
    let dd95 = mc["max_drawdown_pct"]["p95"].as_f64().unwrap_or(100.0);
    dims.insert("monte_carlo_drawdown".into(), json!({ "score": clamp(100.0 - dd95 * 3.0), "evidence": { "max_drawdown_p95_pct": dd95, "probability_of_loss_pct": mc["probability_of_loss_pct"] } }));
    // cost sensitivity: double fees and slippage
    let costly = backtest2::run(&st, a, None, &Config { fee_pct: cfg.fee_pct * 2.0, slippage_pct: cfg.slippage_pct * 2.0, ..*cfg })?;
    dims.insert("cost_sensitivity".into(), json!({ "score": if costly.metrics.return_pct > 0.0 { 100.0 } else if base.metrics.return_pct > 0.0 { clamp(costly.metrics.return_pct / base.metrics.return_pct * 100.0 + 50.0) } else { 0.0 }, "evidence": { "return_pct": base.metrics.return_pct, "return_pct_with_double_costs": costly.metrics.return_pct } }));
    progress(0.85);
    // regime dependency: share of absolute profit from the single best regime
    let regs = backtest2::regimes(a, &base.trades);
    let tot: f64 = regs.as_array().map(|x| x.iter().filter(|r| r["regime"] == "uptrend" || r["regime"] == "downtrend").map(|r| r["net_pnl"].as_f64().unwrap_or(0.0).abs()).sum()).unwrap_or(0.0);
    let top = regs.as_array().map(|x| x.iter().filter(|r| r["regime"] == "uptrend" || r["regime"] == "downtrend").map(|r| r["net_pnl"].as_f64().unwrap_or(0.0).abs()).fold(0.0, f64::max)).unwrap_or(0.0);
    dims.insert("regime_dependency".into(), json!({ "score": if tot > 0.0 { clamp((1.0 - top / tot) * 200.0) } else { 0.0 }, "evidence": regs }));
    // profit concentration: share of gross profit from the best 10% of trades
    let mut wins: Vec<f64> = pnls.iter().copied().filter(|p| *p > 0.0).collect();
    wins.sort_by(|x, y| y.partial_cmp(x).unwrap());
    let k = (pnls.len() as f64 * 0.1).ceil() as usize;
    let gw: f64 = wins.iter().sum();
    let topk: f64 = wins.iter().take(k).sum();
    dims.insert("profit_concentration".into(), json!({ "score": if gw > 0.0 { clamp((1.0 - topk / gw) * 150.0) } else { 0.0 }, "evidence": format!("best {k} trades made {:.1}% of gross profit", if gw > 0.0 { topk / gw * 100.0 } else { 0.0 }) }));
    dims.insert("data_quality".into(), json!({ "score": clamp(100.0 - data_warnings as f64 * 34.0), "evidence": format!("{data_warnings} data-quality warning(s)") }));
    let (mut sum, mut wsum) = (0.0, 0.0);
    for (k, w) in WEIGHTS {
        if let Some(s) = dims.get(k).and_then(|d| d["score"].as_f64()) {
            sum += s * w;
            wsum += w;
        }
    }
    progress(1.0);
    Ok(json!({
        "score_version": SCORE_VERSION, "status": "scored", "score": r(sum / wsum, 1), "trades": n, "dimensions": dims,
        "weights": WEIGHTS.iter().map(|(k, w)| json!({ "dimension": k, "weight": w })).collect::<Vec<_>>(),
        "note": "A transparent summary of the evidence, not a forecast. A high score does not make a strategy profitable and never approves it for live trading.",
    }))
}

// ------------------------------------------------------------------ A/B experiments

/// Two strategies on the identical data window and assumptions, each in its own engine state. Pure
/// simulation: no orders, no accounts.
pub fn experiment(a_spec: &Value, b_spec: &Value, f: &Frame, cfg: &Config, min_trades: usize, seed: u64) -> Result<Value, String> {
    let (sa, sb) = (strategy::parse(a_spec).map_err(|e| format!("variant A: {e}"))?, strategy::parse(b_spec).map_err(|e| format!("variant B: {e}"))?);
    let ra = backtest2::run(&sa, f, None, cfg)?;
    let rb = backtest2::run(&sb, f, None, cfg)?;
    let pa: Vec<f64> = ra.trades.iter().filter_map(|t| t.pnl.parse().ok()).collect();
    let pb: Vec<f64> = rb.trades.iter().filter_map(|t| t.pnl.parse().ok()).collect();
    let enough = pa.len() >= min_trades && pb.len() >= min_trades;
    // bootstrap interval of the difference in mean trade P&L (B - A)
    let ci = if enough {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut diffs: Vec<f64> = (0..2000)
            .map(|_| {
                let ma = (0..pa.len()).map(|_| pa[rng.gen_range(0..pa.len())]).sum::<f64>() / pa.len() as f64;
                let mb = (0..pb.len()).map(|_| pb[rng.gen_range(0..pb.len())]).sum::<f64>() / pb.len() as f64;
                mb - ma
            })
            .collect();
        Some((pct(&mut diffs, 0.025), pct(&mut diffs, 0.975)))
    } else {
        None
    };
    let verdict = match ci {
        None => format!("not enough trades to compare (need {min_trades} each; A {}, B {})", pa.len(), pb.len()),
        Some((lo, _)) if lo > 0.0 => "B's mean trade is higher than A's, and the 95% interval excludes zero".into(),
        Some((_, hi)) if hi < 0.0 => "A's mean trade is higher than B's, and the 95% interval excludes zero".into(),
        Some(_) => "no clear difference: the 95% interval of the difference includes zero".into(),
    };
    Ok(json!({
        "window": { "from": f.t.first(), "to": f.t.last(), "bars": f.len() }, "assumptions": { "fee_pct": cfg.fee_pct, "slippage_pct": cfg.slippage_pct, "capital": cfg.capital },
        "a": { "metrics": ra.metrics, "trades": pa.len() }, "b": { "metrics": rb.metrics, "trades": pb.len() },
        "difference_in_mean_trade_ci95": ci.map(|(lo, hi)| json!([r(lo, 6), r(hi, 6)])), "seed": seed, "evidence": verdict,
        "execution": "simulated fills on historical bars (next-open, fees, slippage); not broker fills",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(n: usize, seed: u64) -> Frame {
        let mut s = seed;
        let mut x = 100.0;
        let mut f = Frame::default();
        for i in 0..n {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let o = x;
            x *= 1.0 + ((s % 1000) as f64 / 1000.0 - 0.5) * 0.03;
            f.push(1_700_000_000 + i as i64 * 3600, o, o.max(x) * 1.003, o.min(x) * 0.997, x, 1000.0);
        }
        f
    }

    fn cfg() -> Config {
        Config { capital: 10_000.0, fee_pct: 0.1, slippage_pct: 0.05, qty_step: [0.000001; 2], trade_from: 0, max_volume_pct: None }
    }

    fn mc(method: &str, seed: u64) -> McParams {
        McParams { method: method.into(), sims: 500, seed, block: 3, horizon: None, extra_cost: 0.0, ruin_pct: 50.0, dd_thresholds: vec![10.0] }
    }

    #[test]
    fn monte_carlo_known_cases_and_reproducibility() {
        let pnls = [10.0, -10.0, 10.0, -10.0, 10.0, -10.0];
        // reordering never changes the total: every path ends exactly flat
        let s = monte_carlo(&pnls, 1000.0, &mc("shuffle", 1), &|_| {}, &|| false).unwrap();
        assert_eq!((s["return_pct"]["p5"].as_f64(), s["return_pct"]["p95"].as_f64()), (Some(0.0), Some(0.0)));
        let a = monte_carlo(&pnls, 1000.0, &mc("block", 9), &|_| {}, &|| false).unwrap();
        let b = monte_carlo(&pnls, 1000.0, &mc("block", 9), &|_| {}, &|| false).unwrap();
        assert_eq!(a["return_pct"], b["return_pct"], "same inputs and seed, same result");
        // all losses: certain ruin at a 50% floor when the losses sum past it
        let ruin = monte_carlo(&[-200.0; 5], 1000.0, &mc("bootstrap", 2), &|_| {}, &|| false).unwrap();
        assert_eq!(ruin["risk_of_ruin_pct"], 100.0);
        assert!(monte_carlo(&pnls, 1000.0, &mc("bootstrap", 1), &|_| {}, &|| true).unwrap_err().contains("cancelled"));
        assert!(monte_carlo(&[1.0], 1000.0, &mc("bootstrap", 1), &|_| {}, &|| false).is_err());
    }

    #[test]
    fn walk_forward_folds_never_leak() {
        let a = walk(800, 3);
        let spec = strategy::templates().into_iter().find(|t| t["id"] == "ema_cross").unwrap()["spec"].clone();
        let axes = vec![Axis { path: "/indicators/0/params/period".into(), values: vec![json!(5), json!(9)] }];
        let p = WfParams { mode: "rolling".into(), train: 300, test: 100, step: 100, objective: "return".into() };
        let wf = walk_forward(&spec, &axes, &a, &cfg(), &p, &|_| {}, &|| false).unwrap();
        let folds = wf["folds"].as_array().unwrap();
        assert_eq!(folds.len(), 5);
        for f in folds.iter().filter(|f| f["status"] == "ok") {
            assert!(f["train"]["to"].as_i64() < f["test"]["from"].as_i64());
        }
        // rolling windows: fold 2 trains from bar 100, not bar 0
        assert_eq!(folds[1]["train"]["from"].as_i64(), Some(a.t[100]));
        // changing the data after fold 1's test window cannot change fold 1
        let mut later = a.clone();
        for i in 400..later.len() {
            later.c[i] *= 1.4;
            later.h[i] *= 1.4;
        }
        let wf2 = walk_forward(&spec, &axes, &later, &cfg(), &p, &|_| {}, &|| false).unwrap();
        assert_eq!(wf["folds"][0], wf2["folds"][0]);
        assert!(wf["stability"]["oos_folds"].as_u64().unwrap() > 0);
        assert!(walk_forward(&spec, &axes, &a, &cfg(), &WfParams { train: 900, ..p.clone() }, &|_| {}, &|| false).is_err());
    }

    #[test]
    fn robustness_is_explained_and_needs_samples() {
        let a = walk(1500, 11);
        let spec = strategy::templates().into_iter().find(|t| t["id"] == "ema_cross").unwrap()["spec"].clone();
        let r1 = robustness(&spec, &a, &cfg(), 0, &|_| {}, &|| false).unwrap();
        let r2 = robustness(&spec, &a, &cfg(), 0, &|_| {}, &|| false).unwrap();
        assert_eq!(r1, r2, "recalculable from the same evidence");
        if r1["status"] == "scored" {
            let s = r1["score"].as_f64().unwrap();
            assert!((0.0..=100.0).contains(&s));
            for (k, _) in WEIGHTS {
                assert!(r1["dimensions"].get(k).is_some(), "{k} missing");
            }
        }
        let quiet = strategy::templates().into_iter().find(|t| t["id"] == "breakout").unwrap()["spec"].clone();
        let r3 = robustness(&quiet, &walk(200, 5), &cfg(), 0, &|_| {}, &|| false).unwrap();
        assert_eq!(r3["status"], "insufficient_data");
        assert!(r3["score"].is_null());
    }

    #[test]
    fn experiments_compare_on_identical_data_and_place_nothing() {
        let a = walk(800, 21);
        let spec = strategy::templates().into_iter().find(|t| t["id"] == "ema_cross").unwrap()["spec"].clone();
        let same = experiment(&spec, &spec, &a, &cfg(), 5, 1).unwrap();
        assert_eq!(same["a"]["metrics"], same["b"]["metrics"], "identical variants, identical results");
        assert!(same["evidence"].as_str().unwrap().contains("no clear difference") || same["evidence"].as_str().unwrap().contains("not enough"));
        assert!(experiment(&spec, &json!({ "kind": "nope" }), &a, &cfg(), 5, 1).unwrap_err().contains("variant B"));
    }
}
