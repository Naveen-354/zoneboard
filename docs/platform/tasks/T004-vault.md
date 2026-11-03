# T004: Credential vault

Phase 1 · Depends: T001 · Touches: `vault.rs` (new), `Cargo.toml`

## Goal
Store broker API credentials where the database, logs and API can never expose them. Decision D2: Windows Credential Manager (OS keychain). A master-password encrypted file is **not** part of this task.

## In scope
1. A `SecretStore` trait: `put(account_id, name, secret)`, `get(account_id, name) -> Secret`, `delete`, `exists`. Secrets are `secrecy::SecretString` (or bytes) and zeroized on drop.
2. `KeychainStore` on the `keyring` crate. Service name `zoneboard`, entry user `<account_id>/<name>`. Surface Windows Credential Manager's size limit as a clear error (an Ed25519 private key in PEM form must fit; test it).
3. `MemoryStore` for tests.
4. Secrets must never be `Debug`-printable, `Serialize`-able or `Display`-able as their value. Add tests that format them with `{:?}` and serialize attempts do not compile or print only `[REDACTED]`.
5. Helper `key_hint(api_key) -> "…abcd"` (last 4 characters) as the only thing the API will ever show about a key.

## Out of scope
HTTP routes (they arrive with T007), a master-password file, key rotation.

## Acceptance criteria
- Round-trip, overwrite, delete and missing-key tests on `MemoryStore`.
- A keychain test marked `#[ignore]` that uses a unique throwaway service name and cleans up; the owner runs it once by hand (`cargo test -- --ignored`).
- No secret value appears in any `Debug`, log or error text (assert).

## Notes
Pre-approved dependencies: `keyring`, `secrecy`, `zeroize`. The `keyring` crate must work on `windows-gnu`; if it does not, stop and report instead of switching approach silently.
