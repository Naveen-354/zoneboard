//! Declarative strategy specs (roadmap decision D6): JSON built by the Bot Studio, validated here, and run by
//! [`crate::botengine`] in backtests and on the demo account. No user code is ever executed.
//!
//! ```json
//! { "kind": "signal", "direction": "long",
//!   "indicators": [{ "id": "fast", "type": "ema", "params": { "period": 9 } }],
//!   "entry": { "long": { "left": { "ind": "fast" }, "op": "crosses_above", "right": { "ind": "slow" } } },
//!   "exit":  { "long": { "left": { "ind": "fast" }, "op": "crosses_below", "right": { "ind": "slow" } } },
//!   "stop_loss": { "type": "pct", "value": 2 }, "take_profit": { "type": "atr", "value": 3, "period": 14 },
//!   "trailing_stop": null, "max_hold_bars": 0, "sizing": { "type": "pct_capital", "value": 50 } }
//! ```
//!
//! Operands may look back (`"shift": 3` = three bars ago) but never forward: there is no way to write a
//! condition that reads a future bar.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::indicators::{self, Series};
use crate::zones::Frame;

const MAX_INDICATORS: usize = 12;
const MAX_CONDITIONS: usize = 32;
const MAX_DEPTH: usize = 5;
const MAX_SHIFT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Signal,
    Grid,
    Dca,
    Pairs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Long,
    Short,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Long,
    Short,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Long => "long",
            Side::Short => "short",
        }
    }
    pub fn sign(self) -> f64 {
        match self {
            Side::Long => 1.0,
            Side::Short => -1.0,
        }
    }
}

/// A distance from the entry price: a percent of it, or a multiple of ATR at the signal bar.
#[derive(Clone, Debug, PartialEq)]
pub enum Level {
    Pct(f64),
    Atr { mult: f64, period: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sizing {
    /// Percent of the bot's current capital (allocation plus realised P&L).
    PctCapital(f64),
    FixedQty(f64),
    FixedNotional(f64),
    /// Risk this percent of capital between entry and stop. Needs a stop-loss.
    RiskPct(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Gt,
    Lt,
    Gte,
    Lte,
    CrossesAbove,
    CrossesBelow,
    Rising,
    Falling,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    Series { key: String, shift: usize },
    Value(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    All(Vec<Cond>),
    Any(Vec<Cond>),
    Not(Box<Cond>),
    Cmp { left: Operand, op: Op, right: Operand },
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndDef {
    pub id: String,
    pub kind: String,
    pub params: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    /// When no absolute bounds are given, the grid spans +/- this percent around the first price it sees.
    pub range_pct: f64,
    pub levels: usize,
    /// Close everything and halt when price closes this percent below the lowest line.
    pub stop_below_pct: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dca {
    pub base_pct: f64,
    pub safety_pct: f64,
    pub safety_orders: usize,
    pub step_pct: f64,
    pub step_scale: f64,
    pub volume_scale: f64,
    pub take_profit_pct: f64,
    pub stop_loss_pct: Option<f64>,
    pub restart: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pairs {
    pub lookback: usize,
    pub entry_z: f64,
    pub exit_z: f64,
    pub stop_z: Option<f64>,
    /// `false`: spread = ln(A) - ln(B) (pairs trading). `true`: rolling OLS hedge ratio (statistical arbitrage).
    pub ols: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Strategy {
    pub kind: Kind,
    pub direction: Direction,
    pub indicators: Vec<IndDef>,
    pub entry_long: Option<Cond>,
    pub entry_short: Option<Cond>,
    pub exit_long: Option<Cond>,
    pub exit_short: Option<Cond>,
    pub stop_loss: Option<Level>,
    pub take_profit: Option<Level>,
    pub trailing_stop: Option<Level>,
    pub max_hold_bars: usize,
    pub sizing: Sizing,
    pub grid: Option<Grid>,
    pub dca: Option<Dca>,
    pub pairs: Option<Pairs>,
}

// ------------------------------------------------------------------ parsing

fn num(v: Option<&Value>, what: &str, lo: f64, hi: f64) -> Result<f64, String> {
    let x = v.and_then(Value::as_f64).ok_or_else(|| format!("{what} must be a number"))?;
    if !x.is_finite() || x < lo || x > hi {
        return Err(format!("{what} must be between {lo} and {hi}"));
    }
    Ok(x)
}

fn opt_num(v: Option<&Value>, what: &str, lo: f64, hi: f64) -> Result<Option<f64>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        some => num(some, what, lo, hi).map(Some),
    }
}

fn whole(v: Option<&Value>, what: &str, lo: usize, hi: usize) -> Result<usize, String> {
    let x = num(v, what, lo as f64, hi as f64)?;
    if x.fract() != 0.0 {
        return Err(format!("{what} must be a whole number"));
    }
    Ok(x as usize)
}

fn obj<'a>(v: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    v.as_object().ok_or_else(|| format!("{what} must be an object"))
}

fn known(m: &Map<String, Value>, what: &str, keys: &[&str]) -> Result<(), String> {
    match m.keys().find(|k| !keys.contains(&k.as_str())) {
        Some(k) => Err(format!("{what}: unknown field '{k}'")),
        None => Ok(()),
    }
}

fn level(v: Option<&Value>, what: &str) -> Result<Option<Level>, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(None) };
    let m = obj(v, what)?;
    known(m, what, &["type", "value", "period"])?;
    match m.get("type").and_then(Value::as_str) {
        Some("pct") => Ok(Some(Level::Pct(num(m.get("value"), &format!("{what}.value"), 0.01, 99.0)?))),
        Some("atr") => Ok(Some(Level::Atr {
            mult: num(m.get("value"), &format!("{what}.value"), 0.1, 50.0)?,
            period: match m.get("period") {
                None | Some(Value::Null) => 14,
                p => whole(p, &format!("{what}.period"), 1, 200)?,
            },
        })),
        _ => Err(format!("{what}.type must be pct or atr")),
    }
}

const PRICE_FIELDS: [&str; 7] = ["open", "high", "low", "close", "volume", "hl2", "hlc3"];

struct CondParser<'a> {
    outputs: &'a HashMap<String, &'static [&'static str]>,
    count: usize,
}

impl CondParser<'_> {
    fn operand(&self, v: &Value, what: &str) -> Result<Operand, String> {
        if let Some(x) = v.as_f64() {
            return Ok(Operand::Value(x));
        }
        let m = obj(v, what)?;
        known(m, what, &["ind", "out", "price", "value", "shift"])?;
        let shift = match m.get("shift") {
            None | Some(Value::Null) => 0,
            s => whole(s, &format!("{what}.shift (bars ago, never negative)"), 0, MAX_SHIFT)?,
        };
        if let Some(x) = m.get("value") {
            return Ok(Operand::Value(num(Some(x), &format!("{what}.value"), -1e15, 1e15)?));
        }
        if let Some(p) = m.get("price") {
            let p = p.as_str().filter(|p| PRICE_FIELDS.contains(p)).ok_or_else(|| format!("{what}.price must be one of {}", PRICE_FIELDS.join(", ")))?;
            return Ok(Operand::Series { key: format!("price.{p}"), shift });
        }
        let id = m.get("ind").and_then(Value::as_str).ok_or_else(|| format!("{what} needs ind, price or value"))?;
        let outs = self.outputs.get(id).ok_or_else(|| format!("{what}: no indicator with id '{id}'"))?;
        let out = match m.get("out").and_then(Value::as_str) {
            Some(o) => o,
            None => outs[0],
        };
        if !outs.contains(&out) {
            return Err(format!("{what}: indicator '{id}' has outputs {}, not '{out}'", outs.join(", ")));
        }
        Ok(Operand::Series { key: format!("{id}.{out}"), shift })
    }

    fn cond(&mut self, v: &Value, what: &str, depth: usize) -> Result<Cond, String> {
        if depth > MAX_DEPTH {
            return Err(format!("{what}: conditions nest deeper than {MAX_DEPTH} levels"));
        }
        let m = obj(v, what)?;
        let list = |s: &mut Self, items: &Value, name: &str| -> Result<Vec<Cond>, String> {
            let arr = items.as_array().ok_or_else(|| format!("{what}.{name} must be a list"))?;
            if arr.is_empty() {
                return Err(format!("{what}.{name} must not be empty"));
            }
            arr.iter().enumerate().map(|(i, c)| s.cond(c, &format!("{what}.{name}[{i}]"), depth + 1)).collect()
        };
        if let Some(items) = m.get("all") {
            known(m, what, &["all"])?;
            return Ok(Cond::All(list(self, items, "all")?));
        }
        if let Some(items) = m.get("any") {
            known(m, what, &["any"])?;
            return Ok(Cond::Any(list(self, items, "any")?));
        }
        if let Some(c) = m.get("not") {
            known(m, what, &["not"])?;
            return Ok(Cond::Not(Box::new(self.cond(c, &format!("{what}.not"), depth + 1)?)));
        }
        known(m, what, &["left", "op", "right"])?;
        self.count += 1;
        if self.count > MAX_CONDITIONS {
            return Err(format!("at most {MAX_CONDITIONS} conditions per strategy"));
        }
        let op = match m.get("op").and_then(Value::as_str) {
            Some("gt" | ">") => Op::Gt,
            Some("lt" | "<") => Op::Lt,
            Some("gte" | ">=") => Op::Gte,
            Some("lte" | "<=") => Op::Lte,
            Some("crosses_above") => Op::CrossesAbove,
            Some("crosses_below") => Op::CrossesBelow,
            Some("rising") => Op::Rising,
            Some("falling") => Op::Falling,
            _ => return Err(format!("{what}.op must be gt, lt, gte, lte, crosses_above, crosses_below, rising or falling")),
        };
        let left = self.operand(m.get("left").ok_or_else(|| format!("{what}.left is missing"))?, &format!("{what}.left"))?;
        let right = match (op, m.get("right")) {
            (Op::Rising | Op::Falling, None | Some(Value::Null)) => Operand::Value(1.0),
            (_, Some(r)) => self.operand(r, &format!("{what}.right"))?,
            (_, None) => return Err(format!("{what}.right is missing")),
        };
        if matches!(op, Op::Rising | Op::Falling) {
            match right {
                Operand::Value(n) if n >= 1.0 && n.fract() == 0.0 && n <= MAX_SHIFT as f64 => {}
                _ => return Err(format!("{what}: rising/falling takes a whole number of bars as right")),
            }
        }
        if matches!(left, Operand::Value(_)) && !matches!(op, Op::Rising | Op::Falling) && matches!(right, Operand::Value(_)) {
            return Err(format!("{what} compares two constants"));
        }
        Ok(Cond::Cmp { left, op, right })
    }
}

pub fn parse(spec: &Value) -> Result<Strategy, String> {
    let m = obj(spec, "strategy")?;
    known(m, "strategy", &["kind", "direction", "indicators", "entry", "exit", "stop_loss", "take_profit", "trailing_stop", "max_hold_bars", "sizing", "grid", "dca", "pairs", "name", "notes"])?;
    let kind: Kind = serde_json::from_value(m.get("kind").cloned().unwrap_or(json!("signal"))).map_err(|_| "kind must be signal, grid, dca or pairs".to_string())?;
    let direction: Direction = serde_json::from_value(m.get("direction").cloned().unwrap_or(json!("long"))).map_err(|_| "direction must be long, short or both".to_string())?;

    let mut indicators = Vec::new();
    let mut outputs: HashMap<String, &'static [&'static str]> = HashMap::new();
    if let Some(list) = m.get("indicators").filter(|v| !v.is_null()) {
        let arr = list.as_array().ok_or("indicators must be a list")?;
        if arr.len() > MAX_INDICATORS {
            return Err(format!("at most {MAX_INDICATORS} indicators"));
        }
        for (i, d) in arr.iter().enumerate() {
            let what = format!("indicators[{i}]");
            let dm = obj(d, &what)?;
            known(dm, &what, &["id", "type", "params"])?;
            let id = dm.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if id.is_empty() || id.len() > 24 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || id == "price" {
                return Err(format!("{what}.id must be 1 to 24 letters, digits or _ (and not 'price')"));
            }
            if outputs.contains_key(&id) {
                return Err(format!("{what}: duplicate id '{id}'"));
            }
            let kind = dm.get("type").and_then(Value::as_str).unwrap_or("");
            let s = indicators::spec(kind).ok_or_else(|| format!("{what}.type: unknown indicator '{kind}'"))?;
            let params = indicators::resolve_params(s, dm.get("params").and_then(Value::as_object))?;
            outputs.insert(id.clone(), s.outputs);
            indicators.push(IndDef { id, kind: kind.to_string(), params });
        }
    }

    let mut cp = CondParser { outputs: &outputs, count: 0 };
    let side_conds = |cp: &mut CondParser, key: &str| -> Result<(Option<Cond>, Option<Cond>), String> {
        let Some(v) = m.get(key).filter(|v| !v.is_null()) else { return Ok((None, None)) };
        let sm = obj(v, key)?;
        known(sm, key, &["long", "short"])?;
        let one = |cp: &mut CondParser, s: &str| -> Result<Option<Cond>, String> {
            match sm.get(s) {
                None | Some(Value::Null) => Ok(None),
                Some(c) => cp.cond(c, &format!("{key}.{s}"), 0).map(Some),
            }
        };
        Ok((one(cp, "long")?, one(cp, "short")?))
    };
    let (entry_long, entry_short) = side_conds(&mut cp, "entry")?;
    let (exit_long, exit_short) = side_conds(&mut cp, "exit")?;

    let stop_loss = level(m.get("stop_loss"), "stop_loss")?;
    let take_profit = level(m.get("take_profit"), "take_profit")?;
    let trailing_stop = level(m.get("trailing_stop"), "trailing_stop")?;
    let max_hold_bars = match m.get("max_hold_bars") {
        None | Some(Value::Null) => 0,
        v => whole(v, "max_hold_bars", 0, 10_000)?,
    };

    let sizing = match m.get("sizing").filter(|v| !v.is_null()) {
        None => Sizing::PctCapital(100.0),
        Some(v) => {
            let sm = obj(v, "sizing")?;
            known(sm, "sizing", &["type", "value"])?;
            let val = sm.get("value");
            match sm.get("type").and_then(Value::as_str) {
                Some("pct_capital") => Sizing::PctCapital(num(val, "sizing.value", 0.1, 100.0)?),
                Some("fixed_qty") => Sizing::FixedQty(num(val, "sizing.value", 1e-8, 1e12)?),
                Some("fixed_notional") => Sizing::FixedNotional(num(val, "sizing.value", 0.01, 1e15)?),
                Some("risk_pct") => Sizing::RiskPct(num(val, "sizing.value", 0.01, 20.0)?),
                _ => return Err("sizing.type must be pct_capital, fixed_qty, fixed_notional or risk_pct".into()),
            }
        }
    };

    let grid = match m.get("grid").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => {
            let g = obj(v, "grid")?;
            known(g, "grid", &["lower", "upper", "range_pct", "levels", "stop_below_pct"])?;
            let lower = opt_num(g.get("lower"), "grid.lower", 1e-12, 1e15)?;
            let upper = opt_num(g.get("upper"), "grid.upper", 1e-12, 1e15)?;
            if lower.is_some() != upper.is_some() {
                return Err("grid: give both lower and upper, or neither (then range_pct is used)".into());
            }
            if let (Some(l), Some(u)) = (lower, upper) {
                if l >= u {
                    return Err("grid.lower must be below grid.upper".into());
                }
            }
            Some(Grid {
                lower,
                upper,
                range_pct: opt_num(g.get("range_pct"), "grid.range_pct", 0.5, 90.0)?.unwrap_or(10.0),
                levels: match g.get("levels") {
                    None | Some(Value::Null) => 10,
                    l => whole(l, "grid.levels", 2, 100)?,
                },
                stop_below_pct: opt_num(g.get("stop_below_pct"), "grid.stop_below_pct", 0.1, 90.0)?,
            })
        }
    };

    let dca = match m.get("dca").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => {
            let d = obj(v, "dca")?;
            known(d, "dca", &["base_pct", "safety_pct", "safety_orders", "step_pct", "step_scale", "volume_scale", "take_profit_pct", "stop_loss_pct", "restart"])?;
            let g = |k: &str, def: f64, lo: f64, hi: f64| opt_num(d.get(k), &format!("dca.{k}"), lo, hi).map(|x| x.unwrap_or(def));
            let dca = Dca {
                base_pct: g("base_pct", 10.0, 0.1, 100.0)?,
                safety_pct: g("safety_pct", 10.0, 0.0, 100.0)?,
                safety_orders: match d.get("safety_orders") {
                    None | Some(Value::Null) => 5,
                    s => whole(s, "dca.safety_orders", 0, 25)?,
                },
                step_pct: g("step_pct", 2.0, 0.1, 50.0)?,
                step_scale: g("step_scale", 1.0, 0.5, 3.0)?,
                volume_scale: g("volume_scale", 1.5, 0.5, 3.0)?,
                take_profit_pct: g("take_profit_pct", 2.0, 0.1, 100.0)?,
                stop_loss_pct: opt_num(d.get("stop_loss_pct"), "dca.stop_loss_pct", 0.1, 95.0)?,
                restart: d.get("restart").and_then(Value::as_bool).unwrap_or(true),
            };
            let total = dca.base_pct + (0..dca.safety_orders).map(|k| dca.safety_pct * dca.volume_scale.powi(k as i32)).sum::<f64>();
            if total > 100.0 + 1e-9 {
                return Err(format!("dca: base plus all safety orders need {total:.1}% of the allocation; keep it at 100% or less"));
            }
            let deepest: f64 = (0..dca.safety_orders).map(|k| dca.step_pct * dca.step_scale.powi(k as i32)).sum();
            if deepest >= 95.0 {
                return Err(format!("dca: the last safety order would sit {deepest:.1}% below the base price; reduce the steps"));
            }
            Some(dca)
        }
    };

    let pairs = match m.get("pairs").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => {
            let p = obj(v, "pairs")?;
            known(p, "pairs", &["lookback", "entry_z", "exit_z", "stop_z", "hedge"])?;
            let entry_z = opt_num(p.get("entry_z"), "pairs.entry_z", 0.5, 10.0)?.unwrap_or(2.0);
            let exit_z = opt_num(p.get("exit_z"), "pairs.exit_z", 0.0, 10.0)?.unwrap_or(0.5);
            let stop_z = opt_num(p.get("stop_z"), "pairs.stop_z", 0.5, 20.0)?;
            if exit_z >= entry_z {
                return Err("pairs.exit_z must be below pairs.entry_z".into());
            }
            if stop_z.is_some_and(|s| s <= entry_z) {
                return Err("pairs.stop_z must be above pairs.entry_z".into());
            }
            Some(Pairs {
                lookback: match p.get("lookback") {
                    None | Some(Value::Null) => 60,
                    l => whole(l, "pairs.lookback", 10, 1000)?,
                },
                entry_z,
                exit_z,
                stop_z,
                ols: match p.get("hedge").and_then(Value::as_str) {
                    None | Some("ratio") => false,
                    Some("ols") => true,
                    _ => return Err("pairs.hedge must be ratio or ols".into()),
                },
            })
        }
    };

    let s = Strategy { kind, direction, indicators, entry_long, entry_short, exit_long, exit_short, stop_loss, take_profit, trailing_stop, max_hold_bars, sizing, grid, dca, pairs };
    check_kind(&s)?;
    Ok(s)
}

fn check_kind(s: &Strategy) -> Result<(), String> {
    if matches!(s.sizing, Sizing::RiskPct(_)) && s.stop_loss.is_none() {
        return Err("risk_pct sizing needs a stop_loss: the risk is the distance to the stop".into());
    }
    match s.kind {
        Kind::Signal => {
            let long = matches!(s.direction, Direction::Long | Direction::Both);
            let short = matches!(s.direction, Direction::Short | Direction::Both);
            if long && s.entry_long.is_none() {
                return Err("a long signal strategy needs entry.long".into());
            }
            if short && s.entry_short.is_none() {
                return Err("a short signal strategy needs entry.short".into());
            }
            if s.grid.is_some() || s.dca.is_some() || s.pairs.is_some() {
                return Err("a signal strategy cannot have grid, dca or pairs settings".into());
            }
        }
        Kind::Grid => {
            if s.grid.is_none() {
                return Err("a grid strategy needs grid settings".into());
            }
            if s.direction != Direction::Long {
                return Err("grid bots are long-only here (spot-style: buy low lines, sell the next line up)".into());
            }
        }
        Kind::Dca => {
            if s.dca.is_none() {
                return Err("a dca strategy needs dca settings".into());
            }
            if s.direction != Direction::Long {
                return Err("dca bots are long-only".into());
            }
        }
        Kind::Pairs => {
            if s.pairs.is_none() {
                return Err("a pairs strategy needs pairs settings".into());
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ evaluation

/// Every series a strategy can read, computed once on **closed** bars.
pub struct Ctx {
    pub series: HashMap<String, Series>,
    pub atr: HashMap<usize, Series>,
}

impl Ctx {
    pub fn build(s: &Strategy, f: &Frame) -> Result<Self, String> {
        let mut series = HashMap::new();
        for (name, v) in [("open", &f.o), ("high", &f.h), ("low", &f.l), ("close", &f.c), ("volume", &f.v)] {
            series.insert(format!("price.{name}"), v.clone());
        }
        series.insert("price.hl2".into(), (0..f.len()).map(|i| (f.h[i] + f.l[i]) / 2.0).collect());
        series.insert("price.hlc3".into(), (0..f.len()).map(|i| (f.h[i] + f.l[i] + f.c[i]) / 3.0).collect());
        for d in &s.indicators {
            for (out, v) in indicators::compute(&d.kind, &d.params, f)? {
                series.insert(format!("{}.{out}", d.id), v);
            }
        }
        let mut atr = HashMap::new();
        for l in [&s.stop_loss, &s.take_profit, &s.trailing_stop].into_iter().flatten() {
            if let Level::Atr { period, .. } = l {
                atr.entry(*period).or_insert_with(|| indicators::atr(f, *period));
            }
        }
        Ok(Self { series, atr })
    }

    fn at(&self, o: &Operand, i: usize, back: usize) -> f64 {
        match o {
            Operand::Value(x) => *x,
            Operand::Series { key, shift } => {
                let k = shift + back;
                if k > i {
                    return f64::NAN;
                }
                self.series.get(key).and_then(|s| s.get(i - k)).copied().unwrap_or(f64::NAN)
            }
        }
    }

    /// Is `c` true on bar `i`? Reads bars `<= i` only. Anything involving a `NaN` (warm-up) is false.
    pub fn eval(&self, c: &Cond, i: usize) -> bool {
        match c {
            Cond::All(v) => v.iter().all(|c| self.eval(c, i)),
            Cond::Any(v) => v.iter().any(|c| self.eval(c, i)),
            Cond::Not(c) => !self.eval(c, i),
            Cond::Cmp { left, op, right } => {
                let (a, b) = (self.at(left, i, 0), self.at(right, i, 0));
                match op {
                    Op::Rising | Op::Falling => {
                        let n = b as usize;
                        let prev = self.at(left, i, n);
                        if !a.is_finite() || !prev.is_finite() {
                            return false;
                        }
                        if *op == Op::Rising { a > prev } else { a < prev }
                    }
                    _ if !a.is_finite() || !b.is_finite() => false,
                    Op::Gt => a > b,
                    Op::Lt => a < b,
                    Op::Gte => a >= b,
                    Op::Lte => a <= b,
                    Op::CrossesAbove | Op::CrossesBelow => {
                        let (pa, pb) = (self.at(left, i, 1), self.at(right, i, 1));
                        if !pa.is_finite() || !pb.is_finite() {
                            return false;
                        }
                        if *op == Op::CrossesAbove { pa <= pb && a > b } else { pa >= pb && a < b }
                    }
                }
            }
        }
    }

    /// The distance a level puts between the entry and a stop or target, measured at signal bar `i`.
    pub fn distance(&self, l: &Level, entry: f64, i: usize) -> Option<f64> {
        let d = match l {
            Level::Pct(p) => entry * p / 100.0,
            Level::Atr { mult, period } => mult * self.atr.get(period)?.get(i).copied()?,
        };
        (d.is_finite() && d > 0.0).then_some(d)
    }
}

// ------------------------------------------------------------------ the strategy library

fn ind(id: &str, kind: &str, params: Value) -> Value {
    json!({ "id": id, "type": kind, "params": params })
}
fn s(id: &str) -> Value {
    json!({ "ind": id })
}
fn so(id: &str, out: &str) -> Value {
    json!({ "ind": id, "out": out })
}
fn px(f: &str) -> Value {
    json!({ "price": f })
}
fn cmp(l: Value, op: &str, r: Value) -> Value {
    json!({ "left": l, "op": op, "right": r })
}
fn v(x: f64) -> Value {
    json!({ "value": x })
}

/// Ready-made strategies for the library and the builder's templates. Each one validates (tested).
pub fn templates() -> Vec<Value> {
    let pct = |x: f64| json!({ "type": "pct", "value": x });
    let atrl = |x: f64| json!({ "type": "atr", "value": x, "period": 14 });
    let t = |id: &str, name: &str, group: &str, about: &str, spec: Value| json!({ "id": id, "name": name, "group": group, "about": about, "spec": spec });
    vec![
        t("ma_cross", "Moving Average Crossover", "trend", "Buy when the 20 SMA crosses above the 50 SMA, sell on the cross back.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("fast", "sma", json!({"period": 20})), ind("slow", "sma", json!({"period": 50}))],
            "entry": { "long": cmp(s("fast"), "crosses_above", s("slow")) },
            "exit": { "long": cmp(s("fast"), "crosses_below", s("slow")) },
            "stop_loss": pct(5.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("ema_cross", "EMA Crossover", "trend", "9 EMA over 21 EMA, long and short, with a 2 ATR stop.", json!({
            "kind": "signal", "direction": "both",
            "indicators": [ind("fast", "ema", json!({"period": 9})), ind("slow", "ema", json!({"period": 21}))],
            "entry": { "long": cmp(s("fast"), "crosses_above", s("slow")), "short": cmp(s("fast"), "crosses_below", s("slow")) },
            "exit": { "long": cmp(s("fast"), "crosses_below", s("slow")), "short": cmp(s("fast"), "crosses_above", s("slow")) },
            "stop_loss": atrl(2.0), "sizing": { "type": "pct_capital", "value": 50 } })),
        t("supertrend", "Supertrend", "trend", "Ride the Supertrend direction; a 3 ATR trailing stop protects gains.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("st", "supertrend", json!({"period": 10, "multiplier": 3}))],
            "entry": { "long": cmp(so("st", "direction"), "crosses_above", v(0.0)) },
            "exit": { "long": cmp(so("st", "direction"), "crosses_below", v(0.0)) },
            "trailing_stop": atrl(3.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("macd", "MACD", "trend", "MACD line crossing its signal line, only above the zero line.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("m", "macd", json!({"fast": 12, "slow": 26, "signal": 9}))],
            "entry": { "long": { "all": [cmp(so("m", "macd"), "crosses_above", so("m", "signal")), cmp(so("m", "macd"), "gt", v(0.0))] } },
            "exit": { "long": cmp(so("m", "macd"), "crosses_below", so("m", "signal")) },
            "stop_loss": pct(4.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("adx_trend", "ADX Trend", "trend", "Enter when ADX shows a strong trend and +DI leads -DI; exit when the lead flips.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("dmi", "adx", json!({"period": 14}))],
            "entry": { "long": { "all": [cmp(so("dmi", "adx"), "gt", v(25.0)), cmp(so("dmi", "plus_di"), "crosses_above", so("dmi", "minus_di"))] } },
            "exit": { "long": cmp(so("dmi", "plus_di"), "crosses_below", so("dmi", "minus_di")) },
            "stop_loss": atrl(2.5), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("rsi", "RSI Reversal", "momentum", "Buy when RSI(14) climbs back above 30, sell above 70.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("r", "rsi", json!({"period": 14}))],
            "entry": { "long": cmp(s("r"), "crosses_above", v(30.0)) },
            "exit": { "long": cmp(s("r"), "gt", v(70.0)) },
            "stop_loss": pct(5.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("stoch_rsi", "Stochastic RSI", "momentum", "%K crosses above %D below 20; exit when %K crosses below %D above 80.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("srsi", "stoch_rsi", json!({}))],
            "entry": { "long": { "all": [cmp(so("srsi", "k"), "crosses_above", so("srsi", "d")), cmp(so("srsi", "k"), "lt", v(20.0))] } },
            "exit": { "long": { "all": [cmp(so("srsi", "k"), "crosses_below", so("srsi", "d")), cmp(so("srsi", "k"), "gt", v(80.0))] } },
            "stop_loss": pct(4.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("cci", "CCI Momentum", "momentum", "Long when CCI(20) crosses above +100, out when it falls below 0.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("c", "cci", json!({"period": 20}))],
            "entry": { "long": cmp(s("c"), "crosses_above", v(100.0)) },
            "exit": { "long": cmp(s("c"), "crosses_below", v(0.0)) },
            "stop_loss": atrl(2.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("roc", "Rate of Change", "momentum", "Long when 10-bar ROC turns positive while price is above the 50 EMA.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("roc", "roc", json!({"period": 10})), ind("trend", "ema", json!({"period": 50}))],
            "entry": { "long": { "all": [cmp(s("roc"), "crosses_above", v(0.0)), cmp(px("close"), "gt", s("trend"))] } },
            "exit": { "long": cmp(s("roc"), "crosses_below", v(0.0)) },
            "stop_loss": pct(3.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("bollinger", "Bollinger Reversion", "mean_reversion", "Buy a close below the lower band, sell at the middle band.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("bb", "bollinger", json!({"period": 20, "stddev": 2}))],
            "entry": { "long": cmp(px("close"), "lt", so("bb", "lower")) },
            "exit": { "long": cmp(px("close"), "gt", so("bb", "middle")) },
            "stop_loss": pct(5.0), "max_hold_bars": 30, "sizing": { "type": "pct_capital", "value": 100 } })),
        t("vwap_reversion", "VWAP Reversion", "mean_reversion", "Buy when price is 2% below its 20-bar VWAP, sell back at VWAP.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("vw", "vwap", json!({"period": 20}))],
            "entry": { "long": cmp(so("vw", "deviation_pct"), "lt", v(-2.0)) },
            "exit": { "long": cmp(so("vw", "deviation_pct"), "gte", v(0.0)) },
            "stop_loss": pct(4.0), "max_hold_bars": 20, "sizing": { "type": "pct_capital", "value": 100 } })),
        t("zscore", "Z-Score Reversion", "mean_reversion", "Fade a 2-sigma stretch from the 20-bar mean, both ways.", json!({
            "kind": "signal", "direction": "both",
            "indicators": [ind("z", "zscore", json!({"period": 20}))],
            "entry": { "long": cmp(s("z"), "lt", v(-2.0)), "short": cmp(s("z"), "gt", v(2.0)) },
            "exit": { "long": cmp(s("z"), "gte", v(0.0)), "short": cmp(s("z"), "lte", v(0.0)) },
            "stop_loss": pct(4.0), "sizing": { "type": "pct_capital", "value": 50 } })),
        t("obv", "OBV Trend", "volume", "Long while OBV is above its 20 EMA and rising; exit when it drops below.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("o", "obv", json!({})), ind("oe", "obv_ema", json!({"period": 20}))],
            "entry": { "long": { "all": [cmp(s("o"), "crosses_above", s("oe")), cmp(s("o"), "rising", v(3.0))] } },
            "exit": { "long": cmp(s("o"), "crosses_below", s("oe")) },
            "stop_loss": atrl(2.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("volume_breakout", "Volume Breakout", "volume", "Close above the prior 20-bar high on at least 2x average volume.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("dc", "donchian", json!({"period": 20})), ind("vr", "volume_ratio", json!({"period": 20}))],
            "entry": { "long": { "all": [cmp(px("close"), "gt", so("dc", "upper")), cmp(s("vr"), "gte", v(2.0))] } },
            "exit": { "long": cmp(px("close"), "lt", so("dc", "middle")) },
            "stop_loss": atrl(2.0), "take_profit": atrl(4.0), "sizing": { "type": "risk_pct", "value": 1 } })),
        t("vw_momentum", "Volume-Weighted Momentum", "volume", "Long when volume-weighted momentum turns up through zero.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("vwm", "vw_momentum", json!({"period": 14}))],
            "entry": { "long": cmp(s("vwm"), "crosses_above", v(0.0)) },
            "exit": { "long": cmp(s("vwm"), "crosses_below", v(0.0)) },
            "stop_loss": pct(3.0), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("ichimoku", "Ichimoku Cloud", "advanced", "Long above the cloud on a Tenkan/Kijun cross; exit below the Kijun.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("ich", "ichimoku", json!({}))],
            "entry": { "long": { "all": [cmp(so("ich", "tenkan"), "crosses_above", so("ich", "kijun")), cmp(px("close"), "gt", so("ich", "span_a")), cmp(px("close"), "gt", so("ich", "span_b"))] } },
            "exit": { "long": cmp(px("close"), "lt", so("ich", "kijun")) },
            "stop_loss": atrl(2.5), "sizing": { "type": "pct_capital", "value": 100 } })),
        t("donchian", "Donchian Breakout", "advanced", "Turtle-style: buy a close above the prior 20-bar high, exit below the prior 10-bar low.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("entry", "donchian", json!({"period": 20})), ind("exit", "donchian", json!({"period": 10}))],
            "entry": { "long": cmp(px("close"), "gt", so("entry", "upper")) },
            "exit": { "long": cmp(px("close"), "lt", so("exit", "lower")) },
            "stop_loss": atrl(2.0), "sizing": { "type": "risk_pct", "value": 1 } })),
        t("atr_system", "ATR Channel System", "advanced", "Trend filter by 50 EMA; entries on a 1.5 ATR push, 2 ATR stop, 3 ATR trailing stop.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("trend", "ema", json!({"period": 50})), ind("kc", "ema", json!({"period": 20})), ind("a", "atr", json!({"period": 14}))],
            "entry": { "long": { "all": [cmp(px("close"), "gt", s("trend")), cmp(px("close"), "crosses_above", s("kc"))] } },
            "stop_loss": atrl(2.0), "trailing_stop": atrl(3.0), "sizing": { "type": "risk_pct", "value": 1 } })),
        t("breakout", "Range Breakout", "advanced", "Buy a breakout of the prior 55-bar high with a 2:1 reward to risk.", json!({
            "kind": "signal", "direction": "long",
            "indicators": [ind("dc", "donchian", json!({"period": 55}))],
            "entry": { "long": cmp(px("close"), "crosses_above", so("dc", "upper")) },
            "stop_loss": atrl(2.0), "take_profit": atrl(4.0), "max_hold_bars": 40, "sizing": { "type": "risk_pct", "value": 1 } })),
        t("grid", "Grid Trading", "advanced", "10 lines across +/-10% of the start price: buy each line on the way down, sell it one line up.", json!({
            "kind": "grid", "direction": "long", "grid": { "range_pct": 10, "levels": 10, "stop_below_pct": 5 } })),
        t("dca", "DCA (safety orders)", "advanced", "Base order, then up to 5 safety orders from 2% below (steps 1.2x wider, sizes 1.5x larger), take profit 2% over the average price.", json!({
            "kind": "dca", "direction": "long",
            "dca": { "base_pct": 10, "safety_pct": 6, "safety_orders": 5, "step_pct": 2, "step_scale": 1.2, "volume_scale": 1.5, "take_profit_pct": 2, "stop_loss_pct": 25, "restart": true } })),
        t("pairs", "Pairs Trading", "advanced", "Trade the log-price spread of two related symbols when it stretches 2 sigma; exit near the mean.", json!({
            "kind": "pairs", "direction": "both", "max_hold_bars": 100,
            "pairs": { "lookback": 60, "entry_z": 2, "exit_z": 0.5, "stop_z": 4, "hedge": "ratio" }, "sizing": { "type": "pct_capital", "value": 100 } })),
        t("stat_arb", "Statistical Arbitrage", "advanced", "Pairs with a rolling OLS hedge ratio: trade the regression residual's z-score.", json!({
            "kind": "pairs", "direction": "both", "max_hold_bars": 100,
            "pairs": { "lookback": 100, "entry_z": 2, "exit_z": 0.25, "stop_z": 4, "hedge": "ols" }, "sizing": { "type": "pct_capital", "value": 100 } })),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_validates() {
        let t = templates();
        assert!(t.len() >= 23);
        for x in &t {
            parse(&x["spec"]).unwrap_or_else(|e| panic!("{}: {e}", x["id"]));
        }
    }

    #[test]
    fn bad_specs_are_rejected_with_a_reason() {
        let bad = |v: Value, frag: &str| {
            let e = parse(&v).unwrap_err();
            assert!(e.contains(frag), "{v} -> {e}");
        };
        bad(json!({"kind": "signal"}), "entry.long");
        bad(json!({"kind": "nope"}), "kind");
        bad(json!({"entry": {"long": {"left": {"ind": "x"}, "op": "gt", "right": 1}}}), "no indicator");
        bad(json!({"indicators": [{"id": "a", "type": "sma"}], "entry": {"long": {"left": {"ind": "a", "shift": -1}, "op": "gt", "right": 1}}}), "never negative");
        bad(json!({"indicators": [{"id": "a", "type": "sma"}], "entry": {"long": {"left": {"ind": "a", "out": "upper"}, "op": "gt", "right": 1}}}), "outputs");
        bad(json!({"indicators": [{"id": "a", "type": "sma"}, {"id": "a", "type": "ema"}]}), "duplicate");
        bad(json!({"entry": {"long": {"left": 1, "op": "gt", "right": 2}}}), "two constants");
        bad(json!({"indicators": [{"id": "a", "type": "sma"}], "entry": {"long": {"left": {"ind": "a"}, "op": "gt", "right": 1}}, "sizing": {"type": "risk_pct", "value": 1}}), "needs a stop_loss");
        bad(json!({"kind": "dca", "dca": {"base_pct": 50, "safety_pct": 30, "safety_orders": 3}}), "100%");
        bad(json!({"kind": "grid", "grid": {"lower": 10, "upper": 5}}), "below");
        bad(json!({"kind": "pairs", "pairs": {"entry_z": 1, "exit_z": 2}}), "exit_z");
        bad(json!({"bogus": 1}), "unknown field");
    }

    #[test]
    fn conditions_evaluate_and_never_read_ahead() {
        let mut f = Frame::default();
        for (i, c) in [1.0, 2.0, 3.0, 2.0, 1.0, 2.0, 4.0].iter().enumerate() {
            f.push(i as i64, *c, *c, *c, *c, 1.0);
        }
        let st = parse(&json!({
            "indicators": [{"id": "m", "type": "sma", "params": {"period": 2}}],
            "entry": {"long": {"left": {"price": "close"}, "op": "crosses_above", "right": {"ind": "m"}}}
        }))
        .unwrap();
        let ctx = Ctx::build(&st, &f).unwrap();
        let c = st.entry_long.as_ref().unwrap();
        // sma2: _, 1.5, 2.5, 2.5, 1.5, 1.5, 3.0 ; close vs sma: crosses above at i=5 (1 <= 1.5 -> 2 > 1.5)
        let fired: Vec<usize> = (0..f.len()).filter(|&i| ctx.eval(c, i)).collect();
        assert_eq!(fired, vec![5]);
        let rising = Cond::Cmp { left: Operand::Series { key: "price.close".into(), shift: 0 }, op: Op::Rising, right: Operand::Value(2.0) };
        assert!(ctx.eval(&rising, 6) && !ctx.eval(&rising, 4) && !ctx.eval(&rising, 1));
        // shift past the start is NaN, which is false, never a panic
        let far = Cond::Cmp { left: Operand::Series { key: "price.close".into(), shift: 50 }, op: Op::Gt, right: Operand::Value(0.0) };
        assert!(!ctx.eval(&far, 6));
    }
}
