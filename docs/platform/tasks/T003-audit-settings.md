# T003: Audit log and settings store

Phase 0 · Depends: T001 · Touches: `audit.rs` (new), `settings.rs` (new), `db.rs` (migration 3), `server.rs`

## Goal
An append-only record of everything security- or money-relevant, and a typed settings store for user preferences.

## In scope
1. **Migration 3** adds `audit_log(id, ts_utc, actor, action, entity, detail_json)` and `settings(key PRIMARY KEY, value_json, updated_utc)`. SQLite **triggers forbid UPDATE and DELETE on `audit_log`** (append-only, tested).
2. `audit::record(actor, action, entity, detail)`. `detail` is passed through the T001 redaction (secret-named fields masked) before storage. Actors: `user`, `system`, `strategy:<id>` (later).
3. Record: `app_start`, `settings_update`. Later tasks add their own events through the same function.
4. `GET /api/audit?limit=&before=` (newest first, max 500 per call, cursor by id).
5. `GET /api/settings` and `PUT /api/settings`: a **typed schema** with validation. Initial keys: `timezone` (valid IANA name), `notifications.desktop` (bool), `preferences.default_account` (string or null). Unknown keys -> 400. Every change writes an audit entry with old and new values.
6. **`live_armed` is not a stored setting.** It will be runtime-only and default to disarmed on every start (ROADMAP gate G2). Add a comment saying so; do not implement arming.

## Out of scope
UI (T006), notifications themselves, retention/export.

## Acceptance criteria
- Tests: triggers block UPDATE/DELETE on `audit_log`; redaction in `detail`; invalid timezone rejected; unknown key rejected; audit entry written on change with old/new; pagination cursor correct.
- If T002 has merged, the routes are behind the token; if not, register them in the same place so merging either order is a trivial conflict.

## Plan review: corrections the implementation must follow (Claude, 2026-11-03)
Antigravity's plan was reviewed and approved **with these changes**. Where it differs from the plan, this section wins.

1. **Database access:** approved, but as a closure helper, not a raw mutex: `Journal::with_conn<R>(&self, f: impl FnOnce(&mut Connection) -> R) -> R` (lock inside, `.lock().unwrap()` as the file does today). `settings::update_settings` takes `&mut Connection` and uses `conn.transaction()` so a setting change and its audit row commit together or not at all; `audit::record` takes `&Connection` (a `Transaction` derefs to it). Also note `audit::record` takes a connection where the spec showed none: that is an accepted deviation (needed for transactions), so report it as a deviation, not "none".
2. **Do not type the timezone list by hand.** About 430 names written from memory will be wrong. Generate it: a script `tools/gen_timezones.mjs` (Node) takes `Intl.supportedValuesOf('timeZone')`, adds `UTC`, sorts and de-duplicates, and writes `src-tauri/src/timezones.rs` (`pub static CANONICAL: &[&str]`) with a header comment saying it is generated, by which command, and with which Node version. Commit the generated file. Tests: the list is sorted and unique (binary search depends on it), contains `UTC`, `Asia/Kolkata`, `America/New_York`, `Europe/London`, and rejects `Mars/Olympus`, `EST`, the empty string and `asia/kolkata` (case matters). Do not claim it "exactly matches" any browser: say "canonical names only; aliases such as `Asia/Calcutta` are rejected".
3. **Append-only needs a third trigger.** In SQLite, `INSERT OR REPLACE` / `REPLACE INTO` over an existing `id` deletes the old row **without firing DELETE triggers** (recursive triggers are off by default), so history could be overwritten. Add `audit_log_no_replace`, a `BEFORE INSERT ON audit_log WHEN EXISTS (SELECT 1 FROM audit_log WHERE id = NEW.id)` trigger that raises ABORT, and a test that `INSERT OR REPLACE` over an existing id fails and the original row is unchanged. In the code comment say honestly what this is: it stops accidental and application-level edits; someone with file access can still drop the trigger or table (a hash chain is a later phase).
4. **Query parsing:** follow the existing handlers (`Query<HashMap<String, String>>`) so bad input returns `{"error": ...}` JSON, not axum's plain-text rejection. `limit`: default 50, a value above 500 is **clamped** to 500, `0` or non-numeric -> 400. `before` non-numeric -> 400. `GET /api/audit` returns `{"entries": [...], "next_before": <id or null>}` (`next_before` is the id of the last entry returned when there may be more, otherwise null).
5. **Settings:** flat JSON as planned (`{"timezone": "UTC", "notifications.desktop": false, "preferences.default_account": null}`), default timezone `UTC`. `PUT` is all-or-nothing. `live_armed` (and any unknown key) -> 400 with `{"error": ...}`; the `live_armed` message says it is runtime-only. Add a comment in `settings.rs` that no secret-bearing setting may ever be added: audit detail redaction works on key names (`old` / `new`), so a secret value under them would not be masked.
6. **Migration 3:** `PRAGMA user_version = 3` inside its transaction; add a test that a v2 database with data (reuse an existing fixture) migrates to v3 with its data intact; update any test that hard-codes the version to use `CURRENT_VERSION`.
7. **`app_start`** is written in `AppState::new` after migration; a failure to write is a startup error. Tests build many `AppState`s, so tests that count audit rows must account for the `app_start` row.
8. **Merge order with T002:** register the new routes next to the existing `/api/*` ones. T002 protects by path prefix, so they become protected automatically; whichever task merges second adds `/api/audit` and `/api/settings` to T002's route table test.
