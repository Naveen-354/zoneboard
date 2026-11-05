//! Autonomous quant researcher: a bounded, deterministic search over the built-in strategy templates.
//!
//! What it can do: take a hypothesis (templates to try, an objective, a data window), vary each template's
//! numeric indicator periods on a small grid, backtest every candidate on the first 70% of the data, re-test
//! the top candidates on the untouched last 30%, score the best with the robustness score, and write a report
//! with the evidence. What it cannot do, by construction: this module takes frames and specs only. It has no
//! database connection, no account, no order path, no risk limit and no secret. A candidate is only ever a
//! proposal: a person reviews it (`review_status`), and approval publishes a strategy version. It never creates
//! or starts a bot.
//!
//! Bounds: at most 8 templates, at most 120 backtests, at most 5 out-of-sample re-tests. Same inputs, same report.

use serde_json::{json, Value};

use crate::backtest2::{self, Config};
use crate::research;
use crate::strategy;
use crate::zones::Frame;

pub const MAX_TEMPLATES: usize = 8;
pub const MAX_RUNS: usize = 120;
pub const PIPELINE_VERSION: &str = "researcher-v1";
const OBJECTIVES: [&str; 3] = ["return", "sharpe", "profit_factor"];

fn objective_of(m: &backtest2::Metrics, objective: &str) -> f64 {
    match objective {
        "sharpe" => m.sharpe.unwrap_or(f64::NEG_INFINITY),
        "profit_factor" => m.profit_factor.unwrap_or(if m.trades > 0 { 10.0 } else { f64::NEG_INFINITY }),
        _ => m.return_pct,
    }
}

/// Up to 3 variants for each numeric `period` of the spec's indicators (x0.75, x1, x1.5), capped overall.
fn variants(spec: &Value, cap: usize) -> Vec<(Value, Vec<String>)> {
    let mut out = vec![(spec.clone(), vec![])];
    let inds = spec["indicators"].as_array().cloned().unwrap_or_default();
    for (i, ind) in inds.iter().enumerate() {
        let Some(p) = ind["params"]["period"].as_f64() else { continue };
        let mut next = vec![];
        for (s, notes) in &out {
            for k in [0.75, 1.0, 1.5] {
                let np = ((p * k).round()).max(2.0);
                let mut v = s.clone();
                v["indicators"][i]["params"]["period"] = json!(np as i64);
                let mut n = notes.clone();
                if k != 1.0 {
                    n.push(format!("{} period {} -> {}", ind["id"].as_str().unwrap_or("?"), p, np));
                }
                next.push((v, n));
            }
        }
        out = next;
        if out.len() >= cap {
            break;
        }
    }
    out.truncate(cap);
    out
}

/// Run the pipeline. `progress` and `cancelled` come from the job runner.
pub fn run(hypothesis: &Value, frame: &Frame, cfg: &Config, data_warnings: usize, progress: &dyn Fn(f64), cancelled: &dyn Fn() -> bool) -> Result<Value, String> {
    let objective = hypothesis["objective"].as_str().unwrap_or("return");
    if !OBJECTIVES.contains(&objective) {
        return Err(format!("objective must be one of {}", OBJECTIVES.join(", ")));
    }
    let all = strategy::templates();
    let wanted: Vec<String> = hypothesis["templates"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let picked: Vec<&Value> = if wanted.is_empty() { all.iter().filter(|t| t["spec"]["kind"] == "signal").take(MAX_TEMPLATES).collect() } else { all.iter().filter(|t| wanted.iter().any(|w| t["id"] == w.as_str())).collect() };
    if picked.is_empty() {
        return Err("no known templates in the hypothesis".into());
    }
    if picked.len() > MAX_TEMPLATES {
        return Err(format!("at most {MAX_TEMPLATES} templates per research run"));
    }
    if frame.len() < 300 {
        return Err("need at least 300 bars for a research run".into());
    }
    let cut = frame.len() * 7 / 10;
    let is = backtest2::slice(frame, 0, cut);
    let per = (MAX_RUNS / picked.len()).max(1);
    let mut tried = vec![];
    let mut runs = 0;
    for t in &picked {
        for (spec, notes) in variants(&t["spec"], per) {
            if cancelled() {
                return Err("cancelled".into());
            }
            runs += 1;
            let r = strategy::parse(&spec).and_then(|s| backtest2::run(&s, &is, None, &Config { trade_from: 0, ..*cfg }));
            match r {
                Ok(r) => tried.push(json!({ "template": t["id"], "changes": notes, "spec": spec, "in_sample": { "objective": objective_of(&r.metrics, objective), "return_pct": r.metrics.return_pct, "trades": r.metrics.trades, "max_drawdown_pct": r.metrics.max_drawdown_pct } })),
                Err(e) => tried.push(json!({ "template": t["id"], "changes": notes, "error": e })),
            }
            progress(runs as f64 / (MAX_RUNS as f64) * 0.6);
        }
    }
    // rank by in-sample objective, only candidates with enough trades to mean anything
    let mut ranked: Vec<&Value> = tried.iter().filter(|x| x["in_sample"]["trades"].as_u64().unwrap_or(0) >= 10).collect();
    ranked.sort_by(|a, b| b["in_sample"]["objective"].as_f64().unwrap_or(f64::MIN).total_cmp(&a["in_sample"]["objective"].as_f64().unwrap_or(f64::MIN)).then_with(|| a["template"].as_str().cmp(&b["template"].as_str())));
    let mut oos = vec![];
    for c in ranked.iter().take(5) {
        let s = strategy::parse(&c["spec"])?;
        let r = backtest2::run(&s, frame, None, &Config { trade_from: cut, ..*cfg })?;
        oos.push(json!({ "template": c["template"], "changes": c["changes"], "spec": c["spec"], "in_sample": c["in_sample"], "out_of_sample": { "objective": objective_of(&r.metrics, objective), "return_pct": r.metrics.return_pct, "trades": r.metrics.trades, "max_drawdown_pct": r.metrics.max_drawdown_pct } }));
    }
    progress(0.75);
    // a candidate must hold up out of sample: positive OOS return with at least 3 OOS trades
    let best = oos.iter().filter(|c| c["out_of_sample"]["return_pct"].as_f64().unwrap_or(-1.0) > 0.0 && c["out_of_sample"]["trades"].as_u64().unwrap_or(0) >= 3).max_by(|a, b| a["out_of_sample"]["objective"].as_f64().unwrap_or(f64::MIN).total_cmp(&b["out_of_sample"]["objective"].as_f64().unwrap_or(f64::MIN)));
    let (candidate, robustness, conclusion) = match best {
        Some(b) => {
            let rb = research::robustness(&b["spec"], frame, cfg, data_warnings, &|_| {}, cancelled)?;
            (Some(b.clone()), Some(rb), "A candidate held up out of sample. It still needs your review; past results do not predict future ones.")
        }
        None => (None, None, "No candidate held up out of sample. That is a valid result: the hypothesis was not supported on this data."),
    };
    progress(1.0);
    Ok(json!({
        "pipeline_version": PIPELINE_VERSION,
        "hypothesis": hypothesis,
        "objective": objective,
        "bounds": { "max_templates": MAX_TEMPLATES, "max_runs": MAX_RUNS, "runs_used": runs, "in_sample_bars": cut, "out_of_sample_bars": frame.len() - cut },
        "tried": tried.len(),
        "failed": tried.iter().filter(|x| x.get("error").is_some()).count(),
        "shortlist": oos,
        "candidate": candidate,
        "robustness": robustness,
        "conclusion": conclusion,
        "limits": "Historical simulation on one symbol and interval. The researcher cannot place orders, change risk limits or read credentials; a person must review any candidate.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk(n: usize, seed: u64) -> Frame {
        let mut f = Frame::default();
        let mut s = seed;
        let mut x = 100.0;
        for i in 0..n {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let o = x;
            x *= 1.0 + ((s % 1000) as f64 / 1000.0 - 0.48) * 0.03;
            f.push(i as i64 * 3600, o, o.max(x) * 1.003, o.min(x) * 0.997, x, 1000.0);
        }
        f
    }

    fn cfg() -> Config {
        Config { capital: 10_000.0, fee_pct: 0.1, slippage_pct: 0.0, qty_step: [0.0001, 0.0001], trade_from: 0, max_volume_pct: None }
    }

    #[test]
    fn bounded_reproducible_and_validated() {
        let f = walk(1200, 3);
        let h = json!({ "templates": ["ema_cross", "ma_cross"], "objective": "return" });
        let a = run(&h, &f, &cfg(), 0, &|_| {}, &|| false).unwrap();
        let b = run(&h, &f, &cfg(), 0, &|_| {}, &|| false).unwrap();
        assert_eq!(a, b, "same inputs, same report");
        assert!(a["bounds"]["runs_used"].as_u64().unwrap() as usize <= MAX_RUNS);
        assert!(a["shortlist"].as_array().unwrap().len() <= 5);
        assert!(a["conclusion"].is_string());
        if let Some(c) = a["candidate"].as_object() {
            assert!(strategy::parse(&c["spec"]).is_ok());
            assert!(c["out_of_sample"]["return_pct"].as_f64().unwrap() > 0.0);
        }
        assert!(run(&json!({ "objective": "moonshot" }), &f, &cfg(), 0, &|_| {}, &|| false).is_err());
        assert!(run(&json!({ "templates": ["nope"] }), &f, &cfg(), 0, &|_| {}, &|| false).is_err());
        assert!(run(&h, &walk(100, 1), &cfg(), 0, &|_| {}, &|| false).is_err());
        assert_eq!(run(&h, &f, &cfg(), 0, &|_| {}, &|| true).unwrap_err(), "cancelled");
    }
}
