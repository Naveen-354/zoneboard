//! Market-data quality: duplicate, out-of-order, missing and stale candles.
//!
//! Crypto trades around the clock, so any step larger than the interval is a gap. NSE trades in sessions
//! (09:15 to 15:30 IST, weekdays), so only a jump **inside one trading day** counts as a gap for intraday bars,
//! and only a jump of more than 4 calendar days (a long weekend) counts for daily bars. Holidays are not
//! modelled: a missing holiday bar is not reported, and a real one-day gap on a weekday is not caught.

use chrono::{FixedOffset, TimeZone};
use serde::Serialize;
use serde_json::{json, Value};

use crate::zones::Frame;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Quality {
    pub bars: usize,
    pub duplicates: usize,
    pub out_of_order: usize,
    /// (bar time before the gap, bar time after it, bars missing)
    pub gaps: Vec<(i64, i64, i64)>,
    pub missing_bars: i64,
    pub first: Option<i64>,
    pub last: Option<i64>,
    /// seconds since the last bar opened
    pub last_bar_age: Option<i64>,
    pub stale: bool,
    pub warnings: Vec<String>,
}

fn ist_day(t: i64) -> chrono::NaiveDate {
    FixedOffset::east_opt(5 * 3600 + 1800).unwrap().timestamp_opt(t, 0).single().map(|d| d.date_naive()).unwrap_or_default()
}

/// Check a frame. `now` is unix seconds; a frame is stale when its newest bar opened more than 3 intervals ago
/// (crypto) or, for NSE, more than 4 days ago (weekends and holidays are normal there).
pub fn check(f: &Frame, market: &str, interval_secs: i64, now: i64) -> Quality {
    let mut q = Quality { bars: f.len(), duplicates: 0, out_of_order: 0, gaps: vec![], missing_bars: 0, first: f.t.first().copied(), last: f.t.last().copied(), last_bar_age: None, stale: false, warnings: vec![] };
    // compare each bar with the newest time seen so far, so one bar out of order is not also counted as a gap
    let mut newest = match f.t.first() {
        Some(t) => *t,
        None => return q,
    };
    for &b in f.t.iter().skip(1) {
        let a = newest;
        if b == a {
            q.duplicates += 1;
            continue;
        }
        if b < a {
            q.out_of_order += 1;
            continue;
        }
        newest = b;
        let step = b - a;
        if step <= interval_secs {
            continue;
        }
        let is_gap = if market == "crypto" {
            true
        } else if interval_secs >= 86_400 {
            step > 4 * 86_400
        } else {
            ist_day(a) == ist_day(b)
        };
        if is_gap {
            let missing = step / interval_secs - 1;
            if missing > 0 {
                q.gaps.push((a, b, missing));
                q.missing_bars += missing;
            }
        }
    }
    if let Some(last) = q.last {
        let age = now - last;
        q.last_bar_age = Some(age);
        let limit = if market == "crypto" { 3 * interval_secs } else { (4 * 86_400).max(3 * interval_secs) };
        q.stale = age > limit + interval_secs;
    }
    if q.duplicates > 0 {
        q.warnings.push(format!("{} duplicate bar(s)", q.duplicates));
    }
    if q.out_of_order > 0 {
        q.warnings.push(format!("{} bar(s) out of time order", q.out_of_order));
    }
    if q.missing_bars > 0 {
        q.warnings.push(format!("{} missing bar(s) in {} gap(s)", q.missing_bars, q.gaps.len()));
    }
    if q.stale {
        q.warnings.push("the newest bar is old: data looks stale".into());
    }
    q
}

/// Sort by time and drop duplicate timestamps (keeping the last copy). Returns the cleaned frame and how many
/// bars were removed. Used before a backtest so a bad feed cannot reorder history.
pub fn repair(f: &Frame) -> (Frame, usize) {
    let mut idx: Vec<usize> = (0..f.len()).collect();
    idx.sort_by_key(|&i| f.t[i]);
    let mut out = Frame::default();
    for (k, &i) in idx.iter().enumerate() {
        if k + 1 < idx.len() && f.t[idx[k + 1]] == f.t[i] {
            continue;
        }
        out.push(f.t[i], f.o[i], f.h[i], f.l[i], f.c[i], f.v[i]);
    }
    let removed = f.len() - out.len();
    (out, removed)
}

pub fn to_json(q: &Quality, source: &str) -> Value {
    json!({ "source": source, "quality": q, "checked_utc": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ts: &[i64]) -> Frame {
        let mut f = Frame::default();
        for &t in ts {
            f.push(t, 1.0, 1.0, 1.0, 1.0, 1.0);
        }
        f
    }

    #[test]
    fn crypto_gaps_duplicates_and_disorder() {
        let h = 3600;
        let f = frame(&[0, h, h, 2 * h, 5 * h, 4 * h, 6 * h]);
        let q = check(&f, "crypto", h, 6 * h + 10);
        assert_eq!(q.duplicates, 1);
        assert_eq!(q.out_of_order, 1);
        assert_eq!(q.gaps, vec![(2 * h, 5 * h, 2)]);
        assert!(!q.stale);
        assert!(check(&f, "crypto", h, 6 * h + 5 * h).stale);
        let (r, removed) = repair(&f);
        assert_eq!(removed, 1);
        assert!(r.t.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn nse_overnight_and_weekend_jumps_are_not_gaps() {
        let ist = FixedOffset::east_opt(19800).unwrap();
        let at = |d: u32, h: u32, m: u32| ist.with_ymd_and_hms(2026, 11, d, h, m, 0).unwrap().timestamp();
        // Fri 6th 15:15 -> Mon 9th 09:15 (weekend), then a missing 10:15 bar inside Monday
        let f = frame(&[at(6, 14, 15), at(6, 15, 15), at(9, 9, 15), at(9, 11, 15)]);
        let q = check(&f, "india", 3600, at(9, 11, 30));
        assert_eq!(q.gaps.len(), 1, "{q:?}");
        assert_eq!(q.missing_bars, 1);
    }
}
