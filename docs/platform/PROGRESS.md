# Progress

Single source of shared context for Claude, Antigravity and the owner. Claude updates it after every review. Read `ROADMAP.md` for the plan and `WORKFLOW.md` for how work is handed off.

**Updated:** 2026-11-03 · **Phase:** 0 (T001 merged as `d0bec79`; Wave 2 plans reviewed, implementation dispatched) · **Baseline commit:** `ed9c249`

## Next

1. Claude: review **T002, T003, T004** as they return (independent checks, mutation checks, secrets grep, behaviour run), send corrections rounds if needed, then merge in the order **T004, T002, T003** (expect trivial conflicts in `Cargo.toml`, `Cargo.lock`, `lib.rs`, `server.rs`). T009 and T010 are not yet dispatched.
   - Owner approved (2026-11-03) commit of the corrected specs and **implementation with the sandbox off**, on the condition that **no real credential exists on this machine**. Verified before dispatch: no `.env`, no `%APPDATA%\ZoneBoard` folder, no ZoneBoard/Binance/FYERS entry in Credential Manager. **That approval does not cover T007 or any task that handles a real key**; revisit before then.
2. Owner: answer decisions **D1 to D3** below (D3 decides whether FYERS can ever place orders). Not needed for Wave 2.
3. Cleanup: the `D:\project\zoneboard-wt\T001` worktree (and its `target\` folder) is no longer needed; remove it when convenient (`git worktree remove`), the branch `task/T001` can stay.

## Decisions

| # | Decision | Status |
|---|---|---|
| D1 | Binance scope | default **spot only**; awaiting owner |
| D2 | Key storage | default **Windows Credential Manager**; awaiting owner |
| D3 | FYERS static IP available? | **unknown, needed** |
| D4 | Python backend | default **freeze** (T009) |
| D5 | Strategies run in the Rust server | decided |
| D6 | Declarative JSON strategies | default; awaiting owner |
| D7 | Decimals for money in new code | decided |
| D8 | Pre-approved dependency batch | default; awaiting owner |

## Tasks

| Task | Status | Branch | Commit | Note |
|---|---|---|---|---|
| T001 foundation | **done, merged** | task/T001 | merge `d0bec79` (work `1c9f872`, `292c4da`) | 50 Rust + 25 Python tests on merged `main`; 2 review rounds; see Log |
| T002 API token | **plan reviewed**, corrections in spec | task/T002 (plan-only worktree, no edits) | | 8 corrections; waits for go-ahead |
| T003 audit and settings | **plan reviewed**, corrections in spec | task/T003 (plan-only worktree, no edits) | | 8 corrections; waits for go-ahead |
| T004 vault | **plan reviewed**, corrections in spec | task/T004 (plan-only worktree, no edits) | | 10 corrections; waits for go-ahead |
| T005 accounts, demo | spec ready | | | after T001 |
| T006 nav and accounts UI | spec ready | | | after T003, T005 |
| T007 Binance read | spec ready | | | after T001, T004, T005 |
| T008 security review | spec ready | | | after T002, T004, T007 |
| T009 freeze Python | spec ready | | | independent |
| T010 UI smoke harness | spec ready | | | independent |
| Phase 2 onward | not specified | | | specified just in time |

## Log

- 2026-11-02: Audit and roadmap written. Antigravity CLI (`agy` v1.4.12) verified: runs non-interactively, loads `AGENTS.md`, plan mode read-only (smoke test, no files changed). `tools/delegate.ps1` written, not yet run against a task.
- 2026-11-02: **Rehearsal of the full pipeline** in a throwaway clone (not this repo): `delegate.ps1 -Task T001 -Mode plan` created worktree `task/T001`, ran `agy` in plan mode for 449 s, produced a plan that matched the spec (files, order, 4 legacy fixtures, acceptance mapping, only pre-approved dependencies, wrote its own daily log writer instead of adding a crate), followed the report format, and changed no files. The rehearsal found and fixed two bugs in `delegate.ps1` (PowerShell 5.1 treats native stderr as fatal, and `'$Task:'` parses as a scope-qualified variable). **Not yet exercised:** `-Mode accept-edits`, and whether `--sandbox` lets `cargo` run; first real implementation run will tell.
- 2026-11-03: Work committed as `ed9c249`. **T001 dispatched to Antigravity in plan mode on this repo** (worktree `D:\project\zoneboard-wt\T001`, log `..\zoneboard-wt\logs\T001-plan-20261103-094541.log`, 373 s, no files changed). Claude reviewed the plan: approved with 6 corrections (`user_version` inside the transaction; SQL-text fixtures; decimal conversion via shortest text; check the `secrecy` 0.10 API and document that message-embedded secrets are not masked; no directory creation in `data_dir()`; single log file handle). **Finding:** `--sandbox` blocks `cargo` on Windows.
- 2026-11-03: **T001 implemented by Antigravity** (sandbox off, owner-approved; log `..\zoneboard-wt\logs\T001-accept-edits-20261103-100434.log`, 1810 s) as `1c9f872`: `db.rs` (versioned migrations on `PRAGMA user_version`, non-creating `data_dir()`), `logging.rs` (redacting formatter, daily file), `money.rs` (`Money` over `rust_decimal`, half-even, shortest-text float conversion), 4 legacy-DB fixtures. Only the four pre-approved dependencies were added.
- 2026-11-03: **Claude's independent review of T001.** Re-ran the checks (48 Rust, 25 Python); read `db.rs`, `money.rs`, `logging.rs`, `tests/migration.rs`; grepped the diff for secrets and forbidden files (none); ran the debug server on an empty temp data folder with an empty cwd (created `zoneboard-<date>.log` and a `journal.db` at `user_version` 2, no stray `journal.db`, live orders refused). **Mutation checks** (break the code, the tests must fail): redaction off, migration-2 version write dropped, newer-DB refusal removed, rounding half-even to half-away. All four were caught. **Defect found:** `is_secret_field` missed `apiKey`, `APIKEY`, `X-MBX-APIKEY`, `private_key`, `passphrase`, `cookie`, `bearer`, `pin` (probe-confirmed), which matters because T007 logs Binance requests. Also an unguarded `ZONEBOARD_DATA` mutation in a parallel test.
- 2026-11-03: **T001 corrections round** (`docs/platform/reviews/T001-round2.md`, run via `delegate.ps1 -Followup`, log `T001-accept-edits-20261103-104612.log`) landed as `292c4da`: field names normalised (lowercase, strip `_ - . space`) then substring and exact-match lists, a table test (20 secret and 10 harmless names) and an end-to-end `apiKey` / `X_MBX_APIKEY` test, plus an `ENV_LOCK` mutex. Claude re-verified: 50 Rust tests, 0 warnings, 25 Python; two further mutations (drop `apikey`, exact match made substring) are caught. **Known limit, by design:** a secret interpolated into the message text is not masked (documented by a test); secrets never go in message text (AGENTS.md).
- 2026-11-03: **Owner approved the merge.** Review-time docs and tooling committed as `1c54356`, then `task/T001` merged with `--no-ff` as `d0bec79`. Checks on merged `main`: 50 Rust tests (2 parity, 32 unit, 7 migration, 9 API), 0 warnings, 25 Python tests.
- 2026-11-03: **Wave 2 plan-only runs** (T002, T003, T004 in parallel, sandboxed, no files changed; logs `..\zoneboard-wt\logs\T00{2,3,4}-plan-20261103-1104*.log`). Claude reviewed all three against their specs and external facts and added corrections to each spec. Defects found in the plans: **T004** `keyring = "3"` enables no backend (3.6.3 has no default features), so it would silently use an in-memory mock that loses secrets on restart (checked on crates.io); its size check counted UTF-8 bytes but `keyring` stores UTF-16, so the real ceiling is 1280 characters (checked in the 3.6.3 source); it also leaned on an unapproved `thiserror`. **T002** put the token into an ETag header (violates "only in the meta tag") and cached HTML would carry a dead token after a restart; route-coverage test was hand-wavy. **T003** hand-typed ~430 timezone names (now generated by a script), and `INSERT OR REPLACE` would bypass the DELETE trigger and overwrite audit history (third trigger added). Accepted from the plans: removing the config preload link, `AppState::api_token()` accessor, `Journal` connection sharing (as a closure helper), flat settings JSON.
- Plan artifacts that Antigravity writes land under `%USERPROFILE%\.gemini\antigravity-cli\brain\...`; the run log links to them.
- Reminder: delete the Python backend after Phase 1 if the parity tests have moved to fixtures (T009).
