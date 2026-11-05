# ZoneBoard Phases 9 to 12: plan

Branch `feature/phases-9-12`, from `main` at `9cc25d3`. Baseline on that commit: 171 Rust tests, 31 Python, 6/6
browser smoke, 0 warnings, no pre-existing failures. Progress: `ZONEBOARD_PHASES_9_12_PROGRESS.md`.

## Repository findings (what exists and is reused)

Single-user desktop app: Tauri window or browser on loopback, one Rust (axum) server, one SQLite file (schema 8),
vanilla-JS UI without a bundler. No login: the per-launch token plus scoped developer tokens. No event bus,
no job queue, no telemetry stack: one in-process tokio runner. `AGENTS.md` rules apply: loopback only, no new
outbound hosts without reason, secrets only in the OS vault, money in decimals, no real orders unless armed +
approved.

| Requested | Already in the code | Gap to close |
|---|---|---|
| 4.1 Infrastructure monitor | `/api/system/health` (runner heartbeat, unreconciled orders), bot `last_error`, account `last_error`/`last_sync_utc`, `marketdata::check` | per-component status, incidents with dedupe, thresholds, latency metrics |
| 4.2 Portfolio risk | account limits (order size, exposure, daily loss), bot limits, kill switch, `portfolio.rs` exposure | portfolio limits, max drawdown, max open positions, instrument deny list, contributors, overlap, breach events |
| 4.3 Attribution | `portfolio.rs` by account/bot/manual | by symbol, exit reason, regime; fees vs gross; drill-down to trades and fills; reconciliation total |
| 4.4 Drift | backtest metrics stored per bot | baseline vs rolling comparison, sample sizes, statuses |
| 4.5 Version control | immutable `strategy_versions` (DB triggers), export/import | diff, tags, rollback, trade to version link, concurrent-edit guard |
| 5.1 Monte Carlo | seeded bootstrap in every backtest | block bootstrap, ruin probability, thresholds, stored runs, async job |
| 5.2 Walk-forward | anchored walk-forward | rolling mode, step size, stored folds, failed folds, full-period comparison |
| 5.3 Regimes | regime split of trades (trend, volatility) | classifier with versioned rules, confidence, features, history |
| 5.4 Robustness | ingredients exist (OOS, MC, regimes, data quality) | versioned score with evidence per dimension |
| 5.5 A/B testing | backtests, versions | experiment entity running two versions on the same window |
| 6.1 Quant researcher | templates, optimiser, walk-forward, AI drafts (validated) | bounded deterministic pipeline, report, human review gate |
| 6.2 Trade explainer | trades, bot_orders, broker_orders, strategy versions, audit, AI module | fact sheet from records; AI interpretation labelled separately |
| 6.3 News, 6.4 Economic events | none | provider interface, fixture provider (labelled), dedupe, sentiment, alerts |
| 6.5 Workflow builder | none | constrained node catalogue, validation, dry-run, executions |
| 7.1 Multi-account | accounts, per-account bots, limits, portfolio views | account enable/disable enforced in routing and risk |
| 7.2 Smart execution | paper order path, broker outbox | parent/child orders, TWAP and slicing, metrics (paper first) |
| 7.3 Options lab | none; no provider gives ZoneBoard options chains | Black-Scholes pricing, Greeks, payoffs, templates; theoretical only |
| 7.4 Ledger | trades, wallets, audit log | immutable ledger with reversals, statement import, reconciliation, export |
| 7.5 Workspaces | single user, no login by design | see Blockers |

## Target architecture (additions)
- `events.rs`: a durable, typed, versioned event log (table `events`: type, version, correlation id, idempotency
  key unique, payload without secrets). Emitted by risk, bots, backtests, strategy versions, monitor. Workflows and
  incidents read it. This is the event bus: one process does not need another platform (ADR-005).
- `jobs.rs`: durable jobs table plus a cancel flag; Monte Carlo, walk-forward, research and experiments run as
  jobs and return a job id.
- New modules: `monitor.rs`, `riskintel.rs`, `drift.rs`, `regime.rs`, `robustness.rs`, `experiments.rs`,
  `researcher.rs`, `explain.rs`, `intel.rs` (news + calendar), `workflows.rs`, `smartexec.rs`, `options.rs`,
  `ledger.rs`. Migration 9 adds their tables. UI: two new pages, **Operations** and **Research Lab**, with tabs.

## Order and gates
Phase 9, 10, 11 then 12 as in the brief; each phase ends green and committed. Live execution stays as it is:
armed + approved + limits, never exercised here.

## Security and risk
Every order path keeps going through `risk.rs`; new checks are added there, not beside it. Research, experiments,
regimes, news and workflows get no order capability; workflow actions are a fixed catalogue (notify, pause a
paper bot, request approval, start research, run a risk assessment). Tests prove each of those boundaries.

## Blockers
- **7.5 Workspaces** would need user accounts and a server reachable by other people. ZoneBoard is deliberately
  a loopback-only single-user app without login (AGENTS.md, ADR-001). Building multi-user tenancy would weaken that
  model; it needs the owner's decision first.
- **Live news, calendar and options data**: no licensed provider is configured; only fixture providers, labelled.
- **Broker statements**: import is built for a documented CSV format; real statement formats need samples.
- No network here: nothing is verified against live services.
