# T009: Freeze the Python backend

Phase 0 · Depends: none · Touches: `README.md`, `AGENTS.md`, top-of-file notes in `app.py`, `data.py`, `backtest.py`, `zones.py`

## Goal
Stop maintaining three copies of every feature (decision D4). The Python code stays only as the **reference** for the zone and backtest rules and for the parity fixture.

## In scope
1. Add a short "frozen" notice at the top of `app.py`, `data.py`, `backtest.py` (what it is for, that new features go to Rust only, that it must keep passing its tests).
2. README: the Python server is documented as legacy; the Rust app is the product. Keep the run instructions that still work.
3. `AGENTS.md`: state that Python is frozen: no new endpoints or features; only changes that keep the parity fixture and existing tests green. If the UI starts depending on a Rust-only feature (accounts, tokens), the Python server is allowed to not support it, and the README must say so.
4. Plan the deletion: add a line to `docs/platform/PROGRESS.md` "Delete the Python backend after Phase 1 if parity tests have moved to fixtures".

## Out of scope
Deleting anything, changing behaviour.

## Acceptance criteria
- All tests still pass; only comments and docs changed (show `git diff --stat`).
