# Test results (2026-11-04, branch feature/bot-studio)

| Suite | Before | Now |
|---|---|---|
| Rust `cargo test --no-default-features` | 107 | **171 passed, 0 failed**, 0 warnings (113 unit, 36 API, 14 migration, 4 Binance, 1 AI, 2 parity, 1 doctest) |
| Python `python -m pytest -q` | 30 | **31 passed** (30 + SDK contract) |
| Browser smoke `node tools/ui_smoke.mjs` | 5 | **6/6 passed** |
| Mutation checks | n/a | **13/13 caught** (2 survived at first; tests strengthened) |
| End-to-end browser drive of every new page | n/a | no console errors; no horizontal scroll at 375 px |

**Not run here (unverified, not passed):** `node tools/live_check.mjs` (sandbox blocks Binance); any real
Binance testnet or live call; real Anthropic calls; the Tauri desktop window; the `#[ignore]` Credential Manager
test (Windows only).

**Owner checks before relying on it:** back up `journal.db` (schema goes 4 → 8; older builds refuse it); run the
live check; connect a Binance **testnet** account with throwaway keys and watch balances; run a demo bot for a
few candles; only then consider a testnet bot (needs a saved backtest).
