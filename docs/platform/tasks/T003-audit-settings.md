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
