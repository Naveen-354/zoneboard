# T001: Foundation (migrations, logging, money type)

Phase 0 · Depends: the baseline commit · Touches: `db.rs` (new), `money.rs` (new), `logging.rs` (new), `journal.rs`, `server.rs`, `main.rs`, `bin/zoneboard-server.rs`, `Cargo.toml`

## Goal
Make the data layer safe to extend: versioned migrations, structured logs that cannot leak secrets, and a decimal type for money in new code. Nothing a user sees should change.

## In scope
1. **Migrations (`db.rs`).** A runner using `PRAGMA user_version`. Each migration runs in a transaction. Migration 1 = the schema as it is created today (`trades` with `market` and `mode`, `positions`). Migration 2 = the one-off relabel of old "Paper trade" rows from `live` to `demo` (today it re-runs on every start inside `journal.rs`). Move all ad-hoc `ALTER TABLE`/`UPDATE` out of `Journal::open`. A database with a higher `user_version` than the app knows must be **refused** with a clear error (never downgraded or modified).
2. **Legacy databases** created by earlier builds (no `market`; `market` but no `mode`; with `mode` and old "Paper trade" rows; also a database that has no `user_version` set at all) must migrate to the same final state as today. Keep the existing migration tests passing and add one fixture per legacy shape.
3. **Data directory.** One function resolving the app's data folder (`ZONEBOARD_DATA`, else `%APPDATA%\ZoneBoard`). Keep today's journal resolution order (`JOURNAL_DB`, then `./journal.db` if present, then the data folder) but **log which file was chosen**. Do not move any existing file.
4. **Logging (`logging.rs`).** `tracing` with a daily file in the data folder plus stderr. A redaction layer that masks values whose field names look secret (`api_key`, `secret`, `token`, `password`, `authorization`, `signature`) and anything wrapped in `secrecy::Secret`. Server startup logs the port, journal path and version, nothing else sensitive.
5. **Money (`money.rs`).** A thin wrapper over `rust_decimal`: parse from JSON number or string, round half-even to a given scale, `Display`, serde as a string. Documented rules for converting from the legacy `f64` columns. **Do not convert existing columns yet.**

## Out of scope
New tables, converting current columns, any UI change, API changes.

## Acceptance criteria
- All existing tests pass unchanged (33 Rust, 25 Python); zero compiler warnings.
- New tests: each legacy fixture migrates; refusing a newer `user_version`; migrations are idempotent; redaction masks secret-named fields and `Secret` values in log output (assert on captured output); money parsing/rounding table tests including values like `0.1 + 0.2`.
- Starting the server creates the log file and prints the resolved journal path.

## Notes
New dependencies allowed (pre-approved in ROADMAP D8): `rust_decimal`, `tracing`, `tracing-subscriber`, `secrecy`. Anything else: stop and ask.
