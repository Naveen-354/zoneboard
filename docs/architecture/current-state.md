# Architecture: current state (2026-11-04, branch feature/bot-studio)

A single-user desktop app (Tauri window, or any browser on loopback) over one Rust server and one SQLite file.
No cloud service, no login: the server answers on 127.0.0.1 / [::1] only and every `/api` call needs the
per-launch token (or a scoped developer token).

```
Browser / Tauri (static/, vanilla ES modules)
  Board · Scanner · Bot Studio (Explore, My Bots, Create, Library, Backtesting, Replay, AI, Approvals, Performance)
  Portfolio · Accounts · Journal · Settings (Developer tokens)
        │  HTTP JSON, X-ZB-Token  (or Bearer zbk_… for scripts)
        ▼
Rust server (src-tauri/src)
  server.rs      guard (Host check, CSRF origin, tokens), routes
  botapi.rs      handlers for bots, backtests, scanner, portfolio, AI, approvals, dev tokens; the runner loop
  indicators.rs → strategy.rs → botengine.rs   (one engine for backtest, replay, paper and broker)
  backtest2.rs   metrics, optimisation, walk-forward, Monte Carlo, regimes
  strategies.rs  immutable versions, export/import/packs
  bots.rs        lifecycle, DbBook (paper fills or broker intents), runner step, performance
  orders.rs      paper order path          ┐
  execution.rs   broker order path (outbox)├─ both through risk.rs (kill switch, limits, live gate)
  approvals.rs   human approval of live    ┘
  exchange/binance.rs + connections.rs   Binance signed REST; credentials in vault.rs (OS keychain)
  marketdata.rs  data quality · data.rs  Binance/Yahoo/synthetic candles
  scanner.rs · portfolio.rs · ai.rs · devtokens.rs · audit.rs · settings.rs · accounts.rs · journal.rs
        ▼
SQLite journal.db (schema version 8): trades, positions, accounts, wallets, audit_log, settings, bots, bot_orders,
bot_logs, backtests, account_limits, strategies, strategy_versions, approvals, ai_proposals, ai_events,
api_tokens, scan_runs, workers, broker_orders
```

## Environments
| Environment | Where decided | Orders go to |
|---|---|---|
| BACKTEST / REPLAY | request to `/api/backtest`; no account involved, no credentials loaded | in-memory book |
| PAPER | bot's account is a `demo` account (persisted) | `orders.rs` → positions on the demo account |
| TESTNET | account `kind=binance, environment=testnet` (persisted) | `execution.rs` → Binance Spot Testnet |
| LIVE | account `kind=binance, environment=live` (persisted) **and** armed **and** approved | `execution.rs` → Binance Spot |

The environment comes from the persisted account row, never from a request flag.

## Known gaps
See `docs/roadmap/progress.md`: FYERS, a persistent candle store / WebSocket hub, session filters, Indian taxes,
alerts, currency conversion, custom code sandbox, webhooks.
