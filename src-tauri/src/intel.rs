//! News, sentiment and economic events.
//!
//! No live news or calendar provider is configured (none is named in a spec, and adding an outbound host needs
//! the owner's decision), so items arrive through a provider interface: `manual` (the owner imports a JSON list
//! or a CSV export from a source they trust) and `sample` (made-up items, only with sample data, labelled as
//! such). Every item keeps its provider, publisher, original link and both timestamps (published and retrieved).
//!
//! Dedupe: exact by (provider, external_id); across providers, items whose normalised titles match share a
//! `cluster_key` and are shown once with the count. Sentiment is a transparent word-list score (`lexicon-v1`):
//! a number in [-1, 1] with a confidence that grows with the number of matched words, labelled "heuristic". It is
//! a reading aid, never a trading instruction, and nothing here can place an order.
//!
//! Economic events: imported (manual) with prior, forecast and actual; a changed time becomes `revised`, a
//! filled actual `released`. The owner may switch on a blackout around high-importance events: new entries are
//! then refused by the central risk check for that window (`riskintel::extra_checks`).

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const SENTIMENT_MODEL: &str = "lexicon-v1";
pub const PROVIDERS: [&str; 2] = ["manual", "sample"];

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

const POS: [&str; 24] = ["surge", "surges", "rally", "rallies", "gain", "gains", "beat", "beats", "record", "upgrade", "upgraded", "approval", "approved", "growth", "profit", "bullish", "strong", "soar", "soars", "rise", "rises", "jump", "jumps", "inflows"];
const NEG: [&str; 24] = ["plunge", "plunges", "crash", "crashes", "loss", "losses", "miss", "misses", "downgrade", "downgraded", "ban", "hack", "hacked", "lawsuit", "fraud", "bearish", "weak", "fall", "falls", "drop", "drops", "outflows", "default", "probe"];

fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// `(score, confidence)`: score in [-1, 1]; confidence 0 with no matched words, approaching 0.9 with many.
pub fn sentiment(text: &str) -> (f64, f64) {
    let w = words(text);
    let p = w.iter().filter(|x| POS.contains(&x.as_str())).count() as f64;
    let n = w.iter().filter(|x| NEG.contains(&x.as_str())).count() as f64;
    if p + n == 0.0 {
        return (0.0, 0.0);
    }
    let score = (p - n) / (p + n);
    let conf = (0.9 * (1.0 - (-(p + n) / 3.0).exp()) * (score.abs() * 0.5 + 0.5) * 1000.0).round() / 1000.0;
    ((score * 1000.0).round() / 1000.0, conf)
}

const STOP: [&str; 12] = ["the", "a", "an", "of", "to", "in", "on", "for", "and", "as", "is", "after"];

/// Items about the same story from different providers share this key: sorted significant title words, hashed.
pub fn cluster_key(title: &str) -> String {
    let mut w: Vec<String> = words(title).into_iter().filter(|x| !STOP.contains(&x.as_str())).collect();
    w.sort();
    w.dedup();
    let h = Sha256::digest(w.join(" ").as_bytes());
    h[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// Store news items. Returns counts and the ids of newly stored items. Bad items are reported, not stored.
pub fn ingest_news(conn: &Connection, provider: &str, items: &[Value]) -> Result<Value, String> {
    if !PROVIDERS.contains(&provider) {
        return Err("unknown news provider".into());
    }
    if items.len() > 500 {
        return Err("at most 500 items per import".into());
    }
    let (mut added, mut dup, mut bad) = (vec![], 0, vec![]);
    for (i, it) in items.iter().enumerate() {
        let title = s(it, "title");
        let published = s(it, "published_utc");
        let ext = { let e = s(it, "id"); if e.is_empty() { cluster_key(&format!("{title}{published}")) } else { e } };
        if title.is_empty() || title.len() > 400 || chrono::DateTime::parse_from_rfc3339(&published).is_err() {
            bad.push(json!({ "index": i, "error": "title (up to 400 chars) and an RFC 3339 published_utc are required" }));
            continue;
        }
        let url = s(it, "url");
        if !url.is_empty() && !(url.starts_with("https://") || url.starts_with("http://")) {
            bad.push(json!({ "index": i, "error": "url must be http(s)" }));
            continue;
        }
        let summary: String = s(it, "summary").chars().take(2000).collect();
        let symbols: Vec<String> = it.get("symbols").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_uppercase()).filter(|x| !x.is_empty() && x.len() <= 20).collect()).unwrap_or_default();
        let (sc, cf) = sentiment(&format!("{title}. {summary}"));
        let n = conn
            .execute(
                "INSERT OR IGNORE INTO news_items (provider, external_id, url, title, summary, publisher, published_utc, retrieved_utc, symbols, cluster_key, sentiment, confidence, model) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![provider, ext, (!url.is_empty()).then_some(url), title, summary, s(it, "publisher"), published, now(), symbols.join(","), cluster_key(&title), sc, cf, SENTIMENT_MODEL],
            )
            .map_err(|e| e.to_string())?;
        if n == 1 {
            let id = conn.last_insert_rowid();
            added.push(id);
            if sc.abs() >= 0.5 && cf >= 0.4 && !symbols.is_empty() {
                crate::events::emit(conn, "NewsSentimentChanged", &symbols.join(","), json!({ "news_id": id, "sentiment": sc, "confidence": cf, "model": SENTIMENT_MODEL, "title": title }), None, Some(&format!("news:{id}")));
                crate::events::notify(conn, "news", &format!("{} news for {}", if sc > 0.0 { "Positive" } else { "Negative" }, symbols.join(", ")), &format!("{title} (heuristic sentiment {sc}, confidence {cf}; not advice)"), Some(&format!("news:{}", cluster_key(&title))));
            }
        } else {
            dup += 1;
        }
    }
    Ok(json!({ "provider": provider, "added": added.len(), "duplicates": dup, "rejected": bad, "ids": added }))
}

/// News, newest first, one row per story cluster.
pub fn news(conn: &Connection, symbol: Option<&str>, limit: usize) -> Result<Value, String> {
    let mut st = conn
        .prepare(
            "SELECT id, provider, url, title, summary, publisher, published_utc, retrieved_utc, symbols, cluster_key, sentiment, confidence, model,
                    (SELECT count(*) FROM news_items n2 WHERE n2.cluster_key = n.cluster_key) AS copies
             FROM news_items n WHERE id = (SELECT min(id) FROM news_items n3 WHERE n3.cluster_key = n.cluster_key)
               AND (?1 IS NULL OR (',' || symbols || ',') LIKE '%,' || ?1 || ',%')
             ORDER BY published_utc DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<Value> = st
        .query_map(params![symbol, limit as i64], |r| {
            Ok(json!({ "id": r.get::<_, i64>(0)?, "provider": r.get::<_, String>(1)?, "url": r.get::<_, Option<String>>(2)?, "title": r.get::<_, String>(3)?, "summary": r.get::<_, String>(4)?, "publisher": r.get::<_, String>(5)?,
                "published_utc": r.get::<_, String>(6)?, "retrieved_utc": r.get::<_, String>(7)?, "symbols": r.get::<_, String>(8)?.split(',').filter(|x| !x.is_empty()).collect::<Vec<_>>(),
                "cluster_key": r.get::<_, String>(9)?, "sentiment": r.get::<_, Option<f64>>(10)?, "confidence": r.get::<_, Option<f64>>(11)?, "model": r.get::<_, Option<String>>(12)?, "copies": r.get::<_, i64>(13)?,
                "label": if r.get::<_, String>(1)? == "sample" { "SAMPLE (made up)" } else { "imported" } }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(json!({ "items": rows, "sentiment_model": SENTIMENT_MODEL, "note": "Sentiment is a word-list heuristic with a confidence, not a prediction and not advice." }))
}

/// Made-up items for sample mode only. Labelled `sample` everywhere they appear.
pub fn sample_news(now_utc: chrono::DateTime<chrono::Utc>) -> Vec<Value> {
    let t = |h: i64| (now_utc - chrono::Duration::hours(h)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    vec![
        json!({ "id": "s1", "title": "Sample: BTC rallies to record as inflows surge", "publisher": "Sample Wire", "published_utc": t(2), "symbols": ["BTCUSDT"] }),
        json!({ "id": "s2", "title": "Sample: record inflows as BTC rallies, surge continues", "publisher": "Sample Daily", "published_utc": t(1), "symbols": ["BTCUSDT"] }),
        json!({ "id": "s3", "title": "Sample: exchange hack probe weighs on ETH, prices drop", "publisher": "Sample Wire", "published_utc": t(5), "symbols": ["ETHUSDT"] }),
        json!({ "id": "s4", "title": "Sample: markets quiet ahead of data", "publisher": "Sample Daily", "published_utc": t(8), "symbols": [] }),
    ]
}

// ------------------------------------------------------------------ economic events

const IMPORTANCE: [&str; 3] = ["low", "medium", "high"];

pub fn ingest_events(conn: &Connection, provider: &str, items: &[Value]) -> Result<Value, String> {
    if !PROVIDERS.contains(&provider) {
        return Err("unknown calendar provider".into());
    }
    let (mut added, mut revised, mut released, mut same, mut bad) = (0, 0, 0, 0, vec![]);
    for (i, it) in items.iter().take(1000).enumerate() {
        let (name, region, at, imp, ext) = (s(it, "name"), s(it, "region").to_uppercase(), s(it, "scheduled_utc"), s(it, "importance").to_lowercase(), s(it, "id"));
        if name.is_empty() || ext.is_empty() || region.is_empty() || chrono::DateTime::parse_from_rfc3339(&at).is_err() || !IMPORTANCE.contains(&imp.as_str()) {
            bad.push(json!({ "index": i, "error": "id, name, region, an RFC 3339 scheduled_utc and importance low|medium|high are required" }));
            continue;
        }
        let cancelled = it.get("cancelled").and_then(Value::as_bool).unwrap_or(false);
        let (prior, forecast, actual) = (it.get("prior").map(|v| v.to_string().trim_matches('"').to_string()), it.get("forecast").map(|v| v.to_string().trim_matches('"').to_string()), it.get("actual").map(|v| v.to_string().trim_matches('"').to_string()));
        let old: Option<(String, Option<String>, String)> = conn.query_row("SELECT scheduled_utc, actual, status FROM econ_events WHERE provider = ?1 AND external_id = ?2", params![provider, ext], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(|e| e.to_string())?;
        match old {
            None => {
                let status = if cancelled { "cancelled" } else if actual.is_some() { "released" } else { "scheduled" };
                conn.execute("INSERT INTO econ_events (provider, external_id, name, region, scheduled_utc, importance, prior, forecast, actual, status, retrieved_utc) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", params![provider, ext, name, region, at, imp, prior, forecast, actual, status, now()]).map_err(|e| e.to_string())?;
                added += 1;
            }
            Some((old_at, old_actual, _)) => {
                let status = if cancelled {
                    "cancelled"
                } else if actual.is_some() && old_actual.is_none() {
                    released += 1;
                    "released"
                } else if old_at != at {
                    revised += 1;
                    if at > old_at { "delayed" } else { "revised" }
                } else {
                    same += 1;
                    if actual.is_some() { "released" } else { "scheduled" }
                };
                conn.execute(
                    "UPDATE econ_events SET name = ?3, region = ?4, scheduled_utc = ?5, importance = ?6, prior = ?7, forecast = ?8, actual = ?9, status = ?10, revised_utc = CASE WHEN scheduled_utc <> ?5 THEN ?11 ELSE revised_utc END, retrieved_utc = ?11 WHERE provider = ?1 AND external_id = ?2",
                    params![provider, ext, name, region, at, imp, prior, forecast, actual, status, now()],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(json!({ "provider": provider, "added": added, "revised_or_delayed": revised, "released": released, "unchanged": same, "rejected": bad }))
}

pub fn events_list(conn: &Connection, from: &str, to: &str) -> Result<Value, String> {
    let mut st = conn.prepare("SELECT id, provider, name, region, scheduled_utc, importance, prior, forecast, actual, status, revised_utc, retrieved_utc FROM econ_events WHERE scheduled_utc >= ?1 AND scheduled_utc <= ?2 ORDER BY scheduled_utc LIMIT 500").map_err(|e| e.to_string())?;
    let rows: Vec<Value> = st
        .query_map(params![from, to], |r| {
            Ok(json!({ "id": r.get::<_, i64>(0)?, "provider": r.get::<_, String>(1)?, "name": r.get::<_, String>(2)?, "region": r.get::<_, String>(3)?, "scheduled_utc": r.get::<_, String>(4)?, "importance": r.get::<_, String>(5)?,
                "prior": r.get::<_, Option<String>>(6)?, "forecast": r.get::<_, Option<String>>(7)?, "actual": r.get::<_, Option<String>>(8)?, "status": r.get::<_, String>(9)?, "revised_utc": r.get::<_, Option<String>>(10)?, "retrieved_utc": r.get::<_, String>(11)? }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(json!({ "events": rows, "blackout": blackout_config(conn) }))
}

/// Which regions an instrument is exposed to, for event windows. Crypto trades globally: US data moves it most.
pub fn regions_for(market: &str) -> &'static [&'static str] {
    if market == "india" {
        &["IN", "US"]
    } else {
        &["US", "GLOBAL"]
    }
}

pub fn blackout_config(conn: &Connection) -> Value {
    crate::events::kv_get(conn, "econ.blackout").and_then(|v| serde_json::from_str(&v).ok()).unwrap_or(json!({ "enabled": false, "minutes_before": 30, "minutes_after": 30 }))
}

pub fn set_blackout(conn: &Connection, b: &Value) -> Result<Value, String> {
    let before = b.get("minutes_before").and_then(Value::as_i64).unwrap_or(30);
    let after = b.get("minutes_after").and_then(Value::as_i64).unwrap_or(30);
    if !(0..=720).contains(&before) || !(0..=720).contains(&after) {
        return Err("minutes must be between 0 and 720".into());
    }
    let v = json!({ "enabled": b.get("enabled").and_then(Value::as_bool).unwrap_or(false), "minutes_before": before, "minutes_after": after });
    crate::events::kv_set(conn, "econ.blackout", &v.to_string());
    let _ = crate::audit::record(conn, "user", "econ_blackout_set", "risk", &v);
    Ok(v)
}

/// The high-importance event whose blackout window covers `at` for this market, if the blackout is on.
pub fn blackout_hit(conn: &Connection, market: &str, at: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let b = blackout_config(conn);
    if b["enabled"] != true {
        return None;
    }
    let from = (at - chrono::Duration::minutes(b["minutes_after"].as_i64().unwrap_or(30))).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let to = (at + chrono::Duration::minutes(b["minutes_before"].as_i64().unwrap_or(30))).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let regions = regions_for(market);
    let mut st = conn.prepare("SELECT name, region, scheduled_utc FROM econ_events WHERE importance = 'high' AND status <> 'cancelled' AND scheduled_utc >= ?1 AND scheduled_utc <= ?2").ok()?;
    let hit = st.query_map(params![from, to], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).ok()?.filter_map(Result::ok).find(|(_, rg, _)| regions.contains(&rg.as_str()));
    hit.map(|(n, rg, t)| format!("{n} ({rg}) at {t}"))
}

/// Emit `EconomicEventApproaching` once per event, an hour ahead.
pub fn announce(conn: &Connection, now_utc: chrono::DateTime<chrono::Utc>) {
    let to = (now_utc + chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let from = now_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rows: Vec<(i64, String, String, String, String)> = conn
        .prepare("SELECT id, name, region, scheduled_utc, importance FROM econ_events WHERE status IN ('scheduled','revised','delayed') AND importance <> 'low' AND scheduled_utc > ?1 AND scheduled_utc <= ?2")
        .and_then(|mut st| st.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).map(|m| m.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    for (id, name, region, at, imp) in rows {
        if crate::events::emit(conn, "EconomicEventApproaching", &region, json!({ "event_id": id, "name": name, "scheduled_utc": at, "importance": imp }), None, Some(&format!("econ:{id}:{at}"))).is_some() {
            crate::events::notify(conn, "calendar", &format!("{name} ({region}) within the hour"), &format!("{imp} importance, scheduled {at}"), Some(&format!("econ:{id}:{at}")));
        }
    }
}

/// Parse a CSV export (header row; columns matched by name) into the JSON items `ingest_events` takes.
pub fn events_from_csv(text: &str) -> Result<Vec<Value>, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let head: Vec<String> = lines.next().ok_or("empty CSV")?.split(',').map(|h| h.trim().to_lowercase()).collect();
    let col = |n: &str| head.iter().position(|h| h == n);
    let need = ["id", "name", "region", "scheduled_utc", "importance"];
    if let Some(m) = need.iter().find(|n| col(n).is_none()) {
        return Err(format!("CSV is missing the {m} column"));
    }
    Ok(lines
        .map(|l| {
            let cells: Vec<&str> = l.split(',').map(str::trim).collect();
            let mut o = serde_json::Map::new();
            for (i, h) in head.iter().enumerate() {
                if let Some(c) = cells.get(i).filter(|c| !c.is_empty()) {
                    o.insert(h.clone(), json!(c));
                }
            }
            Value::Object(o)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentiment_is_bounded_and_explainable() {
        let (p, cp) = sentiment("Bitcoin surges to record as inflows jump");
        let (n, _) = sentiment("Exchange hacked, prices plunge, lawsuit follows");
        let (z, cz) = sentiment("Committee meets on Tuesday");
        assert!(p > 0.9 && cp > 0.4, "{p} {cp}");
        assert!(n < -0.9);
        assert_eq!((z, cz), (0.0, 0.0));
        let (m, _) = sentiment("Gains erased as stocks fall");
        assert!(m.abs() < 0.01);
    }

    #[test]
    fn news_dedupes_and_clusters() {
        let c = crate::db::open(":memory:").unwrap();
        let items = sample_news(chrono::Utc::now());
        let r = ingest_news(&c, "sample", &items).unwrap();
        assert_eq!(r["added"], 4);
        assert_eq!(ingest_news(&c, "sample", &items).unwrap()["duplicates"], 4, "same provider ids are stored once");
        // the same story from another provider clusters with the first
        ingest_news(&c, "manual", &[json!({ "id": "m1", "title": "Sample: BTC rallies to record as inflows surge", "published_utc": "2026-11-01T00:00:00Z", "symbols": ["btcusdt"] })]).unwrap();
        let n = news(&c, Some("BTCUSDT"), 50).unwrap();
        let rows = n["items"].as_array().unwrap();
        assert!(rows.iter().any(|x| x["copies"].as_i64() == Some(2)), "{n}");
        assert!(rows.iter().all(|x| x["model"] == SENTIMENT_MODEL));
        // strongly worded symbol news raised an event and a notification
        let ev = crate::events::list(&c, 0, Some("NewsSentimentChanged"), 10).unwrap();
        assert!(!ev.is_empty());
        let bad = ingest_news(&c, "manual", &[json!({ "title": "x", "published_utc": "yesterday" }), json!({ "title": "y", "published_utc": "2026-11-01T00:00:00Z", "url": "javascript:alert(1)" })]).unwrap();
        assert_eq!(bad["rejected"].as_array().unwrap().len(), 2);
        assert!(ingest_news(&c, "twitter", &[]).is_err());
    }

    #[test]
    fn events_revise_release_and_blackout() {
        let c = crate::db::open(":memory:").unwrap();
        let soon = (chrono::Utc::now() + chrono::Duration::minutes(20)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let ev = json!({ "id": "cpi-nov", "name": "US CPI", "region": "US", "scheduled_utc": soon, "importance": "high", "forecast": "3.1" });
        assert_eq!(ingest_events(&c, "manual", std::slice::from_ref(&ev)).unwrap()["added"], 1);
        assert!(blackout_hit(&c, "crypto", chrono::Utc::now()).is_none(), "blackout is off by default");
        set_blackout(&c, &json!({ "enabled": true, "minutes_before": 30, "minutes_after": 15 })).unwrap();
        assert!(blackout_hit(&c, "crypto", chrono::Utc::now()).unwrap().contains("US CPI"));
        assert!(blackout_hit(&c, "crypto", chrono::Utc::now() - chrono::Duration::hours(3)).is_none());
        announce(&c, chrono::Utc::now());
        announce(&c, chrono::Utc::now());
        assert_eq!(crate::events::list(&c, 0, Some("EconomicEventApproaching"), 10).unwrap().len(), 1, "announced once");
        let mut later = ev.clone();
        later["scheduled_utc"] = json!((chrono::Utc::now() + chrono::Duration::hours(5)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        assert_eq!(ingest_events(&c, "manual", &[later.clone()]).unwrap()["revised_or_delayed"], 1);
        later["actual"] = json!("3.3");
        assert_eq!(ingest_events(&c, "manual", &[later]).unwrap()["released"], 1);
        let st: String = c.query_row("SELECT status FROM econ_events", [], |r| r.get(0)).unwrap();
        assert_eq!(st, "released");
        let csv = events_from_csv("id,name,region,scheduled_utc,importance\nrbi,RBI policy,IN,2026-12-05T04:30:00Z,high\n").unwrap();
        assert_eq!(ingest_events(&c, "manual", &csv).unwrap()["added"], 1);
        assert!(events_from_csv("name\nx").is_err());
        assert!(set_blackout(&c, &json!({ "minutes_before": 9999 })).is_err());
    }
}
