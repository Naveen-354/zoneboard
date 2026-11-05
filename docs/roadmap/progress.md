# Master roadmap progress

Tracks `docs/ZONEBOARD_MASTER_PROMPT.md`. Updated 2026-11-04, branch `feature/bot-studio` (not merged to `main`).

Status words: **VERIFIED** = implemented, automated tests pass and it was driven in a real browser;
**TESTED** = implemented and covered by automated tests; **IMPLEMENTED** = built but not covered by a test;
**BLOCKED** = needs something outside this sandbox (named); **NOT_STARTED** = not built.

> Nothing here was run against a live exchange, a real API key or real money. The build sandbox could not reach
> Binance, Yahoo or Anthropic; every external integration is tested against a local fake server.

## Phase 0: audit and baseline: VERIFIED
| Feature | Status | Evidence |
|---|---|---|
| Repository audit, current/target architecture | VERIFIED | `docs/architecture/current-state.md`, `target-state.md` |
| Baseline tests recorded | VERIFIED | 107 Rust + 30 Python before this work (`docs/testing/test-results.md`) |
| Decisions recorded | VERIFIED | `docs/architecture/decisions/` |

## Phase 1: market data and broker foundation
| Feature | Status | Evidence / gap |
|---|---|---|
| Binance signed REST client (HMAC), clock sync, weight, 429/418 pause | TESTED | `exchange/binance.rs`; official signature vector; fake-server tests |
| Connect / verify / balances / disconnect, secrets only in the OS vault | VERIFIED | `connections.rs`, `tests/binance.rs`, Accounts page |
| Withdrawal-enabled keys refused, live needs `confirm_live` | TESTED | mutation-checked |
| Capability metadata | TESTED | `exchange::Capabilities` in balances/verify replies |
| Data quality: duplicates, disorder, gaps (NSE-session aware), staleness | TESTED | `marketdata.rs`; backtests repair and report; runner refuses entries on stale data |
| Market-data health endpoint | TESTED | `/api/market-data/health` |
| Real Binance testnet / live connection | **BLOCKED** | needs the owner's throwaway testnet keys and network access; owner runs it by hand |
| Ed25519 Binance keys | NOT_STARTED | HMAC only |
| FYERS OAuth, token lifecycle, live NSE feed | **BLOCKED** | `docs/fyers/SPEC.md` items U1 to U3 (hosts, redirect URI, WebSocket protocol) must be verified with the owner's FYERS app first; AGENTS.md says stop and ask |
| Server-side WebSocket market-data hub, historical candle store and backfill jobs | NOT_STARTED | candles are fetched per request and cached; no persistent candle store |

## Phase 2: strategy engine and research
| Feature | Status | Evidence / gap |
|---|---|---|
| 22 causal indicators (SMA, EMA, WMA, RSI, Stoch, StochRSI, MACD, ADX, ATR, Supertrend, CCI, ROC, Bollinger, VWAP, Z-Score, OBV, volume SMA/ratio, VW momentum, Ichimoku, Donchian) | TESTED | hand vectors, published RSI example, prefix (no look-ahead) property for every indicator |
| Declarative strategies: AND/OR/NOT, crosses, comparisons, rising/falling, shifts; signal, grid, DCA, pairs/stat-arb | VERIFIED | `strategy.rs`, visual builder |
| Time/session filters in conditions | NOT_STARTED | |
| 23 templates | VERIFIED | every template validates and backtests |
| Immutable strategy versions (DB triggers), export/import, packs | TESTED | `strategies.rs` |
| Event-driven backtester: next-open fills, fees, slippage, stop-before-target, gaps, partial fills by volume | TESTED | `backtest2.rs` |
| Metrics incl. Sharpe/Sortino (annualisation from bar timestamps), PF, expectancy, exposure, streaks, buy-and-hold benchmark | TESTED | |
| In-sample / out-of-sample optimisation, walk-forward, Monte Carlo (seeded), regime split | VERIFIED | tested + Backtesting tab |
| Reproducibility: engine version, dataset hash, strategy hash, seed stored; same input same output | TESTED | determinism test, API test |
| Market Replay (play/pause/step/speed) | VERIFIED | Replay tab; replay reveals a causal run |
| Indian taxes/levies, funding/borrow costs, market calendars and holidays | NOT_STARTED | a flat fee per side is the cost model |
| Bayesian / genetic optimisation | NOT_STARTED | grid search only |

## Phase 3: bot runtime, paper trading and risk
| Feature | Status | Evidence / gap |
|---|---|---|
| Bot runner (candle-close driven), pause on restart, gap skip | TESTED | runner tests with real bar sequences |
| Lifecycle stages with audited invalid transitions | TESTED | |
| Paper trading on the demo account through the order manager | TESTED | wallet moves by exactly the bot's P&L |
| Central risk engine: kill switch, account limits, bot limits, funds, broker gate | TESTED | mutation-checked; manual ticket orders use it too |
| Broker execution via outbox: deterministic client ids, no duplicates, lost-reply lookup, reconciliation each tick | TESTED | fake-exchange test; mutation-checked |
| Stop policy (keep vs close), kill switch flattens | TESTED / IMPLEMENTED | API tests; broker flatten path tested only via `run_intents` |
| Runner heartbeat and health endpoint | VERIFIED | |
| Price-deviation checks, instrument allow/deny lists, consecutive-loss pause, portfolio drawdown limit | NOT_STARTED | |
| Bot runtime on live Binance data | **BLOCKED** | sandbox cannot reach Binance; owner to run a demo bot |

## Phase 4: AI research and approval
| Feature | Status | Evidence / gap |
|---|---|---|
| Provider (Anthropic) with key in vault, model choice, off switch, budget, timeout, audit | TESTED | fake model server |
| Strategy drafts (strict validation, never auto-saved), risk review, backtest explanation checked number by number | TESTED | injection reply rejected; invented numbers flagged |
| No AI tools: cannot approve, arm, change limits, read secrets, trade | TESTED | approval requires actor `user`; mutation-checked |
| Approval Center bound to exact scope; expiry; revocation | VERIFIED | |
| Real Anthropic calls | **BLOCKED** | needs the owner's API key (a paid service) |

## Phase 5: scanners
| Feature | Status | Evidence / gap |
|---|---|---|
| Movers, relative volume, breakout/breakdown, volatility, trend+momentum, NSE gap, relative strength, correlation | VERIFIED | formula tests, API test, Scanner page |
| Source, timestamp, formula, freshness on every result; scan history; result opens a backtest | VERIFIED | |
| Funding/OI, options analytics, news/calendar, spread/depth | NOT_STARTED (listed as unavailable in the UI, with reasons) | no entitled data source |
| Scanner alerts | NOT_STARTED | no notification channel exists yet |

## Phase 6: portfolio and analytics
| Feature | Status | Evidence / gap |
|---|---|---|
| Accounts overview; broker balances; reconciliation (differences shown) | TESTED | |
| Attribution (account, bot, manual), daily/weekly/monthly reports, exposure, CSV | VERIFIED | |
| Correlation of bots, inverse-volatility allocation proposal (never applied) | TESTED | |
| Currency conversion | NOT_STARTED | USDT and INR are kept separate, never converted |

## Phase 7: developer hub
| Feature | Status | Evidence / gap |
|---|---|---|
| Scoped, hashed, revocable, rate-limited API tokens; safety endpoints unreachable | VERIFIED | API test, mutation-checked, Settings page |
| OpenAPI description kept complete by a test | TESTED | |
| Python SDK (stdlib) with contract test against the real server | TESTED | `tools/sdk/` |
| Strategy import/export and packs (schema, hash, engine checks) | TESTED | |
| Sandboxed custom code execution | NOT_STARTED | by decision (ADR-003): strategies stay declarative |
| Webhooks / event subscriptions | NOT_STARTED | outbound calls to arbitrary hosts conflict with AGENTS.md |

## Final end-to-end acceptance (section 14)
Steps 1, 5 to 16 and 21, 22: done on sample data and the fake exchange (automated tests + browser).
Steps 2 to 4 and 17 to 20 with a real broker: **BLOCKED** on the owner's keys and network; 17/18 with real money
only with the owner's explicit authorisation, never by an agent.
