//! The Rust engine must give the same answers as the Python reference (zones.py, backtest.py) on the
//! same bars. The fixture was written by tests/make_fixture.py from the Python implementation.

use serde::Deserialize;
use serde_json::Value;
use zoneboard::{backtest, zones};

#[derive(Deserialize)]
struct Fixture {
    rr: f64,
    cost_pct: f64,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    t: Vec<i64>,
    o: Vec<f64>,
    h: Vec<f64>,
    l: Vec<f64>,
    c: Vec<f64>,
    v: Vec<f64>,
    zones: Vec<Value>,
    levels: Vec<Value>,
    summary: Value,
    trades: Vec<(usize, String, f64, f64)>,
}

fn frame(c: &Case) -> zones::Frame {
    zones::Frame { t: c.t.clone(), o: c.o.clone(), h: c.h.clone(), l: c.l.clone(), c: c.c.clone(), v: c.v.clone() }
}

/// Structural equality; numbers within `tol` (0 = exact), integers always exact.
fn same(a: &Value, b: &Value, tol: f64) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if x.is_i64() && y.is_i64() {
                x == y
            } else {
                (x.as_f64().unwrap() - y.as_f64().unwrap()).abs() <= tol
            }
        }
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q, tol)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w, tol))),
        _ => a == b,
    }
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/parity.json")).expect("fixture parses")
}

#[test]
fn zones_levels_and_summary_match_python() {
    let (mut n_zones, mut n_levels) = (0, 0);
    for case in fixture().cases {
        let f = frame(&case);
        let an = zones::analyze(&f);
        let (z, l) = zones::chart_overlays(&f, &an.obs, &an.liq);
        let (z, l) = (serde_json::to_value(&z).unwrap(), serde_json::to_value(&l).unwrap());
        assert!(same(&z, &Value::Array(case.zones.clone()), 0.0), "{}: zones differ\nrust:   {z}\npython: {:?}", case.name, case.zones);
        assert!(same(&l, &Value::Array(case.levels.clone()), 0.0), "{}: levels differ\nrust:   {l}\npython: {:?}", case.name, case.levels);

        let s = serde_json::to_value(zones::summarize(&f, &an.obs, &an.liq, &an.atr)).unwrap();
        assert!(same(&s, &case.summary, 0.011), "{}: summary differs\nrust:   {s}\npython: {}", case.name, case.summary);
        n_zones += case.zones.len();
        n_levels += case.levels.len();
    }
    // the comparison must not pass vacuously
    assert!(n_zones > 10 && n_levels > 30, "{n_zones} zones, {n_levels} levels");
}

#[test]
fn backtest_trades_match_python() {
    let fx = fixture();
    let mut total = 0;
    for case in &fx.cases {
        let got = backtest::backtest_symbol(&frame(case), fx.rr, fx.cost_pct);
        assert_eq!(got.len(), case.trades.len(), "{}: trade count", case.name);
        for (g, (idx, side, net, gross)) in got.iter().zip(&case.trades) {
            assert_eq!((g.idx, if g.long { "long" } else { "short" }), (*idx, side.as_str()), "{}: entry or side", case.name);
            assert!((g.net_r - net).abs() < 1e-9 && (g.gross_r - gross).abs() < 1e-9, "{}: R at bar {idx}: rust {}/{} python {net}/{gross}", case.name, g.net_r, g.gross_r);
        }
        total += got.len();
    }
    assert!(total > 50, "only {total} trades compared");
}
