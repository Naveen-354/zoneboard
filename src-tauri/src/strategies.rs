//! Strategy registry with immutable versions.
//!
//! A version is a validated strategy spec plus its SHA-256 content hash. Versions are never edited: the
//! database refuses UPDATE and DELETE on `strategy_versions` (triggers in migration 7), so "editing" a strategy
//! publishes a new version. Bots and saved backtests point at the exact version they used, which is what a
//! live approval is bound to.
//!
//! Export format (also the import format): `{"format": "zoneboard-strategy", "schema": 1, "name", "spec",
//! "hash"}`. Import re-validates the spec and recomputes the hash; a mismatch is refused.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const EXPORT_FORMAT: &str = "zoneboard-strategy";
pub const SCHEMA: i64 = 1;

/// Canonical JSON (object keys sorted, no whitespace), so equal specs hash equally whatever their key order.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            format!("{{{}}}", keys.iter().map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical(&m[*k]))).collect::<Vec<_>>().join(","))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

pub fn hash(spec: &Value) -> String {
    Sha256::digest(canonical(spec).as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Publish `spec` under strategy `name` (created if new). The same content as an existing version of that
/// strategy returns that version instead of a duplicate. Returns (version id, version number, hash).
pub fn publish(conn: &Connection, name: &str, spec: &Value, note: &str, actor: &str) -> Result<(i64, i64, String), String> {
    crate::strategy::parse(spec)?;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("strategy name must be 1 to 60 characters".into());
    }
    let h = hash(spec);
    let sid: i64 = match conn.query_row("SELECT id FROM strategies WHERE name = ?1", [name], |r| r.get(0)).optional().map_err(|e| e.to_string())? {
        Some(id) => id,
        None => {
            conn.execute("INSERT INTO strategies (name, created_utc) VALUES (?1, ?2)", params![name, now()]).map_err(|e| e.to_string())?;
            conn.last_insert_rowid()
        }
    };
    if let Some((vid, v)) = conn.query_row("SELECT id, version FROM strategy_versions WHERE strategy_id = ?1 AND hash = ?2", params![sid, h], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(|e| e.to_string())? {
        return Ok((vid, v, h));
    }
    let next: i64 = conn.query_row("SELECT coalesce(max(version), 0) + 1 FROM strategy_versions WHERE strategy_id = ?1", [sid], |r| r.get(0)).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO strategy_versions (strategy_id, version, spec, hash, note, created_utc) VALUES (?1,?2,?3,?4,?5,?6)",
        params![sid, next, canonical(spec), h, note.chars().take(500).collect::<String>(), now()],
    )
    .map_err(|e| e.to_string())?;
    let vid = conn.last_insert_rowid();
    crate::events::emit(conn, "StrategyVersionCreated", &format!("strategy:{sid}"), json!({ "strategy_id": sid, "version": next, "hash": h }), None, Some(&format!("sv:{sid}:{h}")));
    crate::audit::record(conn, actor, "strategy_version_published", &format!("strategy:{sid}"), &json!({ "strategy_id": sid, "name": name, "version": next, "hash": h })).map_err(|e| e.to_string())?;
    Ok((vid, next, h))
}

pub fn version(conn: &Connection, vid: i64) -> Result<Option<Value>, String> {
    conn.query_row(
        "SELECT v.id, v.strategy_id, s.name, v.version, v.spec, v.hash, v.note, v.created_utc FROM strategy_versions v JOIN strategies s ON s.id = v.strategy_id WHERE v.id = ?1",
        [vid],
        |r| {
            let spec: String = r.get(4)?;
            Ok(json!({ "id": r.get::<_, i64>(0)?, "strategy_id": r.get::<_, i64>(1)?, "name": r.get::<_, String>(2)?, "version": r.get::<_, i64>(3)?, "spec": serde_json::from_str::<Value>(&spec).unwrap_or(Value::Null), "hash": r.get::<_, String>(5)?, "note": r.get::<_, String>(6)?, "created_utc": r.get::<_, String>(7)? }))
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn list(conn: &Connection) -> Result<Value, String> {
    let mut st = conn.prepare("SELECT s.id, s.name, s.created_utc, (SELECT count(*) FROM strategy_versions v WHERE v.strategy_id = s.id), (SELECT max(version) FROM strategy_versions v WHERE v.strategy_id = s.id) FROM strategies s ORDER BY s.name").map_err(|e| e.to_string())?;
    let rows: Vec<Value> = st
        .query_map([], |r| Ok(json!({ "id": r.get::<_, i64>(0)?, "name": r.get::<_, String>(1)?, "created_utc": r.get::<_, String>(2)?, "versions": r.get::<_, i64>(3)?, "latest": r.get::<_, Option<i64>>(4)? })))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(json!({ "strategies": rows }))
}

pub fn versions(conn: &Connection, sid: i64) -> Result<Value, String> {
    let mut st = conn.prepare("SELECT id FROM strategy_versions WHERE strategy_id = ?1 ORDER BY version DESC").map_err(|e| e.to_string())?;
    let ids: Vec<i64> = st.query_map([sid], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    let mut out = vec![];
    for id in ids {
        if let Some(v) = version(conn, id)? {
            out.push(v);
        }
    }
    if out.is_empty() {
        return Err("no such strategy".into());
    }
    Ok(json!({ "strategy_id": sid, "versions": out }))
}

pub fn export(conn: &Connection, vid: i64) -> Result<Value, String> {
    let v = version(conn, vid)?.ok_or("no such strategy version")?;
    Ok(json!({ "format": EXPORT_FORMAT, "schema": SCHEMA, "name": v["name"], "version": v["version"], "spec": v["spec"], "hash": v["hash"], "exported_utc": now() }))
}

pub fn import(conn: &Connection, doc: &Value) -> Result<Value, String> {
    if doc.get("format").and_then(Value::as_str) != Some(EXPORT_FORMAT) {
        return Err(format!("not a ZoneBoard strategy file (format must be {EXPORT_FORMAT})"));
    }
    match doc.get("schema").and_then(Value::as_i64) {
        Some(SCHEMA) => {}
        Some(n) => return Err(format!("strategy file schema {n} is not supported (this build reads schema {SCHEMA})")),
        None => return Err("strategy file has no schema number".into()),
    }
    let spec = doc.get("spec").ok_or("strategy file has no spec")?;
    if let Some(h) = doc.get("hash").and_then(Value::as_str) {
        if h != hash(spec) {
            return Err("the file's hash does not match its spec: it was changed or damaged".into());
        }
    }
    let name = doc.get("name").and_then(Value::as_str).unwrap_or("Imported strategy");
    let (vid, v, h) = publish(conn, name, spec, "imported", "user")?;
    Ok(json!({ "version_id": vid, "version": v, "hash": h }))
}

pub const TAGS: [&str; 4] = ["experimental", "validated", "paper-approved", "production-approved"];

/// Every difference between two specs: (JSON pointer, value in a, value in b). Missing = null.
pub fn diff_values(a: &Value, b: &Value) -> Vec<Value> {
    fn walk(a: &Value, b: &Value, path: String, out: &mut Vec<Value>) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
                keys.sort();
                keys.dedup();
                for k in keys {
                    walk(x.get(k).unwrap_or(&Value::Null), y.get(k).unwrap_or(&Value::Null), format!("{path}/{k}"), out);
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                for i in 0..x.len().max(y.len()) {
                    walk(x.get(i).unwrap_or(&Value::Null), y.get(i).unwrap_or(&Value::Null), format!("{path}/{i}"), out);
                }
            }
            _ if a != b => out.push(json!({ "path": if path.is_empty() { "/".into() } else { path }, "from": a, "to": b })),
            _ => {}
        }
    }
    let mut out = vec![];
    walk(a, b, String::new(), &mut out);
    out
}

pub fn diff(conn: &Connection, a: i64, b: i64) -> Result<Value, String> {
    let va = version(conn, a)?.ok_or("no such strategy version")?;
    let vb = version(conn, b)?.ok_or("no such strategy version")?;
    let changes = diff_values(&va["spec"], &vb["spec"]);
    let kind = |p: &str| if p.starts_with("/indicators") && p.contains("/params/") { "parameter" } else if p.starts_with("/entry") || p.starts_with("/exit") || p.starts_with("/indicators") || p == "/kind" || p == "/direction" { "logic" } else { "configuration" };
    let changes: Vec<Value> = changes.into_iter().map(|mut c| {
        let k = kind(c["path"].as_str().unwrap_or(""));
        c["kind"] = json!(k);
        c
    }).collect();
    Ok(json!({ "from": { "id": a, "name": va["name"], "version": va["version"] }, "to": { "id": b, "name": vb["name"], "version": vb["version"] }, "changes": changes }))
}

/// Tag a version. `production-approved` needs an approved live-deployment approval of that exact version.
pub fn tag(conn: &Connection, vid: i64, tag: &str) -> Result<Value, String> {
    if !TAGS.contains(&tag) {
        return Err(format!("tag must be one of {}", TAGS.join(", ")));
    }
    version(conn, vid)?.ok_or("no such strategy version")?;
    if tag == "production-approved" {
        let n: i64 = conn.query_row("SELECT count(*) FROM approvals WHERE strategy_version_id = ?1 AND status = 'approved'", [vid], |r| r.get(0)).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("production-approved needs an approved live deployment of this exact version (Approval Center)".into());
        }
    }
    conn.execute("INSERT OR IGNORE INTO strategy_tags (version_id, tag, created_utc) VALUES (?1,?2,?3)", params![vid, tag, now()]).map_err(|e| e.to_string())?;
    crate::audit::record(conn, "user", "strategy_version_tagged", &format!("strategy_version:{vid}"), &json!({ "version_id": vid, "tag": tag })).map_err(|e| e.to_string())?;
    detail(conn, vid)
}

/// Roll back by publishing a NEW version with an earlier version's logic. History is never rewritten; the new
/// version's notes say where it came from (which also gives it its own content hash).
pub fn rollback(conn: &Connection, vid: i64) -> Result<Value, String> {
    let v = version(conn, vid)?.ok_or("no such strategy version")?;
    let mut spec = v["spec"].clone();
    let latest: i64 = conn.query_row("SELECT max(version) FROM strategy_versions WHERE strategy_id = ?1", [v["strategy_id"].as_i64()], |r| r.get(0)).map_err(|e| e.to_string())?;
    spec["notes"] = json!(format!("rollback to v{} (from v{latest})", v["version"]));
    let (nid, n, h) = publish(conn, v["name"].as_str().unwrap_or(""), &spec, &format!("rollback to v{}", v["version"]), "user")?;
    crate::audit::record(conn, "user", "strategy_rolled_back", &format!("strategy:{}", v["strategy_id"]), &json!({ "from_version": latest, "to_logic_of": v["version"], "new_version": n })).map_err(|e| e.to_string())?;
    Ok(json!({ "version_id": nid, "version": n, "hash": h }))
}

/// A version with its tags and everything that used it: bots, backtests, approvals, trades.
pub fn detail(conn: &Connection, vid: i64) -> Result<Value, String> {
    let mut v = version(conn, vid)?.ok_or("no such strategy version")?;
    let list = |sql: &str| -> Vec<Value> {
        conn.prepare(sql).and_then(|mut st| st.query_map([vid], |r| r.get::<_, i64>(0)).map(|m| m.filter_map(Result::ok).map(|x| json!(x)).collect())).unwrap_or_default()
    };
    let tags: Vec<String> = conn.prepare("SELECT tag FROM strategy_tags WHERE version_id = ?1 ORDER BY tag").and_then(|mut st| st.query_map([vid], |r| r.get(0)).map(|m| m.filter_map(Result::ok).collect())).unwrap_or_default();
    v["tags"] = json!(tags);
    v["bots"] = json!(list("SELECT id FROM bots WHERE strategy_version_id = ?1"));
    v["backtests"] = json!(list("SELECT id FROM backtests WHERE strategy_version_id = ?1"));
    v["approvals"] = json!(list("SELECT id FROM approvals WHERE strategy_version_id = ?1"));
    v["trades"] = json!(conn.query_row("SELECT count(*) FROM trades WHERE strategy_version_id = ?1", [vid], |r| r.get::<_, i64>(0)).unwrap_or(0));
    Ok(v)
}

pub const PACK_FORMAT: &str = "zoneboard-pack";

/// Every strategy's latest version as one pack.
pub fn export_pack(conn: &Connection) -> Result<Value, String> {
    let mut st = conn.prepare("SELECT max(id) FROM strategy_versions GROUP BY strategy_id").map_err(|e| e.to_string())?;
    let ids: Vec<i64> = st.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    let docs: Result<Vec<Value>, String> = ids.into_iter().map(|id| export(conn, id)).collect();
    Ok(json!({ "format": PACK_FORMAT, "schema": SCHEMA, "requires_engine": crate::backtest2::ENGINE_VERSION, "name": "ZoneBoard strategies", "strategies": docs? }))
}

/// Import a pack: every strategy is checked first; if any fails, nothing is imported.
pub fn import_pack(conn: &Connection, pack: &Value) -> Result<Value, String> {
    if pack.get("format").and_then(Value::as_str) != Some(PACK_FORMAT) {
        return Err(format!("not a strategy pack (format must be {PACK_FORMAT})"));
    }
    if pack.get("schema").and_then(Value::as_i64) != Some(SCHEMA) {
        return Err(format!("pack schema not supported (this build reads schema {SCHEMA})"));
    }
    if let Some(e) = pack.get("requires_engine").and_then(Value::as_str) {
        if e != crate::backtest2::ENGINE_VERSION {
            return Err(format!("this pack was made for engine {e}; this build runs {}: results could differ, so it is refused", crate::backtest2::ENGINE_VERSION));
        }
    }
    let list = pack.get("strategies").and_then(Value::as_array).ok_or("a pack needs a strategies list")?;
    if list.is_empty() || list.len() > 200 {
        return Err("a pack holds 1 to 200 strategies".into());
    }
    for (i, d) in list.iter().enumerate() {
        let spec = d.get("spec").ok_or(format!("strategy {}: no spec", i + 1))?;
        crate::strategy::parse(spec).map_err(|e| format!("strategy {}: {e}", i + 1))?;
        if d.get("hash").and_then(Value::as_str).is_some_and(|h| h != hash(spec)) {
            return Err(format!("strategy {}: hash mismatch (changed or damaged)", i + 1));
        }
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let mut out = vec![];
    for d in list {
        out.push(import(&tx, d)?);
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(json!({ "imported": out }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Value {
        crate::strategy::templates().into_iter().find(|t| t["id"] == "rsi").unwrap()["spec"].clone()
    }

    #[test]
    fn versions_are_content_addressed_and_immutable() {
        let c = crate::db::open(":memory:").unwrap();
        let (v1, n1, h1) = publish(&c, "RSI", &spec(), "", "user").unwrap();
        let (again, n_again, _) = publish(&c, "RSI", &spec(), "", "user").unwrap();
        assert_eq!((again, n_again), (v1, n1), "same content, same version");
        let mut s2 = spec();
        s2["indicators"][0]["params"]["period"] = json!(10);
        let (_, n2, h2) = publish(&c, "RSI", &s2, "faster", "user").unwrap();
        assert_eq!((n1, n2), (1, 2));
        assert_ne!(h1, h2);
        // the database itself refuses edits and deletes of a published version
        assert!(c.execute("UPDATE strategy_versions SET spec = '{}' WHERE id = ?1", [v1]).is_err());
        assert!(c.execute("DELETE FROM strategy_versions WHERE id = ?1", [v1]).is_err());
        // key order does not change the hash
        let reordered: Value = serde_json::from_str(&canonical(&spec())).unwrap();
        assert_eq!(hash(&reordered), h1);
        assert!(publish(&c, "Bad", &json!({"kind": "nope"}), "", "user").is_err());
    }

    #[test]
    fn export_import_round_trip_and_tamper_detection() {
        let c = crate::db::open(":memory:").unwrap();
        let (v1, _, h) = publish(&c, "RSI", &spec(), "", "user").unwrap();
        let doc = export(&c, v1).unwrap();
        let c2 = crate::db::open(":memory:").unwrap();
        let got = import(&c2, &doc).unwrap();
        assert_eq!(got["hash"], h);
        let mut bad = doc.clone();
        bad["spec"]["direction"] = json!("short");
        assert!(import(&c2, &bad).unwrap_err().contains("hash"));
        let mut future = doc;
        future["schema"] = json!(99);
        assert!(import(&c2, &future).unwrap_err().contains("schema"));
    }

    #[test]
    fn diff_tags_rollback_and_trade_links() {
        let mut c = crate::db::open(":memory:").unwrap();
        let (v1, _, _) = publish(&c, "RSI", &spec(), "", "user").unwrap();
        let mut s2 = spec();
        s2["indicators"][0]["params"]["period"] = json!(10);
        s2["direction"] = json!("long");
        s2["stop_loss"]["value"] = json!(7);
        let (v2, _, _) = publish(&c, "RSI", &s2, "", "user").unwrap();
        let d = diff(&c, v1, v2).unwrap();
        let kinds: Vec<(&str, &str)> = d["changes"].as_array().unwrap().iter().map(|x| (x["path"].as_str().unwrap(), x["kind"].as_str().unwrap())).collect();
        assert!(kinds.contains(&("/indicators/0/params/period", "parameter")), "{kinds:?}");
        assert!(kinds.contains(&("/stop_loss/value", "configuration")));
        // rollback publishes v3 with v1's logic; v1 and v2 are untouched
        let rb = rollback(&c, v1).unwrap();
        assert_eq!(rb["version"], 3);
        let logic = diff(&c, v1, rb["version_id"].as_i64().unwrap()).unwrap();
        assert!(logic["changes"].as_array().unwrap().iter().all(|x| x["path"] == "/notes"), "only the note differs: {logic}");
        assert!(tag(&c, v2, "production-approved").unwrap_err().contains("approved live deployment"));
        assert_eq!(tag(&c, v2, "validated").unwrap()["tags"], json!(["validated"]));
        assert!(tag(&c, v2, "shiny").is_err());
        // a trade made by a bot keeps the bot's version
        let id = crate::bots::create(&mut c, &json!({ "name": "t", "symbol": "BTCUSDT", "interval": "1h", "spec": spec(), "allocation": "1000" })).unwrap();
        let bot = crate::bots::load(&c, id).unwrap();
        let r = crate::orders::BotRef { id, name: "t", account_id: "demo-1", market: "crypto", capital: rust_decimal::Decimal::from(1000), max_daily_loss: None, max_open_positions: 5 };
        let req = crate::botengine::OpenReq { leg: 0, side: crate::strategy::Side::Long, qty: 1.0, price: 100.0, time: 1, stop: None, target: None, trail: None, tag: "signal".into() };
        let pos = crate::orders::open(&mut c, &r, "BTCUSDT", &req, &crate::risk::Gate::default()).unwrap();
        crate::orders::close(&mut c, &r, pos, 110.0, 2, "exit").unwrap();
        let tv: Option<i64> = c.query_row("SELECT strategy_version_id FROM trades WHERE bot_id = ?1", [id], |r| r.get(0)).unwrap();
        let bv: Option<i64> = c.query_row("SELECT strategy_version_id FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        assert_eq!(tv, bv);
        assert_eq!(detail(&c, bv.unwrap()).unwrap()["trades"], 1);
        // concurrent edit: a stale revision is refused
        crate::bots::update(&mut c, id, &json!({ "name": "first", "revision": 0 })).unwrap();
        assert!(matches!(crate::bots::update(&mut c, id, &json!({ "name": "second", "revision": 0 })), Err(crate::bots::BotError::Conflict(_))));
        let _ = bot;
    }

    #[test]
    fn packs_are_all_or_nothing_and_engine_checked() {
        let c = crate::db::open(":memory:").unwrap();
        publish(&c, "RSI", &spec(), "", "user").unwrap();
        let pack = export_pack(&c).unwrap();
        let c2 = crate::db::open(":memory:").unwrap();
        let mut bad = pack.clone();
        bad["strategies"].as_array_mut().unwrap().push(json!({ "format": EXPORT_FORMAT, "schema": 1, "name": "Bad", "spec": { "kind": "nope" } }));
        assert!(import_pack(&c2, &bad).is_err());
        assert_eq!(list(&c2).unwrap()["strategies"].as_array().unwrap().len(), 0, "nothing imported when one fails");
        let mut old = pack.clone();
        old["requires_engine"] = json!("botengine-0");
        assert!(import_pack(&c2, &old).unwrap_err().contains("engine"));
        assert_eq!(import_pack(&c2, &pack).unwrap()["imported"].as_array().unwrap().len(), 1);
    }
}
