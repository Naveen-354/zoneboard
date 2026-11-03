# T010: Browser smoke-test harness

Phase 0 · Depends: none · Touches: `tools/ui_smoke.mjs` (new), `AGENTS.md` (add the command), `README.md` (one line)

## Goal
Catch UI regressions automatically. Today the UI is verified by hand-run scripts. Build a small, repeatable harness with **no new npm dependencies** (Node 22+ has `fetch` and `WebSocket` built in).

## In scope
1. `node tools/ui_smoke.mjs` starts `zoneboard-server` (debug build is fine) on a free port with `SAMPLE_DATA=1`, a temporary journal and a temporary data folder, so it needs **no internet and touches no real data**.
2. It launches Microsoft Edge (or Chrome) headless with a remote-debugging port and drives the page over the DevTools protocol (the same way the repo's earlier debug scripts did).
3. Scenarios: boot (no console errors, board populated, chart drawn, pill reads "Sample data"); the Live/Demo account toggle (instant, ticket disabled with the right message in Live and in sample mode); Board, Journal navigation; mobile width (375 px, no horizontal scroll). Make it easy to add scenarios (a list of named async functions).
4. Prints PASS/FAIL per scenario, exits non-zero on any failure, always kills the browser and server (even on error), and always removes its temp files.

## Out of scope
Order-placement scenarios (they need a real price; they arrive once the server owns pricing in Phase 2), visual diffing.

## Acceptance criteria
- Runs green on this machine; a deliberately broken UI (for example, remove the toggle) makes it fail.
- Works from a clean checkout of the repo (the script builds nothing itself: it needs the server already built, and says so clearly if it is missing).
- Add `node tools/ui_smoke.mjs` to the checks list in `AGENTS.md`.
