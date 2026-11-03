# Progress

Single source of shared context for Claude, Antigravity and the owner. Claude updates it after every review. Read `ROADMAP.md` for the plan and `WORKFLOW.md` for how work is handed off.

**Updated:** 2026-11-03 · **Phase:** 0 (T001 reviewed and accepted on `task/T001`, awaiting merge approval) · **Baseline commit:** `ed9c249`

## Next

1. Owner: approve **merging `task/T001` into `main`** (`git merge --no-ff`), and committing the docs and tooling changes made during review (`tools/delegate.ps1` `-Followup`, `docs/platform/reviews/T001-round2.md`, this file).
2. Owner: answer decisions **D1 to D3** below (D3 decides whether FYERS can ever place orders). Not needed for Wave 2.
3. Claude, after the merge: dispatch **T002, T003, T004** in parallel (separate worktrees), with **T009** and **T010** beside them.

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
| T001 foundation | **implemented, reviewed, accepted; awaiting merge** | task/T001 | `292c4da` (on `1c9f872`) | 50 Rust + 25 Python tests; 2 review rounds; see Log |
| T002 API token | spec ready | | | after T001 |
| T003 audit and settings | spec ready | | | after T001 |
| T004 vault | spec ready | | | after T001 |
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
- Plan artifacts that Antigravity writes land under `%USERPROFILE%\.gemini\antigravity-cli\brain\...`; the run log links to them.
- Reminder: delete the Python backend after Phase 1 if the parity tests have moved to fixtures (T009).
