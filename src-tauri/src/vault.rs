//! Credential vault: secure storage for broker API credentials on the OS keychain.
//!
//! # Purpose & Architecture
//! This module provides the [`SecretStore`] trait and implementations to keep broker
//! API keys, secrets, and private keys strictly separated from databases, application logs,
//! error messages, plain configuration files, and the browser API.
//!
//! ZoneBoard adheres to Decision D2 (`docs/platform/ROADMAP.md`):
//! - **Windows:** Uses the OS keychain via Windows Credential Manager ([`KeychainStore`]),
//!   leveraging the `keyring` crate with native Windows backend (`windows-native`).
//! - **Tests:** Uses an in-memory store ([`MemoryStore`]) for hermetic test execution without
//!   touching the OS keychain.
//!
//! # Honest Security Model & Known Boundaries
//! - **Same-user access:** Windows Credential Manager entries are accessible to any process
//!   executing under the same Windows user session. The vault guarantees secrets never leak into
//!   the database, logs, commit history, crash dumps, or HTTP API responses. It does not
//!   defend against same-user malware.
//! - **Zeroization boundaries:** Secrets stored in ZoneBoard's memory are wrapped in
//!   [`secrecy::SecretString`], which zeroizes its memory buffer on drop. However, the underlying
//!   `keyring` crate and Windows Credential Manager API create un-zeroized temporary `String`
//!   and UTF-16 copies during OS communication. Zeroization guarantees apply to ZoneBoard's
//!   in-memory representations, not to external crate internals or the Windows credential subsystem.
//!
//! # Serialization Proof
//! Secrets must never implement [`serde::Serialize`]. The following doctest verifies this invariant:
//!
//! ```compile_fail
//! use secrecy::SecretString;
//! use serde::Serialize;
//!
//! #[derive(Serialize)]
//! struct LeakedPayload {
//!     secret: SecretString,
//! }
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

pub use secrecy::{ExposeSecret, SecretString};

/// Maximum secret size permitted by Windows Credential Manager (2560 UTF-16 bytes = 1280 UTF-16 code units).
pub const MAX_SECRET_BYTES: usize = 2560;

/// Errors returned by [`SecretStore`] operations.
///
/// Guaranteed never to format or leak the secret value in [`fmt::Display`] or [`fmt::Debug`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    /// The specified secret entry does not exist in the store.
    NotFound { account_id: String, name: String },
    /// The secret exceeds the maximum capacity of the credential store (in UTF-16 bytes).
    SecretTooLarge { size: usize, limit: usize },
    /// An identifier (`account_id` or `name`) violated validation rules.
    InvalidIdentifier {
        field: &'static str,
        reason: &'static str,
    },
    /// An error occurred in the underlying credential store.
    Store(String),
    /// The requested operation or platform is not supported.
    Unsupported,
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::NotFound { account_id, name } => {
                write!(f, "secret '{name}' not found for account '{account_id}'")
            }
            VaultError::SecretTooLarge { size, limit } => {
                write!(
                    f,
                    "secret size ({size} UTF-16 bytes) exceeds maximum limit of {limit} UTF-16 bytes"
                )
            }
            VaultError::InvalidIdentifier { field, reason } => {
                write!(f, "invalid identifier for {field}: {reason}")
            }
            VaultError::Store(msg) => {
                write!(f, "credential vault store error: {msg}")
            }
            VaultError::Unsupported => {
                write!(f, "OS keychain is unsupported on this platform")
            }
        }
    }
}

impl std::error::Error for VaultError {}

/// Common trait for secure credential storage.
pub trait SecretStore: Send + Sync {
    /// Stores or overwrites a secret for the given `account_id` and `name`.
    fn put(&self, account_id: &str, name: &str, secret: SecretString) -> Result<(), VaultError>;

    /// Retrieves a secret for the given `account_id` and `name`.
    fn get(&self, account_id: &str, name: &str) -> Result<SecretString, VaultError>;

    /// Deletes a secret. Returns `Err(VaultError::NotFound)` if the entry does not exist.
    fn delete(&self, account_id: &str, name: &str) -> Result<(), VaultError>;

    /// Checks if a secret exists without returning its value.
    fn exists(&self, account_id: &str, name: &str) -> Result<bool, VaultError>;

    /// Deletes a secret if it exists, ignoring `VaultError::NotFound`.
    ///
    /// Useful for account teardown and idempotent cleanup.
    fn delete_if_exists(&self, account_id: &str, name: &str) -> Result<(), VaultError> {
        match self.delete(account_id, name) {
            Ok(()) => Ok(()),
            Err(VaultError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Shared identifier validation ensuring safe composite keyring keys (`<account_id>/<name>`).
fn validate_identifier(field: &'static str, id: &str) -> Result<(), VaultError> {
    if id.is_empty() {
        return Err(VaultError::InvalidIdentifier {
            field,
            reason: "must not be empty",
        });
    }
    if id.chars().count() > 128 {
        return Err(VaultError::InvalidIdentifier {
            field,
            reason: "exceeds maximum length of 128 characters",
        });
    }
    if id.contains('/') {
        return Err(VaultError::InvalidIdentifier {
            field,
            reason: "must not contain forward slash '/'",
        });
    }
    if id.chars().any(|c| c.is_control()) {
        return Err(VaultError::InvalidIdentifier {
            field,
            reason: "must not contain control characters",
        });
    }
    Ok(())
}

fn validate_identifiers(account_id: &str, name: &str) -> Result<(), VaultError> {
    validate_identifier("account_id", account_id)?;
    validate_identifier("name", name)?;
    Ok(())
}

fn check_secret_size(secret: &SecretString) -> Result<(), VaultError> {
    let size = secret.expose_secret().encode_utf16().count() * 2;
    if size > MAX_SECRET_BYTES {
        return Err(VaultError::SecretTooLarge {
            size,
            limit: MAX_SECRET_BYTES,
        });
    }
    Ok(())
}

fn entry_user(account_id: &str, name: &str) -> String {
    format!("{account_id}/{name}")
}

/// Formats a key hint showing only the last 4 characters, prefixed with an ellipsis (`…`).
///
/// This is the only representation of an API key the HTTP API may expose.
/// Shows the last 4 characters only when the value has at least 8 characters; for anything
/// shorter (including empty) returns just `…`.
pub fn key_hint(api_key: &str) -> String {
    let count = api_key.chars().count();
    if count < 8 {
        "…".to_string()
    } else {
        let suffix: String = api_key.chars().skip(count - 4).collect();
        format!("…{suffix}")
    }
}

/// Convenience helper to format a key hint directly from a [`SecretString`].
pub fn key_hint_secret(secret: &SecretString) -> String {
    key_hint(secret.expose_secret())
}

/// In-memory implementation of [`SecretStore`] for unit and hermetic integration tests.
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, SecretString>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

impl SecretStore for MemoryStore {
    fn put(&self, account_id: &str, name: &str, secret: SecretString) -> Result<(), VaultError> {
        validate_identifiers(account_id, name)?;
        check_secret_size(&secret)?;
        let key = entry_user(account_id, name);
        let mut lock = self
            .entries
            .lock()
            .map_err(|_| VaultError::Store("memory lock poisoned".to_string()))?;
        lock.insert(key, secret);
        Ok(())
    }

    fn get(&self, account_id: &str, name: &str) -> Result<SecretString, VaultError> {
        validate_identifiers(account_id, name)?;
        let key = entry_user(account_id, name);
        let lock = self
            .entries
            .lock()
            .map_err(|_| VaultError::Store("memory lock poisoned".to_string()))?;
        match lock.get(&key) {
            Some(s) => Ok(SecretString::from(s.expose_secret().to_string())),
            None => Err(VaultError::NotFound {
                account_id: account_id.to_string(),
                name: name.to_string(),
            }),
        }
    }

    fn delete(&self, account_id: &str, name: &str) -> Result<(), VaultError> {
        validate_identifiers(account_id, name)?;
        let key = entry_user(account_id, name);
        let mut lock = self
            .entries
            .lock()
            .map_err(|_| VaultError::Store("memory lock poisoned".to_string()))?;
        if lock.remove(&key).is_some() {
            Ok(())
        } else {
            Err(VaultError::NotFound {
                account_id: account_id.to_string(),
                name: name.to_string(),
            })
        }
    }

    fn exists(&self, account_id: &str, name: &str) -> Result<bool, VaultError> {
        validate_identifiers(account_id, name)?;
        let key = entry_user(account_id, name);
        let lock = self
            .entries
            .lock()
            .map_err(|_| VaultError::Store("memory lock poisoned".to_string()))?;
        Ok(lock.contains_key(&key))
    }
}

/// OS keychain implementation of [`SecretStore`] backed by Windows Credential Manager.
pub struct KeychainStore {
    service: String,
}

impl KeychainStore {
    /// Default service name (`zoneboard`).
    pub const DEFAULT_SERVICE: &'static str = "zoneboard";

    /// Creates a store with the default service name `zoneboard`.
    pub fn new() -> Self {
        Self {
            service: Self::DEFAULT_SERVICE.to_string(),
        }
    }

    /// Creates a store with a custom service name (used for isolated test suites).
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }
}

impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
fn map_keyring_error(err: keyring::Error, secret: &SecretString) -> VaultError {
    match err {
        keyring::Error::TooLong(_attr, limit) => VaultError::SecretTooLarge {
            size: secret.expose_secret().encode_utf16().count() * 2,
            limit: limit as usize,
        },
        other => VaultError::Store(other.to_string()),
    }
}

#[cfg(windows)]
fn map_keyring_err_no_secret(err: keyring::Error, account_id: &str, name: &str) -> VaultError {
    match err {
        keyring::Error::NoEntry => VaultError::NotFound {
            account_id: account_id.to_string(),
            name: name.to_string(),
        },
        other => VaultError::Store(other.to_string()),
    }
}

#[cfg(windows)]
impl SecretStore for KeychainStore {
    fn put(&self, account_id: &str, name: &str, secret: SecretString) -> Result<(), VaultError> {
        validate_identifiers(account_id, name)?;
        check_secret_size(&secret)?;
        let user = entry_user(account_id, name);
        let entry = keyring::Entry::new(&self.service, &user)
            .map_err(|e| map_keyring_error(e, &secret))?;
        entry
            .set_password(secret.expose_secret())
            .map_err(|e| map_keyring_error(e, &secret))?;
        Ok(())
    }

    fn get(&self, account_id: &str, name: &str) -> Result<SecretString, VaultError> {
        validate_identifiers(account_id, name)?;
        let user = entry_user(account_id, name);
        let entry = keyring::Entry::new(&self.service, &user)
            .map_err(|e| map_keyring_err_no_secret(e, account_id, name))?;
        match entry.get_password() {
            Ok(pwd) => Ok(SecretString::from(pwd)),
            Err(keyring::Error::NoEntry) => Err(VaultError::NotFound {
                account_id: account_id.to_string(),
                name: name.to_string(),
            }),
            Err(e) => Err(map_keyring_err_no_secret(e, account_id, name)),
        }
    }

    fn delete(&self, account_id: &str, name: &str) -> Result<(), VaultError> {
        validate_identifiers(account_id, name)?;
        let user = entry_user(account_id, name);
        let entry = keyring::Entry::new(&self.service, &user)
            .map_err(|e| map_keyring_err_no_secret(e, account_id, name))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Err(VaultError::NotFound {
                account_id: account_id.to_string(),
                name: name.to_string(),
            }),
            Err(e) => Err(map_keyring_err_no_secret(e, account_id, name)),
        }
    }

    fn exists(&self, account_id: &str, name: &str) -> Result<bool, VaultError> {
        validate_identifiers(account_id, name)?;
        let user = entry_user(account_id, name);
        let entry = keyring::Entry::new(&self.service, &user)
            .map_err(|e| map_keyring_err_no_secret(e, account_id, name))?;
        match entry.get_password() {
            Ok(pwd) => {
                drop(pwd);
                Ok(true)
            }
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(map_keyring_err_no_secret(e, account_id, name)),
        }
    }
}

#[cfg(not(windows))]
impl SecretStore for KeychainStore {
    fn put(&self, _account_id: &str, _name: &str, _secret: SecretString) -> Result<(), VaultError> {
        Err(VaultError::Unsupported)
    }

    fn get(&self, _account_id: &str, _name: &str) -> Result<SecretString, VaultError> {
        Err(VaultError::Unsupported)
    }

    fn delete(&self, _account_id: &str, _name: &str) -> Result<(), VaultError> {
        Err(VaultError::Unsupported)
    }

    fn exists(&self, _account_id: &str, _name: &str) -> Result<bool, VaultError> {
        Err(VaultError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_memory_store_lifecycle() {
        let store = MemoryStore::new();
        let acc = "acc_demo";
        let key = "api_secret";

        // Missing initially
        assert_eq!(store.exists(acc, key).unwrap(), false);
        let err = store.get(acc, key).unwrap_err();
        assert_eq!(
            err,
            VaultError::NotFound {
                account_id: acc.to_string(),
                name: key.to_string()
            }
        );

        // Put and get
        store
            .put(acc, key, SecretString::from("secret_alpha_123"))
            .unwrap();
        assert_eq!(store.exists(acc, key).unwrap(), true);
        let retrieved = store.get(acc, key).unwrap();
        assert_eq!(retrieved.expose_secret(), "secret_alpha_123");

        // Overwrite
        store
            .put(acc, key, SecretString::from("secret_beta_456"))
            .unwrap();
        let updated = store.get(acc, key).unwrap();
        assert_eq!(updated.expose_secret(), "secret_beta_456");

        // Delete
        store.delete(acc, key).unwrap();
        assert_eq!(store.exists(acc, key).unwrap(), false);
        let err = store.get(acc, key).unwrap_err();
        assert_eq!(
            err,
            VaultError::NotFound {
                account_id: acc.to_string(),
                name: key.to_string()
            }
        );

        // Delete missing returns NotFound
        let del_err = store.delete(acc, key).unwrap_err();
        assert_eq!(
            del_err,
            VaultError::NotFound {
                account_id: acc.to_string(),
                name: key.to_string()
            }
        );

        // delete_if_exists ignores NotFound
        assert!(store.delete_if_exists(acc, key).is_ok());
    }

    #[test]
    fn test_memory_store_size_boundaries() {
        let store = MemoryStore::new();
        let acc = "acc_test";

        // Boundary: 1280 ASCII chars fits (2560 UTF-16 bytes)
        let exact_1280 = "a".repeat(1280);
        assert!(store
            .put(acc, "exact_1280", SecretString::from(exact_1280.clone()))
            .is_ok());
        let read = store.get(acc, "exact_1280").unwrap();
        assert_eq!(read.expose_secret(), exact_1280);

        // Boundary: 1281 ASCII chars fails (2562 UTF-16 bytes)
        let over_1281 = "a".repeat(1281);
        let err = store
            .put(acc, "over_1281", SecretString::from(over_1281))
            .unwrap_err();
        assert_eq!(
            err,
            VaultError::SecretTooLarge {
                size: 2562,
                limit: 2560,
            }
        );

        // Ed25519 PEM fits (about 120 chars)
        let ed25519_pem = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEINTuctv5v1GQ3Y+WLRZSmEcBHJnHdJJZ3wh8NV352fKG\n-----END PRIVATE KEY-----";
        assert!(store
            .put(acc, "ed25519", SecretString::from(ed25519_pem))
            .is_ok());
        let pem_read = store.get(acc, "ed25519").unwrap();
        assert_eq!(pem_read.expose_secret(), ed25519_pem);
    }

    #[test]
    fn test_identifier_validation() {
        let store = MemoryStore::new();
        let secret = SecretString::from("dummy");

        // Empty identifiers
        assert_eq!(
            store.put("", "name", secret.clone()).unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "account_id",
                reason: "must not be empty"
            }
        );
        assert_eq!(
            store.put("acc", "", secret.clone()).unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "name",
                reason: "must not be empty"
            }
        );

        // Forward slash '/' rejected
        assert_eq!(
            store.put("acc/1", "name", secret.clone()).unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "account_id",
                reason: "must not contain forward slash '/'"
            }
        );
        assert_eq!(
            store.put("acc", "sub/name", secret.clone()).unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "name",
                reason: "must not contain forward slash '/'"
            }
        );

        // Control characters rejected (including NUL)
        assert_eq!(
            store.put("acc\0id", "name", secret.clone()).unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "account_id",
                reason: "must not contain control characters"
            }
        );
        assert_eq!(
            store.put("acc", "name\n", secret.clone()).unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "name",
                reason: "must not contain control characters"
            }
        );

        // Length limit: 128 chars ok, 129 chars rejected
        let id_128 = "x".repeat(128);
        let id_129 = "x".repeat(129);
        assert!(store
            .put(&id_128, "name", SecretString::from("val"))
            .is_ok());
        assert_eq!(
            store
                .put(&id_129, "name", SecretString::from("val"))
                .unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "account_id",
                reason: "exceeds maximum length of 128 characters"
            }
        );
        assert_eq!(
            store
                .put("acc", &id_129, SecretString::from("val"))
                .unwrap_err(),
            VaultError::InvalidIdentifier {
                field: "name",
                reason: "exceeds maximum length of 128 characters"
            }
        );
    }

    #[test]
    fn test_key_hint_formatting() {
        assert_eq!(key_hint("vmPUf294abcd"), "…abcd");
        assert_eq!(key_hint("12345678"), "…5678");
        assert_eq!(key_hint("1234567"), "…");
        assert_eq!(key_hint("1234"), "…");
        assert_eq!(key_hint("12"), "…");
        assert_eq!(key_hint(""), "…");
        assert_eq!(key_hint("🦀🌟🚀🔑🎉🔥✨🎯"), "…🎉🔥✨🎯");

        let sec = SecretString::from("very_secret_9999");
        assert_eq!(key_hint_secret(&sec), "…9999");
    }

    #[test]
    fn test_debug_and_error_never_expose_secret() {
        let raw = "super-secret-cleartext-credential-987654321";
        let secret = SecretString::from(raw);

        // Debug formatting secrecy::SecretString
        let debug_repr = format!("{:?}", secret);
        assert!(!debug_repr.contains(raw));
        assert!(debug_repr.contains("[REDACTED]"));

        // VaultError Display and Debug formatting
        let err = VaultError::SecretTooLarge {
            size: 2600,
            limit: 2560,
        };
        let err_disp = format!("{err}");
        let err_debug = format!("{err:?}");
        assert!(!err_disp.contains(raw));
        assert!(!err_debug.contains(raw));
        assert!(err_disp.contains("2600"));
        assert!(err_disp.contains("2560"));

        let not_found = VaultError::NotFound {
            account_id: "demo_acc".to_string(),
            name: "api_key".to_string(),
        };
        let nf_disp = format!("{not_found}");
        assert!(!nf_disp.contains(raw));
        assert!(nf_disp.contains("demo_acc"));
        assert!(nf_disp.contains("api_key"));
    }

    #[derive(Clone)]
    struct VaultTestBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for VaultTestBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for VaultTestBuffer {
        type Writer = VaultTestBuffer;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn test_logging_does_not_leak_secrets() {
        let raw_secret = "raw_secret_value_do_not_log_123456";
        let secret = SecretString::from(raw_secret);

        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = VaultTestBuffer(buf.clone());

        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .without_time()
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(secret_val = ?secret, "Logging vault diagnostic message");
        });

        let output = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            !output.contains(raw_secret),
            "Log output leaked raw secret!"
        );
        assert!(
            output.contains("[REDACTED]"),
            "Expected [REDACTED] in log output: {output}"
        );
    }
}

#[cfg(test)]
#[cfg(windows)]
mod keychain_integration_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct KeychainTestGuard {
        service: String,
        entries: Vec<(String, String)>,
    }

    impl KeychainTestGuard {
        fn new(service: String) -> Self {
            Self {
                service,
                entries: Vec::new(),
            }
        }

        fn register(&mut self, account_id: &str, name: &str) {
            self.entries.push((account_id.to_string(), name.to_string()));
        }
    }

    impl Drop for KeychainTestGuard {
        fn drop(&mut self) {
            let store = KeychainStore::with_service(&self.service);
            for (account_id, name) in &self.entries {
                let _ = store.delete(account_id, name);
            }
        }
    }

    #[test]
    #[ignore]
    fn test_os_keychain_real_store_lifecycle_and_boundaries() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let service = format!("zoneboard-test-{nanos}");
        let mut guard = KeychainTestGuard::new(service.clone());

        // 1. Write via store1
        let store1 = KeychainStore::with_service(&service);
        let account_id = "test_acc";
        let key_name = "api_key";
        guard.register(account_id, key_name);

        let secret_val = "secret_payload_from_keychain_test";
        store1
            .put(account_id, key_name, SecretString::from(secret_val))
            .expect("store1 put should succeed");
        drop(store1); // Drop store1 to verify persistence across store instances

        // 2. Read back through brand-new store2
        let store2 = KeychainStore::with_service(&service);
        assert!(
            store2.exists(account_id, key_name).expect("exists check"),
            "entry should exist in OS keychain"
        );
        let retrieved = store2
            .get(account_id, key_name)
            .expect("store2 get should succeed");
        assert_eq!(retrieved.expose_secret(), secret_val);

        // 3. Overwrite
        let secret_val_v2 = "secret_payload_v2";
        store2
            .put(account_id, key_name, SecretString::from(secret_val_v2))
            .expect("overwrite should succeed");
        let retrieved_v2 = store2
            .get(account_id, key_name)
            .expect("store2 get v2 should succeed");
        assert_eq!(retrieved_v2.expose_secret(), secret_val_v2);

        // 4. Ed25519 PEM fits (about 120 chars)
        let ed25519_name = "ed25519_key";
        guard.register(account_id, ed25519_name);
        let ed25519_pem = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEINTuctv5v1GQ3Y+WLRZSmEcBHJnHdJJZ3wh8NV352fKG\n-----END PRIVATE KEY-----";
        store2
            .put(account_id, ed25519_name, SecretString::from(ed25519_pem))
            .expect("Ed25519 PEM should fit");
        let ed25519_read = store2
            .get(account_id, ed25519_name)
            .expect("read PEM should succeed");
        assert_eq!(ed25519_read.expose_secret(), ed25519_pem);

        // 5. Boundary: 1280 ASCII chars fits in real store
        let boundary_1280_name = "bound_1280";
        guard.register(account_id, boundary_1280_name);
        let payload_1280 = "x".repeat(1280);
        store2
            .put(
                account_id,
                boundary_1280_name,
                SecretString::from(payload_1280.clone()),
            )
            .expect("1280 chars must fit");
        let read_1280 = store2
            .get(account_id, boundary_1280_name)
            .expect("read 1280");
        assert_eq!(read_1280.expose_secret(), payload_1280);

        // 6. Boundary: 1281 ASCII chars fails in real store (rejected by check_secret_size before keyring is called; step 5 proves our limit is not set too high)
        let payload_1281 = "x".repeat(1281);
        let err = store2
            .put(account_id, "bound_1281", SecretString::from(payload_1281))
            .expect_err("1281 chars must exceed limit");
        assert_eq!(
            err,
            VaultError::SecretTooLarge {
                size: 2562,
                limit: 2560,
            }
        );

        // 7. Delete
        store2
            .delete(account_id, key_name)
            .expect("delete should succeed");
        assert!(
            !store2.exists(account_id, key_name).expect("should not exist"),
            "deleted entry should not exist"
        );
        let not_found_err = store2
            .get(account_id, key_name)
            .expect_err("should be not found");
        assert_eq!(
            not_found_err,
            VaultError::NotFound {
                account_id: account_id.to_string(),
                name: key_name.to_string(),
            }
        );
        let del_not_found = store2
            .delete(account_id, key_name)
            .expect_err("second delete should return NotFound");
        assert_eq!(
            del_not_found,
            VaultError::NotFound {
                account_id: account_id.to_string(),
                name: key_name.to_string(),
            }
        );
        store2
            .delete_if_exists(account_id, key_name)
            .expect("delete_if_exists should ignore NotFound");
    }
}
