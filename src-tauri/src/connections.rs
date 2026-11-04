//! Broker connections (T007): connect a Binance account, verify it, read its real balances, disconnect.
//!
//! * Credentials arrive once in the request body, go straight to the OS vault (`vault.rs`) and are never
//!   stored in SQLite, logged, audited or echoed. The API shows only a key hint (last 4 characters).
//! * A key that can withdraw funds is refused and nothing is stored.
//! * A `live` connection needs `"confirm_live": true`. Agents and tests never contact a live endpoint: tests
//!   point both environments at a local fake server.
//! * Balances are the broker's numbers, cached at most 10 s. On failure the error and the time of the last
//!   success are returned; stale balances are never served as current.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{params, OptionalExtension};
use secrecy::SecretString;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::exchange::binance::BinanceClient;
use crate::exchange::ExchangeError;
use crate::server::AppState;
use crate::vault::key_hint_secret;

const CACHE: Duration = Duration::from_secs(10);
pub const WITHDRAW_MSG: &str = "This key can withdraw funds. Create a key with Reading (and Spot trading if you plan to trade) only, restricted to your IP.";

/// Where each environment's REST API lives. Tests point both at a fake server.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub binance_testnet: String,
    pub binance_live: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self { binance_testnet: crate::exchange::binance::TESTNET_URL.into(), binance_live: crate::exchange::binance::LIVE_URL.into() }
    }
}

#[derive(Default)]
pub struct Cache {
    balances: Mutex<HashMap<String, (Instant, Value)>>,
    /// One client per account, kept so its clock offset and any rate-limit pause outlive a single request.
    clients: std::sync::Mutex<HashMap<String, Arc<BinanceClient>>>,
}

pub enum ConnError {
    Bad(String),
    NotFound,
    Conflict(String),
    Upstream(String),
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn base_for<'a>(s: &'a AppState, env: &str) -> &'a str {
    if env == "live" {
        &s.endpoints.binance_live
    } else {
        &s.endpoints.binance_testnet
    }
}

fn credential(v: Option<&Value>, what: &str) -> Result<SecretString, ConnError> {
    let t = v.and_then(Value::as_str).map(str::trim).unwrap_or("");
    if t.len() < 16 || t.len() > 256 || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
        // the value itself is never echoed
        return Err(ConnError::Bad(format!("{what} must be the 16 to 256 letters and digits Binance gave you")));
    }
    Ok(SecretString::from(t.to_string()))
}

/// The client for a connected account, with its secrets read from the vault.
pub fn client_for(s: &AppState, id: &str) -> Result<(Arc<BinanceClient>, String), ConnError> {
    let row: Option<(String, String, i64)> = s.journal.with_conn(|c| c.query_row("SELECT kind, environment, archived FROM accounts WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()).map_err(|e| ConnError::Upstream(e.to_string()))?;
    let Some((kind, env, archived)) = row else { return Err(ConnError::NotFound) };
    if kind != "binance" || archived != 0 {
        return Err(ConnError::Conflict("not a connected Binance account".into()));
    }
    let key = s.vault.get(id, "api_key").map_err(|e| ConnError::Conflict(format!("credentials unavailable: {e}")))?;
    let secret = s.vault.get(id, "api_secret").map_err(|e| ConnError::Conflict(format!("credentials unavailable: {e}")))?;
    let mut clients = s.conn_cache.clients.lock().unwrap();
    let c = clients.entry(id.to_string()).or_insert_with(|| Arc::new(BinanceClient::new(s.data.client().clone(), base_for(s, &env), &env, key, secret))).clone();
    Ok((c, env))
}

fn upstream(e: ExchangeError) -> ConnError {
    match e {
        ExchangeError::Auth(m) => ConnError::Bad(format!("Binance rejected the key: {m}")),
        other => ConnError::Upstream(other.to_string()),
    }
}

/// `POST /api/accounts` with `kind: binance`.
pub async fn connect(s: &AppState, body: &Value) -> Result<Value, ConnError> {
    let m = body.as_object().ok_or_else(|| ConnError::Bad("request body must be a JSON object".into()))?;
    if let Some(k) = m.keys().find(|k| !["kind", "label", "environment", "api_key", "api_secret", "confirm_live"].contains(&k.as_str())) {
        return Err(ConnError::Bad(format!("unknown field '{k}'")));
    }
    let env = m.get("environment").and_then(Value::as_str).unwrap_or("testnet");
    if !["testnet", "live"].contains(&env) {
        return Err(ConnError::Bad("environment must be testnet or live".into()));
    }
    if env == "live" && m.get("confirm_live").and_then(Value::as_bool) != Some(true) {
        return Err(ConnError::Bad("connecting a live account needs \"confirm_live\": true (a real account: use a read-only key, restricted to your IP)".into()));
    }
    let label = m.get("label").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty()).unwrap_or(if env == "live" { "Binance" } else { "Binance Testnet" }).chars().take(60).collect::<String>();
    let key = credential(m.get("api_key"), "api_key")?;
    let secret = credential(m.get("api_secret"), "api_secret")?;
    let hint = key_hint_secret(&key);
    let client = BinanceClient::new(s.data.client().clone(), base_for(s, env), env, key.clone(), secret.clone());
    client.sync_time().await.map_err(upstream)?;
    let acct = client.account().await.map_err(upstream)?;
    if env == "live" && client.withdrawals_enabled().await.map_err(upstream)? {
        let _ = s.journal.with_conn(|c| crate::audit::record(c, "user", "connection_refused", "system", &json!({ "provider": "binance", "environment": env, "key_hint": hint, "reason": "withdrawals enabled" })));
        return Err(ConnError::Bad(WITHDRAW_MSG.into()));
    }
    // account row first, then the vault; a vault failure removes the row again
    let id = s
        .journal
        .with_conn(|c| -> rusqlite::Result<String> {
            let tx = c.transaction()?;
            let prefix = if env == "live" { "bn-live" } else { "bn-test" };
            let id = (1..=100).map(|n| format!("{prefix}-{n}")).find(|id| tx.query_row("SELECT 1 FROM accounts WHERE id = ?1", [id], |_| Ok(())).optional().ok().flatten().is_none()).unwrap_or_else(|| format!("{prefix}-x"));
            tx.execute(
                "INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived, key_hint, connected_utc, last_sync_utc) VALUES (?1,'binance',?2,?3,'active',?4,0,?5,?4,?4)",
                params![id, label, env, now(), hint],
            )?;
            crate::audit::record(&tx, "user", "account_connected", &format!("account:{id}"), &json!({ "account_id": id, "provider": "binance", "environment": env, "key_hint": hint, "permissions": acct.permissions, "can_trade": acct.can_trade }))?;
            tx.commit()?;
            Ok(id)
        })
        .map_err(|e| ConnError::Upstream(e.to_string()))?;
    let stored = s.vault.put(&id, "api_key", key).and_then(|_| s.vault.put(&id, "api_secret", secret));
    if let Err(e) = stored {
        let _ = s.vault.delete_if_exists(&id, "api_key");
        let _ = s.journal.with_conn(|c| c.execute("DELETE FROM accounts WHERE id = ?1", [&id]));
        return Err(ConnError::Conflict(format!("could not store the credentials in the OS vault: {e}")));
    }
    tracing::info!(account = %id, environment = env, "binance account connected");
    s.journal.with_conn(|c| crate::accounts::account_json(c, &id)).map_err(|e| ConnError::Upstream(format!("{e:?}")))
}

/// `GET /api/accounts/{id}/balances`.
pub async fn balances(s: &AppState, id: &str) -> Result<Value, ConnError> {
    if let Some((t, v)) = s.conn_cache.balances.lock().await.get(id) {
        if t.elapsed() < CACHE {
            return Ok(v.clone());
        }
    }
    let (client, env) = client_for(s, id)?;
    match client.account().await {
        Ok(a) => {
            let t = now();
            let _ = s.journal.with_conn(|c| c.execute("UPDATE accounts SET last_sync_utc = ?1, last_error = NULL WHERE id = ?2", params![t, id]));
            let v = json!({ "account_id": id, "environment": env, "virtual": false, "source": format!("Binance {env} account API"), "as_of": t, "balances": a.balances, "permissions": a.permissions, "can_trade": a.can_trade, "used_weight_1m": client.used_weight(), "capabilities": client.capabilities() });
            s.conn_cache.balances.lock().await.insert(id.to_string(), (Instant::now(), v.clone()));
            Ok(v)
        }
        Err(e) => {
            let msg = e.to_string();
            let last: Option<String> = s.journal.with_conn(|c| {
                let _ = c.execute("UPDATE accounts SET last_error = ?1 WHERE id = ?2", params![msg, id]);
                c.query_row("SELECT last_sync_utc FROM accounts WHERE id = ?1", [id], |r| r.get(0)).optional().ok().flatten().flatten()
            });
            Err(ConnError::Upstream(format!("{msg} (last successful sync: {})", last.unwrap_or_else(|| "never".into()))))
        }
    }
}

/// `POST /api/accounts/{id}/verify`: re-check the key and, for live, that it still cannot withdraw.
pub async fn verify(s: &AppState, id: &str) -> Result<Value, ConnError> {
    let (client, env) = client_for(s, id)?;
    client.sync_time().await.map_err(upstream)?;
    let a = client.account().await.map_err(upstream)?;
    let withdraw = if env == "live" { client.withdrawals_enabled().await.map_err(upstream)? } else { false };
    let t = now();
    let detail = json!({ "account_id": id, "environment": env, "permissions": a.permissions, "can_trade": a.can_trade, "withdrawals_enabled": withdraw });
    s.journal.with_conn(|c| {
        let _ = c.execute("UPDATE accounts SET last_sync_utc = ?1, last_error = ?2 WHERE id = ?3", params![t, withdraw.then_some(WITHDRAW_MSG), id]);
        let _ = crate::audit::record(c, "user", "account_verified", &format!("account:{id}"), &detail);
    });
    if withdraw {
        return Err(ConnError::Conflict(format!("{WITHDRAW_MSG} Disconnect it and connect a safer key.")));
    }
    Ok(json!({ "ok": true, "verified_utc": t, "permissions": a.permissions, "can_trade": a.can_trade, "capabilities": client.capabilities() }))
}

/// `DELETE /api/accounts/{id}`: delete the vault entries, archive the account (its history stays), audit.
pub async fn disconnect(s: &AppState, id: &str) -> Result<Value, ConnError> {
    let kind: Option<String> = s.journal.with_conn(|c| c.query_row("SELECT kind FROM accounts WHERE id = ?1", [id], |r| r.get(0)).optional()).map_err(|e| ConnError::Upstream(e.to_string()))?;
    match kind.as_deref() {
        None => return Err(ConnError::NotFound),
        Some("binance") => {}
        Some(_) => return Err(ConnError::Conflict("only broker connections can be disconnected; demo accounts stay".into())),
    }
    let running: i64 = s.journal.with_conn(|c| c.query_row("SELECT count(*) FROM bots WHERE account_id = ?1 AND status IN ('running','paused')", [id], |r| r.get(0))).unwrap_or(0);
    if running > 0 {
        return Err(ConnError::Conflict("stop the bots that use this account first".into()));
    }
    let _ = s.vault.delete_if_exists(id, "api_key");
    let _ = s.vault.delete_if_exists(id, "api_secret");
    s.conn_cache.balances.lock().await.remove(id);
    s.conn_cache.clients.lock().unwrap().remove(id);
    s.journal
        .with_conn(|c| -> rusqlite::Result<()> {
            c.execute("UPDATE accounts SET archived = 1, status = 'disconnected' WHERE id = ?1", [id])?;
            crate::audit::record(c, "user", "account_disconnected", &format!("account:{id}"), &json!({ "account_id": id }))?;
            Ok(())
        })
        .map_err(|e| ConnError::Upstream(e.to_string()))?;
    Ok(json!({ "ok": true, "account_id": id }))
}
