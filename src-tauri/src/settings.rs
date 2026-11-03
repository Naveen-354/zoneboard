//! Typed user settings store backed by SQLite with transactional change auditing.
//!
//! # Architecture & Security Rules
//! - All settings modifications go through [`update_settings`] inside an explicit SQLite transaction.
//! - Any change writes an audit record into `audit_log` atomically alongside the settings change.
//! - If validation or insertion fails, the entire transaction rolls back (all-or-nothing).
//!
//! > [!IMPORTANT]
//! > **live_armed is not a stored setting.** It will be runtime-only and default to disarmed on every start (ROADMAP gate G2).
//!
//! > [!CAUTION]
//! > **No secret-bearing setting may ever be added here:** audit detail redaction works on key names
//! > (`old` / `new`), so a secret value stored under them would not be masked. Credentials belong exclusively in the OS vault.

use std::fmt;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Schema for application-level user preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Canonical IANA timezone name (e.g. "UTC", "Asia/Kolkata").
    pub timezone: String,
    /// Desktop notification flag.
    #[serde(rename = "notifications.desktop")]
    pub notifications_desktop: bool,
    /// Default trading account name, or `null`.
    #[serde(rename = "preferences.default_account")]
    pub preferences_default_account: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timezone: "UTC".to_string(),
            notifications_desktop: false,
            preferences_default_account: None,
        }
    }
}

/// Errors occurring during settings retrieval or update.
#[derive(Debug)]
pub enum SettingsError {
    /// Attempted to store a runtime-only setting (e.g. `live_armed`).
    RuntimeOnlyKey(String),
    /// Schema validation error (e.g. unknown key, invalid timezone, type mismatch).
    Validation(String),
    /// Underlying SQLite engine failure.
    Database(rusqlite::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeOnlyKey(msg) => write!(f, "{msg}"),
            Self::Validation(msg) => write!(f, "{msg}"),
            Self::Database(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// Retrieves the current settings, falling back to defaults for any unconfigured keys.
pub fn get_settings(conn: &Connection) -> Result<Settings, rusqlite::Error> {
    let mut settings = Settings::default();

    let mut stmt = conn.prepare("SELECT key, value_json FROM settings")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;

    for row in rows {
        let (key, val_json) = row?;
        match key.as_str() {
            "timezone" => {
                if let Ok(Value::String(s)) = serde_json::from_str::<Value>(&val_json) {
                    if crate::timezones::is_valid_timezone(&s) {
                        settings.timezone = s;
                    }
                }
            }
            "notifications.desktop" => {
                if let Ok(Value::Bool(b)) = serde_json::from_str::<Value>(&val_json) {
                    settings.notifications_desktop = b;
                }
            }
            "preferences.default_account" => {
                if let Ok(val) = serde_json::from_str::<Value>(&val_json) {
                    match val {
                        Value::String(s) => settings.preferences_default_account = Some(s),
                        Value::Null => settings.preferences_default_account = None,
                        _ => {}
                    }
                }
            }
            _ => {
                // Unknown stored key; ignored on read
            }
        }
    }

    Ok(settings)
}

/// Validates and atomically applies updates to settings.
///
/// If any key is invalid or unknown, or if any database operation fails,
/// the transaction is aborted and no settings or audit records are modified (all-or-nothing).
pub fn update_settings(
    conn: &mut Connection,
    updates: &serde_json::Map<String, Value>,
) -> Result<Settings, SettingsError> {
    // 1. Validate all keys and values up front before beginning a transaction
    for (k, v) in updates {
        match k.as_str() {
            "live_armed" => {
                return Err(SettingsError::RuntimeOnlyKey(
                    "live_armed is runtime-only and cannot be persisted in settings".to_string(),
                ));
            }
            "timezone" => {
                let tz = v
                    .as_str()
                    .ok_or_else(|| SettingsError::Validation("timezone must be a string".to_string()))?;
                if !crate::timezones::is_valid_timezone(tz) {
                    return Err(SettingsError::Validation(format!("invalid timezone: '{tz}'")));
                }
            }
            "notifications.desktop" => {
                if !v.is_boolean() {
                    return Err(SettingsError::Validation(
                        "notifications.desktop must be a boolean".to_string(),
                    ));
                }
            }
            "preferences.default_account" => {
                if !v.is_string() && !v.is_null() {
                    return Err(SettingsError::Validation(
                        "preferences.default_account must be a string or null".to_string(),
                    ));
                }
            }
            unknown => {
                return Err(SettingsError::Validation(format!(
                    "unknown setting key: '{unknown}'"
                )));
            }
        }
    }

    // 2. Open an explicit transaction
    let tx = conn.transaction().map_err(SettingsError::Database)?;
    let current = get_settings(&tx).map_err(SettingsError::Database)?;

    let mut old_vals = serde_json::Map::new();
    let mut new_vals = serde_json::Map::new();
    let now_utc = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    if let Some(tz_val) = updates.get("timezone") {
        let new_tz = tz_val.as_str().unwrap();
        if new_tz != current.timezone {
            old_vals.insert("timezone".to_string(), json!(current.timezone));
            new_vals.insert("timezone".to_string(), json!(new_tz));
            tx.execute(
                "INSERT INTO settings (key, value_json, updated_utc) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_utc = excluded.updated_utc",
                params!["timezone", serde_json::to_string(&new_tz).unwrap(), now_utc],
            ).map_err(SettingsError::Database)?;
        }
    }

    if let Some(notif_val) = updates.get("notifications.desktop") {
        let new_notif = notif_val.as_bool().unwrap();
        if new_notif != current.notifications_desktop {
            old_vals.insert("notifications.desktop".to_string(), json!(current.notifications_desktop));
            new_vals.insert("notifications.desktop".to_string(), json!(new_notif));
            tx.execute(
                "INSERT INTO settings (key, value_json, updated_utc) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_utc = excluded.updated_utc",
                params!["notifications.desktop", serde_json::to_string(&new_notif).unwrap(), now_utc],
            ).map_err(SettingsError::Database)?;
        }
    }

    if let Some(acc_val) = updates.get("preferences.default_account") {
        let new_acc = acc_val.as_str().map(|s| s.to_string());
        if new_acc != current.preferences_default_account {
            old_vals.insert("preferences.default_account".to_string(), json!(current.preferences_default_account));
            new_vals.insert("preferences.default_account".to_string(), json!(new_acc));
            tx.execute(
                "INSERT INTO settings (key, value_json, updated_utc) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_utc = excluded.updated_utc",
                params!["preferences.default_account", serde_json::to_string(&new_acc).unwrap(), now_utc],
            ).map_err(SettingsError::Database)?;
        }
    }

    // 3. If any settings changed, write audit record in the same transaction
    if !old_vals.is_empty() {
        let detail = json!({
            "old": old_vals,
            "new": new_vals,
        });
        crate::audit::record(&tx, "user", "settings_update", "settings", &detail)
            .map_err(SettingsError::Database)?;
    }

    // 4. Commit transaction
    tx.commit().map_err(SettingsError::Database)?;

    get_settings(conn).map_err(SettingsError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut conn).unwrap();
        conn
    }

    #[test]
    fn default_settings_match_spec() {
        let conn = setup_test_db();
        let s = get_settings(&conn).unwrap();
        assert_eq!(s.timezone, "UTC");
        assert_eq!(s.notifications_desktop, false);
        assert_eq!(s.preferences_default_account, None);
    }

    #[test]
    fn update_settings_commits_change_and_writes_audit_entry() {
        let mut conn = setup_test_db();
        let mut updates = serde_json::Map::new();
        updates.insert("timezone".to_string(), json!("Asia/Kolkata"));
        updates.insert("notifications.desktop".to_string(), json!(true));

        let updated = update_settings(&mut conn, &updates).unwrap();
        assert_eq!(updated.timezone, "Asia/Kolkata");
        assert_eq!(updated.notifications_desktop, true);
        assert_eq!(updated.preferences_default_account, None);

        // Check audit log
        let (entries, _) = crate::audit::list(&conn, 10, None).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.actor, "user");
        assert_eq!(entry.action, "settings_update");
        assert_eq!(entry.entity, "settings");
        assert_eq!(entry.detail["old"]["timezone"], "UTC");
        assert_eq!(entry.detail["new"]["timezone"], "Asia/Kolkata");
        assert_eq!(entry.detail["old"]["notifications.desktop"], false);
        assert_eq!(entry.detail["new"]["notifications.desktop"], true);
    }

    #[test]
    fn invalid_timezone_is_rejected_and_aborts_all_changes() {
        let mut conn = setup_test_db();
        let mut updates = serde_json::Map::new();
        updates.insert("timezone".to_string(), json!("Mars/Olympus"));

        let res = update_settings(&mut conn, &updates);
        assert!(matches!(res, Err(SettingsError::Validation(_))));

        let s = get_settings(&conn).unwrap();
        assert_eq!(s.timezone, "UTC");

        // No audit row written
        let (entries, _) = crate::audit::list(&conn, 10, None).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn live_armed_is_rejected_as_runtime_only() {
        let mut conn = setup_test_db();
        let mut updates = serde_json::Map::new();
        updates.insert("live_armed".to_string(), json!(true));

        let res = update_settings(&mut conn, &updates);
        assert!(matches!(res, Err(SettingsError::RuntimeOnlyKey(_))));
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("live_armed is runtime-only"), "{err_msg}");

        // No audit row written
        let (entries, _) = crate::audit::list(&conn, 10, None).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn unknown_key_is_rejected_and_aborts_all_changes() {
        let mut conn = setup_test_db();
        let mut updates = serde_json::Map::new();
        updates.insert("timezone".to_string(), json!("Asia/Kolkata"));
        updates.insert("invalid_key_xyz".to_string(), json!("hello"));

        let res = update_settings(&mut conn, &updates);
        assert!(matches!(res, Err(SettingsError::Validation(_))));

        // timezone was NOT updated due to all-or-nothing validation
        let s = get_settings(&conn).unwrap();
        assert_eq!(s.timezone, "UTC");

        // No audit row written
        let (entries, _) = crate::audit::list(&conn, 10, None).unwrap();
        assert_eq!(entries.len(), 0);
    }
}
