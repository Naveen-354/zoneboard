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
}
