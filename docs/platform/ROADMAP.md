# Roadmap: ZoneBoard to a personal algo-trading platform

Read `AUDIT.md` first. Delegation mechanics are in `WORKFLOW.md`. Live status is in `PROGRESS.md`.

## 1. Principles

1. **One engine for backtest, paper and live.** A strategy runs through the same Rust code in all three; only the exchange adapter differs. This is the single biggest protection against "worked in the backtest, behaved differently live".
2. **The server owns trading.** Market data used for decisions, order placement, risk checks and strategy execution all run in the Rust server. The browser displays and sends commands. (Audit findings 2 and 3.)
3. **No real order path until the gates in section 5 are met.** Until then every order goes to the paper exchange or a testnet.
4. **Never simulate real balances.** The demo account's virtual credits are always labelled as virtual. Live balances are fetched from the broker, or shown as "not connected".
5. **Evolve, don't replace.** Keep Rust + axum + Tauri, SQLite, the vanilla-JS UI and its look. New code goes behind clear module boundaries.
6. **Small, spec'd, testable tasks** that Antigravity can finish in one run, reviewed by Claude before merge.

## 2. Target architecture

```
UI (static/, Tauri window)  --HTTP + SSE, API token-->  Rust server
  charts keep their direct Binance feed (display only)    |- api         auth token, settings, accounts, orders, strategies
                                                          |- vault       OS keychain; secrets never leave it
                                                          |- data hub    Binance WS (server side), FYERS feed, candle store
                                                          |- exchanges   trait Exchange: Paper | BinanceTestnet | BinanceLive | Fyers
                                                          |- orders      lifecycle, stop-loss / take-profit / trailing, execution log
                                                          |- risk        pre-trade checks, daily loss, exposure, kill switch
                                                          |- strategy    candle-driven runtime, same code: backtest | paper | live
                                                          |- backtest    replays history through the runtime
                                                          '- store       SQLite, versioned migrations, decimals, audit log
```

## 3. Decisions needed from you

Claude has a recommended default for each, so work can start. Change any of them and the plan adapts.

| # | Decision | Recommended default | Why |
|---|---|---|---|
| D1 | Binance scope | **Spot only first.** Futures/margin later, as a separate decision | Leverage multiplies loss; spot has a testnet and a simpler order model |
| D2 | Where API keys are stored | **Windows Credential Manager** (OS keychain, via the `keyring` crate). A master-password encrypted file is an optional fallback | Tied to your Windows login, no password for you to manage, nothing in the database |
| D3 | **FYERS live orders: do you have a static IP?** | Unknown: **needed** | Since 1 Apr 2026 orders only work from a whitelisted static IP. Without one, FYERS stays data + paper only |
| D4 | Python backend | **Freeze now, delete after Phase 1** | Three copies of every rule is unaffordable |
| D5 | Where strategies run | **Rust server** (decided) | Browser tabs throttle when hidden |
| D6 | How strategies are defined | **Declarative JSON** (indicators, entry/exit rules, sizing, risk) built in the UI. No arbitrary user code | Safe, testable, same format for backtest and live. A scripting layer can come later |
| D7 | Money type | **Decimals**, never floats, in all new code | Exactness for balances, fees, sizes |
| D8 | New dependencies | **Pre-approve this batch:** `keyring`, `zeroize`, `secrecy`, `rust_decimal`, `tracing` (+ subscriber), `tokio-tungstenite`, `hmac`, `sha2`, `ed25519-dalek`, `subtle`, `rand`. Anything else needs asking | Stops every task blocking on approvals |

## 4. Phases and tasks

`T0xx` specs for Phases 0 and 1 are written in `tasks/`. Later tasks are specified **just in time**, so the specs reflect what earlier tasks actually built. "Touches" lists the main files so overlapping tasks aren't run at once.

### Phase 0: Foundation and hygiene

| Task | What | Depends | Touches |
|---|---|---|---|
| **T001** | Versioned DB migrations, one data directory, structured logging with redaction, decimal money type | commit | `db.rs` (new), `journal.rs`, `server.rs` |
| **T002** | Per-launch API token, CSRF/Origin checks on every state-changing route | T001 | `server.rs`, `static/js/util.js` |
| **T003** | Audit log (append-only) and settings store, with API | T001 | `audit.rs`, `settings.rs` (new), `server.rs` |
| **T009** | Freeze the Python backend: docs and CI note, parity tests kept | none | docs, `AGENTS.md` |
| **T010** | Browser smoke-test harness (drives the Tauri/Chrome debug port) so UI regressions are caught | none | `tools/` |

### Phase 1: Accounts, credentials, Binance connection (read side)

| Task | What | Depends | Touches |
|---|---|---|---|
| **T004** | Credential vault on the OS keychain; secrets are write-only through the API | T001 | `vault.rs` (new) |
| **T005** | Accounts model; **Demo account with configurable virtual credits** and reset; paper positions/trades linked to an account | T001 | `accounts.rs` (new), `journal.rs`, `server.rs` |
| **T006** | Navigation shell (Board, Accounts, Journal, Settings) and the Accounts and Settings pages | T003, T005 | `static/` |
| **T007** | Binance signed REST client, **read side**: balances, permissions check (**reject keys with withdrawals enabled**), clock sync; testnet and live base URLs by config | T001, T004 | `exchange/binance.rs` (new) |
| **T008** | **Independent adversarial security review** of T002, T004, T007 by Antigravity, report to Claude | T002, T004, T007 | none (report only) |

*Exit:* a Demo account with credits, a connected Binance **testnet** account showing real testnet balances, secrets only in the keychain, audit log recording connection events.

### Phase 2: Execution core (server-side)

T020 server-side Binance market-data hub and candle store · T021 `Exchange` trait, order model and persistence · T022 `PaperExchange` (server-priced fills, balances from the demo account) · T023 order manager: market/limit/stop, stop-loss, take-profit, trailing stop · T024 risk engine: per-order checks, max daily loss, exposure limits, kill switch · T025 `BinanceExchange` orders on **testnet**: place, cancel, query, user-data stream · T026 Orders UI (ticket v2, own-order book, execution log) · T027 portfolio overview: balances, exposure, P&L.

*Exit:* every order (paper or testnet) flows through the same manager and risk engine; the browser no longer sets fill prices.

### Phase 3: Strategy platform

T030 indicator library with published test vectors (SMA, EMA, RSI, MACD, ATR, Bollinger, VWAP, more) · T031 strategy spec (JSON schema) and validator · T032 strategy runtime (candle-close driven) · T033 backtester v2: fees, slippage, metrics (return, drawdown, Sharpe, win rate, expectancy) · T034 strategy manager API and UI · T035 strategy builder UI · T036 forward-test runs (paper, scheduled) · T037 automated execution with **arm/disarm** and kill switch · T038 analytics and strategy comparison.

*Exit:* build a strategy in the UI, backtest it, forward-test it on the demo account, compare runs.

### Phase 4: FYERS (can run in parallel with phases 2 and 3)

T050 to T057 = the eight stages in `docs/fyers/STAGES.md` (live NSE data, daily login). T058 funds/positions read. **T060 FYERS orders: only if D3 is yes**, and verified with tiny real orders (no sandbox exists).

### Phase 5: Analytics, operations, polish

T070 notifications · T071 audit viewer, export and backup · T072 installer and packaging · T073 terminal-style UI polish.

### Phase 6: Live enablement (gated)

T080 live Binance with tiny size, after the gates · T081 live FYERS tiny orders (if D3).

## 5. Gates: what must be true before real money

| Gate | Condition |
|---|---|
| **G1** | No real-order code path exists until Phase 1 and the P0 items in `AUDIT.md` (API token, server-side pricing, engine in the server) are done. |
| **G2** | Live trading is **armed** by you, in Settings, with a typed confirmation and per-account limits (max order size, max daily loss, max exposure) already set. The default is disarmed after every restart. |
| **G3** | A Binance **testnet soak**: strategies run unattended with no unexplained orders. FYERS has no sandbox, so its first live orders are tiny and watched. |
| **G4** | The order path has been reviewed by Claude and adversarially by Antigravity (T008 pattern), and every finding is closed. |
| **G5** | A **kill switch** exists in the UI and the API (cancel all, flatten, disarm). |

Agents never run live verification. Only you do, by hand.

## 6. Order of work and parallelism

1. Commit the current work (prerequisite for worktrees).
2. **Wave 1:** T001 alone first (everything builds on it), plus T009 and T010 beside it.
3. **Wave 2 (parallel, separate worktrees):** T002, T003, T004. They overlap only in route registration in `server.rs`; merge in that order.
4. **Wave 3:** T005, T007, then T006, then T008.
5. Then Phase 2, with Phase 4's FYERS data stages alongside if you want NSE live early.

At most 2 or 3 tasks run at once. Claude merges in dependency order.

## 7. Who does what

| Claude (architecture, review, little code) | Antigravity (bulk of the work) |
|---|---|
| Decisions D1 to D8, the architecture, task specs | Implementation, tests, UI, docs, repetitive changes |
| Review of every merge, especially vault, API auth, order manager, risk engine | Self-check against the spec; adversarial reviews (T008) |
| Updates `PROGRESS.md` and re-plans | Reports in the fixed format from `WORKFLOW.md` |

## 8. Top risks

| Risk | Mitigation |
|---|---|
| FYERS needs a static IP (D3) | Decide early; otherwise FYERS stays data + paper |
| A bug sends real orders | Gates G1 to G5; live arms off by default; tests and agents never hold real keys |
| Antigravity changes diverge or break working code | Worktrees, specs with "Touches", review before merge, tests as the contract |
| Broker API changes (Binance user-data stream already changed) | Verify against official docs inside each task; contract tests with recorded fixtures |
| `windows-gnu` Tauri packaging | Packaging task is late and may switch the desktop shell to a browser-served mode |
| Scope creep | Specs just in time; Python frozen; out-of-scope list per task |
