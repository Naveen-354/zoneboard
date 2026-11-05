# ZoneBoard Phases 9 to 12: progress and status

Branch `feature/phases-9-12` (from `main` at `9cc25d3`). Plan: `ZONEBOARD_PHASES_9_12_PLAN.md`.

| Commit | Phase |
|---|---|
| `46ce373` | 9: monitor, portfolio risk, attribution, drift, strategy version control |
| `911664b` | 10: Monte Carlo, walk-forward, regimes, robustness score, A/B experiments |
| `0a8d59e` | 11: researcher, trade explainer, news, economic events, workflows |
| `d809fa1` | 12: account enable/disable, smart execution (paper), options lab, ledger |
| (this commit) | Operations and Research Lab pages, smoke test, docs |

## Status by feature

Legend: **Done** = implemented, tested, in the UI. **Partial** = what exists works and is tested; the named part is missing.
**Blocked** = needs an owner decision or an external input.

| # | Feature | Status | Where |
|---|---|---|---|
| 4.1 | Trading infrastructure monitor | Done | `monitor.rs`, Operations > Monitor |
| 4.2 | Portfolio-wide risk intelligence | Done | `riskintel.rs` (called inside `risk.rs`), Operations > Risk |
| 4.3 | Performance attribution | Done (regime dimension: Partial, see below) | `attribution.rs`, Operations > Attribution |
| 4.4 | Strategy drift detection | Done | `drift.rs`, Operations > Attribution & drift |
| 4.5 | Strategy version control | Done | `strategies.rs`, Operations > Versions |
| 5.1 | Monte Carlo simulator | Done | `research.rs::monte_carlo`, Research Lab |
| 5.2 | Walk-forward optimisation | Done | `research.rs::walk_forward`, Research Lab |
| 5.3 | Market regime detection | Done | `regime.rs` (`rules-v1`), Research Lab > Regimes |
| 5.4 | Strategy robustness score | Done | `research.rs::robustness` (`robustness-v1`), Research Lab |
| 5.5 | Strategy A/B testing | Partial: historical simulation only | `research.rs::experiment`. Forward (paper) A/B of two live bots is not built. |
| 6.1 | Autonomous quant researcher | Done (bounded, deterministic; no LLM in the loop) | `researcher.rs`, Research Lab > Researcher |
| 6.2 | AI trade explainer | Done (AI part needs the owner's Anthropic key) | `explain.rs`, `ai::explain_trade` |
| 6.3 | News and sentiment | Partial: no live provider | `intel.rs`: manual import + labelled sample provider, dedupe, clustering, heuristic sentiment, alerts |
| 6.4 | Economic event intelligence | Partial: no live calendar provider | `intel.rs`: JSON/CSV import, revisions, release, alerts, entry blackout in central risk |
| 6.5 | Visual workflow builder | Partial: JSON graph editor, not drag-and-drop | `workflows.rs`, Operations > Workflows |
| 7.1 | Multi-account orchestrator | Done | `tradeapi.rs` orchestrator, account status enforced in `riskintel::extra_checks` |
| 7.2 | Smart order execution | Partial: paper only | `smartexec.rs`; broker routing of child orders is not built (refused) |
| 7.3 | Options strategy lab | Done (theoretical only) | `options.rs`, Research Lab > Options lab |
| 7.4 | Trading tax and ledger | Done (record keeping; not tax advice) | `ledger.rs`, Operations > Ledger |
| 7.5 | Multi-user workspaces | **Blocked** | ZoneBoard is loopback-only, single-user, no login (AGENTS.md). Needs an owner decision. |

## Migration

Schema version **9** (`db.rs`), one forward migration, tested from every older fixture (`tests/migration.rs`).
New tables: `events`, `incidents`, `notifications`, `jobs`, `portfolio_limits`, `kv_config`, `drift_observations`,
`regime_observations`, `robustness`, `strategy_tags`, `research_runs`, `news_items`, `econ_events`, `workflows`,
`workflow_versions`, `workflow_runs`, `parent_orders`, `child_orders`, `ledger_entries` (UPDATE/DELETE refused by
triggers), `ledger_imports`. New columns: `account_limits.max_position_notional`, `.max_open_positions`,
`.denied_symbols`; `trades.strategy_version_id`, `positions.strategy_version_id`; `bots.revision`;
`broker_orders.ref_price`.

## API (all under the launch token or a scoped developer token; listed in `/api/openapi.json`)

Phase 9: `GET /api/monitor`, `PUT /api/monitor/thresholds`, `GET /api/events`, `GET /api/notifications`,
`POST /api/notifications/read`, `GET|PUT /api/risk/portfolio`, `PUT /api/risk/limits/{account}/extra`,
`GET /api/attribution`, `GET /api/attribution/trades/{id}`, `GET /api/drift/{id}`, `PUT /api/drift/config`,
`POST /api/drift/{id}/autopause`, `GET /api/strategy-versions/{id}`, `GET /api/strategy-versions/{id}/diff/{other}`,
`POST /api/strategy-versions/{id}/tags`, `POST /api/strategy-versions/{id}/rollback`, `GET /api/jobs`,
`GET /api/jobs/{id}`, `POST /api/jobs/{id}/cancel`.

Phase 10: `POST /api/research/montecarlo`, `POST /api/research/walkforward`, `POST|GET /api/research/robustness`,
`POST /api/research/experiments`, `POST /api/regimes/classify`, `GET /api/regimes/history`. Long work returns
`202 {job_id}`; poll `/api/jobs/{id}`; at most 3 jobs run at once (429 beyond).

Phase 11: `POST|GET /api/research/runs`, `POST /api/research/runs/{id}/review`, `GET /api/explain/trades/{id}`,
`POST /api/explain/trades/{id}/ai`, `GET /api/news`, `POST /api/news/import`, `POST /api/news/sample`,
`GET /api/econ/events`, `POST /api/econ/import`, `PUT /api/econ/blackout`, `GET|POST /api/workflows`,
`GET|PUT /api/workflows/{id}`, `POST /api/workflows/{id}/status`, `POST /api/workflows/{id}/dryrun`.

Phase 12: `POST /api/accounts/{id}/status`, `GET /api/orchestrator`, `POST /api/orchestrator/{id}/pause-bots`,
`POST|GET /api/smart-orders`, `GET /api/smart-orders/{id}`, `POST /api/smart-orders/{id}/cancel`,
`POST /api/smart-orders/{id}/resume`, `POST /api/options/price`, `POST /api/options/strategy`, `GET /api/ledger`,
`POST /api/ledger/adjustments`, `POST /api/ledger/{id}/reverse`, `GET /api/ledger/report`, `GET /api/ledger/export`,
`POST /api/ledger/reconcile`.

## Events (table `events`, versioned, idempotency key unique)

`MarketDataUpdated`, `MarketRegimeChanged`, `StrategyVersionCreated`, `BacktestCompleted`, `WalkForwardCompleted`,
`RiskLimitBreached`, `PortfolioExposureChanged`, `OrderProposed`, `OrderRiskApproved`, `OrderRiskRejected`,
`OrderSubmitted`, `OrderPartiallyFilled`, `OrderFilled`, `OrderRejected`, `BotStateChanged`, `StrategyDriftDetected`,
`BrokerConnectionChanged`, `DataFeedStale`, `EconomicEventApproaching`, `NewsSentimentChanged`, `WorkflowTriggered`,
`WorkflowActionCompleted`, `ResearchRunCompleted`. Payloads carry ids and figures, never secrets.

## Workers

No new process. The existing bot runner tick now also: fills due smart-order children (`tradeapi::smart_tick`),
evaluates monitor components and incidents, runs drift hourly, announces economic events an hour ahead, and feeds
new events to enabled workflows (`opsapi::housekeeping`). Jobs run on tokio's blocking pool with a cancel flag. On
start: running jobs are marked failed (interrupted), working smart orders are paused, bots are paused (as before).

## Security and risk boundaries (each has a test)

- Every new order path goes through `risk.rs`: smart-order parents are pre-checked and every child passes
  `risk::check`; the calendar blackout, disabled accounts, denied symbols, per-symbol and portfolio limits live in
  `riskintel::extra_checks`, which `check_limits` calls for manual, bot, broker and smart orders alike.
- The researcher, regimes, Monte Carlo, experiments, options lab and explainer have no order path. The workflow
  node catalogue has no order, limit, arming or credential node, and validation refuses those names.
- Workflow runs are idempotent per (workflow, event), rate-limited (30 per hour), and workflow events cannot
  trigger workflows.
- The ledger is append-only in the database itself. CSV export neutralises spreadsheet formulas.
- AI output is labelled "AI interpretation", built only from the fact sheet, and its numbers are checked. Without
  a key the facts still work and the AI call is refused (503).
- Data labels: sample news `SAMPLE (made up)`; smart orders `PAPER (simulated fills)`; options `THEORETICAL`;
  demo accounts `VIRTUAL`; research results carry `source` (sample or exchange data) and engine/rule versions.

## Commands and results (this commit, on Linux in the cloud sandbox)

| Check | Result |
|---|---|
| `cd src-tauri && cargo test --no-default-features` | 203 passed, 0 failed, 0 warnings (baseline 171) |
| `python -m pytest -q` | 31 passed |
| `node tools/ui_smoke.mjs` | 7/7 scenarios passed (new: Research Lab and Operations pages, 375 px with both pages) |
| `node tools/live_check.mjs` | not run: the sandbox has no route to Binance |

New tests: unit tests in `events`, `jobs`, `monitor`, `riskintel` (including 8 concurrent threads against one
aggregate limit), `attribution`, `drift`, `strategies`, `regime` (no future data), `research` (reproducible seeds,
walk-forward folds, robustness needs 20 trades), `researcher`, `intel`, `explain`, `workflows`, `smartexec`
(partial fills, rejection, cancel, restart), `options` (textbook values, parity, implied vol), `ledger`
(immutability, reversal, India FY, reconciliation); HTTP tests `phase9_operations_over_http`,
`phase10_research_jobs_over_http`, `phase11_intelligence_over_http`, `phase12_trading_ops_over_http`; route
coverage and OpenAPI tests extended to every new route.

## Environment variables

No new variables. Existing names only: `SAMPLE_DATA`, `ZONEBOARD_DATA`, `JOURNAL_DB`, `PORT`. The AI explainer
uses the Anthropic key the owner stores in the OS keychain (never an environment variable, never logged).

## Live versus mock

| Area | What runs |
|---|---|
| Market data for research | Binance public klines (live) or sample data (`SAMPLE_DATA=1`, labelled) |
| Smart orders | Paper fills at the latest price; no broker routing |
| News, calendar | Owner imports (manual provider) and labelled sample items; no live provider |
| Options | Black-Scholes on owner inputs; no chain or quotes |
| Statement reconciliation | Generic CSV (date, symbol, side, qty, price, fee, ref); broker-specific formats need samples |
| Live trading | Unchanged: armed + approved + limits; never exercised by tests or agents |

## Known limitations

- Attribution by market regime is not a dimension yet: regimes are recorded per symbol and bar, but trades are not
  joined to them automatically (the trade explainer shows the nearest regime).
- A/B testing compares two variants on the same historical window; a forward paper A/B is not built.
- The workflow builder edits the graph as JSON with validation and dry-run; a drag-and-drop canvas is not built.
- Smart execution is paper only and fills a whole child at the latest price (no order book simulation).
- Sentiment is a word-list heuristic (`lexicon-v1`) with a confidence; it is not a language model.
- Workspaces are blocked (above).

## What the owner can decide or provide

1. Workspaces: keep ZoneBoard single-user (recommended) or approve a multi-user design (login, network exposure).
2. A news or calendar provider (name, licence, API key) if live feeds are wanted; until then, imports work.
3. A sample broker statement (with personal data removed) to add a broker-specific import format.
