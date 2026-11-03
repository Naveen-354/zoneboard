//! Backtest the order-block rule on years of history before trusting it.
//!
//!   zoneboard-backtest                                       NSE daily, 5 years, 0.1% round-trip cost
//!   zoneboard-backtest --market crypto --interval 1h         Binance 1h candles (5000 bars each), 0.2% cost
//!   zoneboard-backtest --market crypto --interval 15m --bars 20000 --rr 2
//!   SAMPLE_DATA=1 zoneboard-backtest                          random-walk sample data: should show NO edge
//!
//! Results are in R (units of risk), before and after costs, split into an older and a newer half. If
//! the average R is not clearly above zero after costs in BOTH halves, the rule has no usable edge.
use std::collections::BTreeMap;

use futures_util::stream::{self, StreamExt};
use zoneboard::backtest::{self, Stats};
use zoneboard::data;
use zoneboard::zones::Frame;

struct Args {
    market: String,
    period: String,
    interval: String,
    bars: usize,
    rr: f64,
    cost: Option<f64>,
}

fn parse() -> Result<Args, String> {
    let mut a = Args { market: "india".into(), period: "5y".into(), interval: "1h".into(), bars: 5000, rr: 2.0, cost: None };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match flag.as_str() {
            "--market" => a.market = val("--market")?,
            "--period" => a.period = val("--period")?,
            "--interval" => a.interval = val("--interval")?,
            "--bars" => a.bars = val("--bars")?.parse().map_err(|_| "--bars must be a whole number")?,
            "--rr" => a.rr = val("--rr")?.parse().map_err(|_| "--rr must be a number")?,
            "--cost" => a.cost = Some(val("--cost")?.parse().map_err(|_| "--cost must be a number (percent)")?),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if !["india", "crypto"].contains(&a.market.as_str()) {
        return Err("--market must be india or crypto".into());
    }
    if !["15m", "1h", "4h", "1d"].contains(&a.interval.as_str()) {
        return Err("--interval must be one of 15m, 1h, 4h, 1d".into());
    }
    Ok(a)
}

fn show(s: &Option<Stats>) -> String {
    match s {
        None => "no trades".into(),
        Some(s) => format!("trades {:<5} win {:>5}%   avg R {:>7}   total R {:>7}   profit factor {}", s.trades, s.win_rate, s.avg_r, s.total_r, s.profit_factor),
    }
}

#[tokio::main]
async fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(m) => {
            if !m.is_empty() {
                eprintln!("error: {m}\n");
            }
            eprintln!("usage: zoneboard-backtest [--market india|crypto] [--period 5y] [--interval 15m|1h|4h|1d] [--bars 5000] [--rr 2] [--cost PERCENT]");
            std::process::exit(if m.is_empty() { 0 } else { 2 });
        }
    };
    let sample = zoneboard::server::sample_from_env();
    let client = data::http_client();

    let frames: BTreeMap<String, Frame> = if args.market == "crypto" {
        let interval = args.interval.as_str();
        println!("Loading {} Binance candles per symbol ({interval})...", args.bars);
        stream::iter(data::load_crypto_symbols())
            .map(|s| {
                let client = client.clone();
                async move { (s.clone(), if sample { Ok(data::synthetic(&s, "1d")) } else { data::binance_frame(&client, &s, interval, args.bars).await }) }
            })
            .buffer_unordered(4)
            .filter_map(|(s, r)| async move {
                match r {
                    Ok(f) if f.len() > 100 => Some((s, f)),
                    Ok(_) => None,
                    Err(e) => {
                        eprintln!("skipped {s}: {e}");
                        None
                    }
                }
            })
            .collect()
            .await
    } else {
        println!("Loading {} of daily NSE data...", args.period);
        stream::iter(data::load_symbols())
            .map(|s| {
                let (client, period) = (client.clone(), args.period.clone());
                async move { (s.clone(), if sample { Some(data::synthetic(&s, "1d")) } else { data::yahoo_frame(&client, &s, &period, "1d").await }) }
            })
            .buffer_unordered(8)
            .filter_map(|(s, f)| async move { f.filter(|f| f.len() > 60).map(|f| (s, f)) })
            .collect()
            .await
    };

    let cost = args.cost.unwrap_or(if args.market == "crypto" { 0.2 } else { 0.1 });
    println!("Loaded {} symbols{}; reward:risk {}, round-trip cost {}%\n", frames.len(), if sample { " (SAMPLE random data)" } else { "" }, args.rr, cost);
    let report = backtest::run(&frames, args.rr, cost / 100.0);
    if report.is_empty() {
        println!("No trades found.");
        return;
    }
    for (name, s) in &report {
        println!("{name:22} {}", show(s));
    }
    println!("\nHow to read this: avg R is the average result per trade in units of risk. If it is not clearly above zero AFTER costs, in BOTH halves, the rule has no usable edge.");
}
