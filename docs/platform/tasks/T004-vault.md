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

## Plan review: corrections the implementation must follow (Claude, 2026-11-03)
Antigravity's plan was reviewed and approved **with these changes**. Where it differs from the plan, this section wins. Items 1 and 2 are defects in the plan, checked against crates.io and the `keyring` 3.6.3 source.

1. **The dependency line must enable the Windows backend.** `keyring = "3"` enables **no** backend (3.6.3 has no default features; `windows-native` is opt-in), so the crate silently falls back to its in-memory mock store and `put` would "succeed" and vanish on restart. Use `keyring = { version = "3.6", default-features = false, features = ["windows-native"] }` under `[target.'cfg(windows)'.dependencies]`. Do not use `keyring` 4.x (restructured into separate store crates, not reviewed). On non-Windows, `KeychainStore` operations return `VaultError::Unsupported`; they must never reach the mock store.
2. **The size limit is in UTF-16 bytes, not UTF-8 bytes.** `keyring` 3.6.3 (`windows.rs`) stores `set_password` as UTF-16 and rejects `password.encode_utf16().count() * 2 > 2560`, so the ceiling is 1280 UTF-16 code units. Check `expose_secret().encode_utf16().count() * 2 <= 2560`, return `SecretTooLarge { size, limit }` in those units, and also map `keyring`'s own `TooLong` error to it. Boundary tests: 1280 ASCII characters fits, 1281 does not; an Ed25519 PEM (about 120 characters) fits.
3. **Hand-written errors.** `thiserror` is not pre-approved and the plan's `#[derive(Debug, thiserror::Error if used ...)]` is not valid Rust. Write `Display` and `Error` by hand. `VaultError` never contains the secret.
4. **One shared identifier check** used by both stores (so the tests exercise what production runs): `account_id` and `name` are non-empty, at most 128 characters, contain no `/` and no control characters (including NUL). Reason: the entry user is `<account_id>/<name>`, so `a/b` + `c` and `a` + `b/c` would collide.
5. **`delete` of a missing entry returns `Err(NotFound)`** (strict, deterministic). Add a provided trait method `delete_if_exists` that ignores `NotFound`, for account teardown. This resolves the plan's contradictory wording.
6. **Serialization proof:** one `compile_fail` doctest showing `SecretString` does not implement `serde::Serialize`, plus a `{:?}` test asserting `[REDACTED]`. Do not add `static_assertions` or any other crate.
7. **Log-leak test:** `TestBuffer` in `logging.rs` is private to that module; give the vault test its own tiny writer and do not change `logging.rs`.
8. **Honest module docs:** any process running as this Windows user can read Credential Manager entries (the vault keeps secrets out of the database, logs, API and plain files; it does not defend against same-user malware), and `keyring` makes un-zeroized `String` / UTF-16 copies internally (zeroizing applies to our `SecretString`, not to the crate's internals).
9. **The ignored keychain test:** unique service `zoneboard-test-<nanos>`, a drop guard that deletes its entries even when an assertion fails, and a read-back through a **brand-new** `KeychainStore` (a mock store would not survive that, so it proves the real OS store was used), plus the 1280 / 1281 boundary against the real store. **You must not run it** (`--ignored` is for the owner, by hand); say in your report that you did not.
10. `exists` reads via `get_password`, maps `NoEntry` to `false`, and drops the value immediately.
