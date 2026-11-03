# Codebase audit (2026-11-02)

Scope: what exists today, measured against the mission (a secure personal algo-trading platform: Binance + FYERS, live and demo accounts, strategies, backtesting, risk management). Facts below were measured or run on this machine unless marked *(verify)*.

> Caveat: Claude wrote most of this code in recent sessions, so this is a mechanical audit plus known-issue review, not an independent one. An independent security review by Antigravity is task **T008** in the roadmap.

## 1. Snapshot

| Area | Size | Notes |
|---|---|---|
| Web UI (`static/`) | ~2,000 lines JS, 200 CSS, 140 HTML | Vanilla ES modules, no framework, light theme (ZoneBoard) |
| Rust (`src-tauri/`) | ~1,900 lines + 330 tests | axum server, Tauri window, SQLite journal, zone engine, backtest |
| Python (reference backend) | ~830 lines + 420 tests | Original Flask app, kept for parity tests |
| Docs | ~480 lines | README, AGENTS.md, Fyers spec/stages |

**Tests, run today:** Rust 33 pass (22 unit, 9 HTTP, 2 Python-parity), Python 25 pass, live Binance pipeline check passes, release build succeeds, the desktop exe was driven end to end through its debug port. No TODO/FIXME markers.

**Git:** one commit (the baseline). **29 files are uncommitted** (two feature rounds). This must be committed before any work is delegated, because delegation uses git worktrees from a commit.

## 2. Working today

- Live crypto data from Binance public streams (browser-direct), 30 coins, any pair via search; auto-reconnect and refill; rate-limit-safe subscriptions.
- Zone engine in three languages with parity tests (Python is the reference).
- Board scoring, chart with zones/levels/volume, keyboard navigation, light UI, mobile layout.
- Backtest CLI for the order-block rule on Binance or NSE history.
- Rust server + Tauri desktop app (single exe + `WebView2Loader.dll`), loopback-only, Host check, CSP.
- Journal (SQLite), per market and per account.
- Paper trading on real prices: market orders, long/short, simulated fees, positions that survive restarts, P&L; Demo account vs Live account toggle (Live is disabled and refused by the server).
- Specs and stage plan for live NSE data via FYERS (`docs/fyers/`), not built.

## 3. Not built (against the mission)

Profile and settings; demo account with configurable credits; credential vault; any broker connection beyond public data; balances/margin/portfolio; deposit and withdrawal history; order management (only instant paper market orders); stop-loss, take-profit, trailing stop; position sizing; risk limits (daily loss, exposure); indicators beyond ATR/EMA/zones; strategy builder, runtime and manager; a general backtester; forward testing; automated execution; analytics; audit log; notifications; FYERS data or orders.

## 4. Defects and risks found

Severity: **P0** blocks any real-order work, **P1** needed before the platform is trustworthy, **P2** quality.

| # | Finding | Sev | Why it matters | Fix (roadmap task) |
|---|---|---|---|---|
| 1 | **No authentication on the local API.** Loopback-only plus a Host check, but any local process or browser extension can call every endpoint. | **P0** | Once an order endpoint exists, anything on the machine could place trades. | Per-launch API token + CSRF/Origin checks (T002) |
| 2 | **Paper fills use a price the browser sends** (`POST /api/positions`). The server never checks it. | **P0** | Strategies and live orders must be priced by the server from its own feed. | Server-side market data hub + order manager (Phase 2) |
| 3 | **Execution logic lives in the browser.** A hidden tab throttles timers and animation frames (observed). | **P0** | Automated strategies cannot depend on a tab being visible. | Engine in the Rust server (D5) |
| 4 | **Money is `f64`** (REAL columns) for prices, quantities, fees, P&L. | P1 | Rounding drift is unacceptable for balances and live orders. | Decimal types for new code (T001), migrate later |
| 5 | **No credential storage.** `.env` plan only. | P1 | The brief requires encrypted key storage. | Vault on the OS keychain (T004, decision D2) |
| 6 | **No schema migrations**: ad-hoc `ALTER TABLE` and `UPDATE` run at startup in two languages. | P1 | Every new table risks corrupting the user's journal. | Versioned migrations (T001) |
| 7 | **No audit log, no structured logging, no redaction tests.** | P1 | Required by the brief; also the only way to debug live incidents. | T001, T003 |
| 8 | **Three copies of the zone rules** (Python, JS, Rust). New indicators would triple the cost. | P1 | Maintenance and drift. | Indicators and strategies in Rust only; freeze Python (D4, T009) |
| 9 | **The backtester is one hard-coded rule** with a simple fill model (no slippage, no partial fills, no generic strategy). | P1 | Cannot compare strategies or trust results. | Backtester v2 on the shared runtime (Phase 3) |
| 10 | **Journal location differs** between the Python and Rust apps and depends on the launch folder. | P2 | Surprising data loss risk. | One data directory (T001) |
| 11 | **Tauri runs on `windows-gnu`** (unofficial): `WebView2Loader.dll` must sit beside the exe, one linker warning. | P2 | Packaging and installer risk. | Packaging task (Phase 5) |
| 12 | **UI has two screens** (Board, Journal) and no navigation structure. | P2 | A trading terminal needs Accounts, Orders, Strategies, Settings. | Navigation shell (T006) |
| 13 | **No browser-level tests.** UI was verified by hand through scripts. | P2 | Regressions in the order path would be silent. | Smoke-test harness (Phase 0, T010) |
| 14 | `unwrap()` on a poisoned mutex in the journal and a few `expect`s in server startup. | P2 | A panic in the server must not leave orders in an unknown state. | Error-handling pass during Phase 2 |

## 5. Security posture

Good: loopback only, Host-header check, CSP, `textContent` UI, secrets git-ignored, tests never touch the user's journal, no secrets in logs today (none exist yet).

To add before any live order: API token, vault, redaction tests, audit log, an explicit **arming** step for live trading, kill switch, per-account limits, rejection of Binance keys that have withdrawal enabled, and an adversarial review of the order path.

## 6. Broker facts that shape the plan

Checked on 2026-11-02 against official or broker pages where possible. **Re-verify each against the official docs inside the task that uses it.**

**Binance**
- A **Spot Testnet** exists (`testnet.binance.vision` REST under `/api`, WebSocket API at `ws-api.testnet.binance.vision`). It has **no `/sapi/*`**, so wallet endpoints (deposit and withdrawal history) can't be tested there. *(third-party source, verify)*
- The old `listenKey` user-data stream was removed on **2026-02-20**; the replacement is a `userDataStream.subscribe` request on the WebSocket API. *(single third-party source, verify in the official docs)*
- Key permissions are Reading / Spot & Margin Trading / Withdrawals. Binance advises IP restrictions. `GET /sapi/v1/account/apiRestrictions` reports a key's permissions, so the app can **refuse any key with withdrawals enabled**. HMAC keys are system-generated; Ed25519/RSA keys are self-generated. *(verify)*
- Deposit and withdrawal history lives in the wallet (SAPI) endpoints and should work with a read-only key. *(not confirmed)*

**FYERS**
- **No official sandbox.** Paper trading exists only in a separate product (FYERS API Bridge). Verifying FYERS orders means tiny real orders on a real account, after your explicit go-ahead. Paper trading for NSE stays inside this app.
- **SEBI retail algo framework, in force since 1 April 2026:** orders are accepted only from a registered App ID tied to a **whitelisted static IP** (unique per user); **2FA once per trading day** with no long-lived refresh sessions; up to **10 orders per second** without exchange registration; API apps created before the change cannot place orders. *(FYERS notice and broker summaries; confirm with FYERS)*
- **Consequence: a desktop app on a home connection with a changing IP cannot place FYERS orders.** You need a static IP (an ISP static address, or a small server or VPS that runs the app). This is the biggest feasibility question in the whole mission: see decision D3.
- FYERS market-data limits (Standard plan: 100 live symbols, 50 requests/min) are in `docs/fyers/SPEC.md`.
- The FYERS API exposes funds, holdings, positions, orders, trades, order placement and modification. A deposit/withdrawal ledger was not found, so the UI must say "not available from FYERS" rather than invent one. *(verify)*

## 7. What to keep (no strong reason to replace)

Rust + axum + Tauri, SQLite, the vanilla-JS UI and its design tokens, the zone engine and its parity tests, `docs/fyers/` specs, `AGENTS.md`. The roadmap extends these.
