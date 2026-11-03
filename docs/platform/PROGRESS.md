# Progress

Single source of shared context for Claude, Antigravity and the owner. Claude updates it after every review. Read `ROADMAP.md` for the plan and `WORKFLOW.md` for how work is handed off.

**Updated:** 2026-11-02 · **Phase:** 0 (not started) · **Blocker:** the current work is uncommitted (29 files); delegation needs a commit.

## Next

1. Owner: commit the current work (a worktree is made from a commit).
2. Owner: answer decisions **D1 to D3** below (D3 decides whether FYERS can ever place orders).
3. Claude: dispatch `T001` in plan mode (`tools/delegate.ps1 -Task T001 -Mode plan`), review the plan, then implement. `T009` and `T010` can run beside it.

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
| T001 foundation | spec ready | | | start here |
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
- Plan artifacts that Antigravity writes land under `%USERPROFILE%\.gemini\antigravity-cli\brain\...`; the run log links to them.
- Reminder: delete the Python backend after Phase 1 if the parity tests have moved to fixtures (T009).
