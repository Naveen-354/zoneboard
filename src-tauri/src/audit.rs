//! Append-only audit log for tracking security- and money-relevant system and user events.
//!
//! # Guarantees
//! - Append-only storage: database triggers prevent UPDATE, DELETE, and INSERT OR REPLACE.
//! - Automatic secret redaction: all `detail` fields matching sensitive credential names
//!   (via [`crate::logging::is_secret_field`]) are replaced with `"[REDACTED]"` before write.

use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An entry retrieved from the `audit_log` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEntry {
    pub id: i64,
    pub ts_utc: String,
    pub actor: String,
    pub action: String,
    pub entity: String,
    pub detail: Value,
}

/// Recursively traverses a JSON value and masks any object fields whose key matches
/// [`crate::logging::is_secret_field`] with `"[REDACTED]"`.
pub fn redact_value(val: &Value) -> Value {
    match val {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if crate::logging::is_secret_field(k) {
                    out.insert(k.clone(), Value::String("[REDACTED]".to_string()));
                } else {
                    out.insert(k.clone(), redact_value(v));
                }
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(redact_value).collect()),
        other => other.clone(),
    }
}

/// Appends a new audit record to the `audit_log` table.
///
/// The `detail` JSON is passed through [`redact_value`] before insertion.
/// Returns the generated row `id`.
pub fn record(
    conn: &Connection,
    actor: &str,
    action: &str,
    entity: &str,
    detail: &Value,
) -> Result<i64, rusqlite::Error> {
    let redacted_detail = redact_value(detail);
    let detail_json = serde_json::to_string(&redacted_detail).unwrap_or_else(|_| "{}".to_string());
    let now_utc = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    conn.execute(
        "INSERT INTO audit_log (ts_utc, actor, action, entity, detail_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now_utc, actor, action, entity, detail_json],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Queries audit log entries ordered newest first (descending by `id`).
///
/// Pagination is cursor-based on `id`:
/// - If `before` is provided, only rows with `id < before` are returned.
/// - At most `limit` entries are returned.
/// - Returns `(entries, next_before)` where `next_before` is `Some(last_id)` if there may be
///   more entries, or `None` if this page reaches the end of the log.
pub fn list(
    conn: &Connection,
    limit: usize,
    before: Option<i64>,
) -> Result<(Vec<AuditEntry>, Option<i64>), rusqlite::Error> {
    let fetch_limit = limit.saturating_add(1) as i64;

    let mut stmt = match before {
        Some(_) => conn.prepare(
            "SELECT id, ts_utc, actor, action, entity, detail_json
             FROM audit_log
             WHERE id < ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?,
        None => conn.prepare(
            "SELECT id, ts_utc, actor, action, entity, detail_json
             FROM audit_log
             ORDER BY id DESC
             LIMIT ?1",
        )?,
    };

    let rows = match before {
        Some(cursor_id) => stmt.query_map(params![cursor_id, fetch_limit], row_to_entry)?,
        None => stmt.query_map(params![fetch_limit], row_to_entry)?,
    };

    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }

    if entries.len() > limit {
        entries.truncate(limit);
        let next_before = entries.last().map(|e| e.id);
        Ok((entries, next_before))
    } else {
        Ok((entries, None))
    }
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<AuditEntry> {
    let detail_str: String = row.get(5)?;
    let detail: Value = serde_json::from_str(&detail_str).unwrap_or(Value::Null);
    Ok(AuditEntry {
        id: row.get(0)?,
        ts_utc: row.get(1)?,
        actor: row.get(2)?,
        action: row.get(3)?,
        entity: row.get(4)?,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup_test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut conn).unwrap();
        conn
    }

    #[test]
    fn redact_value_masks_secret_fields_recursively() {
        let input = json!({
            "apiKey": "super-secret-123",
            "safe_user": "alice",
            "nested": {
                "token": "token-xyz",
                "nested_safe": 42
            },
            "array": [
                { "password": "pass", "count": 1 },
                { "safe_item": true }
            ]
        });

        let redacted = redact_value(&input);

        assert_eq!(redacted["apiKey"], "[REDACTED]");
        assert_eq!(redacted["safe_user"], "alice");
        assert_eq!(redacted["nested"]["token"], "[REDACTED]");
        assert_eq!(redacted["nested"]["nested_safe"], 42);
        assert_eq!(redacted["array"][0]["password"], "[REDACTED]");
        assert_eq!(redacted["array"][0]["count"], 1);
        assert_eq!(redacted["array"][1]["safe_item"], true);
    }

    #[test]
    fn record_writes_audit_entry_and_redacts_detail() {
        let conn = setup_test_db();
        let detail = json!({
            "token": "secret-token",
            "user": "trader1"
        });

        let id = record(&conn, "user", "login", "auth", &detail).unwrap();
        assert!(id > 0);

        // a fresh database already holds migration 4's `account_created` row for demo-1
        let (entries, next_before) = list(&conn, 10, None).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].action, "account_created");
        assert_eq!(next_before, None);

        let entry = &entries[0];
        assert_eq!(entry.id, id);
        assert_eq!(entry.actor, "user");
        assert_eq!(entry.action, "login");
        assert_eq!(entry.entity, "auth");
        assert_eq!(entry.detail["token"], "[REDACTED]");
        assert_eq!(entry.detail["user"], "trader1");
    }

    #[test]
    fn triggers_block_update_on_audit_log() {
        let conn = setup_test_db();
        let id = record(&conn, "system", "app_start", "system", &json!({})).unwrap();

        let res = conn.execute("UPDATE audit_log SET actor = 'attacker' WHERE id = ?1", params![id]);
        assert!(res.is_err(), "UPDATE must be forbidden by trigger");
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("audit_log is append-only"), "{err_msg}");

        // Row was not modified
        let actor: String = conn.query_row("SELECT actor FROM audit_log WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(actor, "system");
    }

    #[test]
    fn triggers_block_delete_on_audit_log() {
        let conn = setup_test_db();
        let id = record(&conn, "system", "app_start", "system", &json!({})).unwrap();

        let res = conn.execute("DELETE FROM audit_log WHERE id = ?1", params![id]);
        assert!(res.is_err(), "DELETE must be forbidden by trigger");
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("audit_log is append-only"), "{err_msg}");

        // Row was not deleted
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM audit_log WHERE id = ?1", params![id], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn triggers_block_replace_into_audit_log_and_original_remains() {
        let conn = setup_test_db();
        let id = record(&conn, "system", "app_start", "system", &json!({"ver": 1})).unwrap();

        // INSERT OR REPLACE over existing id must be blocked by audit_log_no_replace trigger
        let res = conn.execute(
            "INSERT OR REPLACE INTO audit_log (id, ts_utc, actor, action, entity, detail_json)
             VALUES (?1, '2026-11-03T00:00:00Z', 'attacker', 'tamper', 'system', '{}')",
            params![id],
        );
        assert!(res.is_err(), "INSERT OR REPLACE must be blocked by trigger");
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("audit_log is append-only"), "{err_msg}");

        // Verify original row is completely untouched
        let (actor, action): (String, String) = conn.query_row(
            "SELECT actor, action FROM audit_log WHERE id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).unwrap();
        assert_eq!(actor, "system");
        assert_eq!(action, "app_start");
    }

    #[test]
    fn pagination_cursor_orders_newest_first_and_handles_next_before() {
        let conn = setup_test_db();
        for i in 1..=5 {
            record(&conn, "user", &format!("action_{i}"), "test", &json!({"seq": i})).unwrap();
        }

        // Page 1: limit 2 -> entries 5 and 4, next_before 4
        let (page1, next1) = list(&conn, 2, None).unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].action, "action_5");
        assert_eq!(page1[1].action, "action_4");
        assert_eq!(next1, Some(page1[1].id));

        // Page 2: limit 2, before 4 -> entries 3 and 2, next_before 2
        let (page2, next2) = list(&conn, 2, next1).unwrap();
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].action, "action_3");
        assert_eq!(page2[1].action, "action_2");
        assert_eq!(next2, Some(page2[1].id));

        // Page 3: limit 2, before 2 -> entry 1 and migration 4's seed row (the oldest), next_before None
        let (page3, next3) = list(&conn, 2, next2).unwrap();
        assert_eq!(page3.len(), 2);
        assert_eq!(page3[0].action, "action_1");
        assert_eq!(page3[1].action, "account_created");
        assert_eq!(next3, None);
    }
}
