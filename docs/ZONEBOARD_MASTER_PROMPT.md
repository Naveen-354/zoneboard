# ZoneBoard --- Master Claude Code Implementation Prompt

## Complete End-to-End Development Across All Phases

**Document purpose:** This is the master execution prompt for Claude
Code. It directs Claude Code to inspect, plan, implement, test, document
and complete the full ZoneBoard roadmap---not merely produce an
architecture proposal.

**Product:** ZoneBoard\
**Product type:** Personal AI-assisted quantitative trading and research
platform\
**Markets:** Cryptocurrency through Binance and Indian markets through
FYERS\
**AI model:** Hybrid human-approved AI\
**Scope:** Personal trading workstation + quantitative research
laboratory\
**Implementation owner:** Claude Code only

------------------------------------------------------------------------

# MASTER INSTRUCTION --- READ BEFORE STARTING

You are the **Principal Software Architect, Senior Full-Stack Engineer,
Quantitative Trading Systems Engineer, AI Systems Engineer, QA Engineer
and DevOps Engineer** responsible for completing the ZoneBoard
application.

Your assignment is to evolve the existing ZoneBoard repository into a
professional, reliable, AI-assisted quantitative trading and research
platform.

This is an implementation assignment, not a request to write another
proposal.

You must:

1.  Inspect and understand the existing repository.
2.  Preserve working functionality.
3.  Create a complete implementation plan based on the real codebase.
4.  Implement every phase in this document, in order.
5.  Test and verify each phase before proceeding.
6.  Continue through all phases without stopping after the audit or
    planning stage.
7.  Fix implementation defects rather than leaving known failures
    behind.
8.  Maintain progress and technical documentation.
9.  Never claim a feature is complete unless its actual behavior has
    been implemented and tested.
10. Use Claude Code as the sole implementation agent. Do not delegate
    coding to Antigravity, other agents, or external coding agents.

Do not wait for approval between phases. You have authorization to
execute the full roadmap. Stop and ask only when an action requires
unavailable credentials, an irreversible destructive operation, a paid
external service, a legal/compliance decision, or explicit authorization
to use real money.

**Important live-trading restriction:** You may build and test
live-trading capabilities, but do not enable, initiate or place
real-money orders without the user's explicit authorization. Never use
real funds for testing.

------------------------------------------------------------------------

# 1. PRODUCT VISION

Build ZoneBoard as a unified platform where the user can:

-   Connect Binance and FYERS accounts.
-   View crypto and NSE markets.
-   Monitor charts, indicators, watchlists and market conditions.
-   Discover, create and configure algorithmic trading bots.
-   Build reusable trading strategies.
-   Backtest strategies against historical data.
-   Replay historical markets.
-   Run strategies in paper/demo mode.
-   Evaluate strategies using realistic costs and risk metrics.
-   Use AI to research markets and generate strategy proposals.
-   Review and approve strategy deployments.
-   Run approved bots through broker integrations.
-   Monitor orders, fills, positions, balances and portfolio
    performance.
-   Manage risk centrally.
-   Develop custom indicators, strategies and integrations.

ZoneBoard must distinguish four environments:

-   `BACKTEST`
-   `REPLAY`
-   `PAPER`
-   `LIVE`

These environments must be isolated in backend logic, credentials,
configuration and execution. A UI label alone is not a security
boundary.

## Product principles

-   Correctness before feature quantity.
-   Real functionality, not placeholder screens.
-   Reproducible research and backtests.
-   Accurate financial calculations.
-   No fabricated balances, market data, orders, fills or P&L.
-   Reusable strategies independent of broker execution.
-   Centralized risk validation for every order.
-   Human authorization for live deployment.
-   Secure credentials and OAuth handling.
-   Observable, recoverable and auditable operations.
-   Preserve existing application conventions unless a documented reason
    justifies changing them.

------------------------------------------------------------------------

# 2. WORKING RULES AND AUTONOMY

## 2.1 Repository-first development

Before editing:

-   Inspect the complete repository structure.
-   Identify frontend and backend frameworks and versions.
-   Inspect database schema and migrations.
-   Inspect existing account/profile and connection management.
-   Inspect Binance and FYERS integrations.
-   Inspect demo/live trading implementation.
-   Inspect Bot Studio, strategy code, market data, orders and portfolio
    logic.
-   Inspect tests, deployment configuration, environment variables and
    documentation.
-   Run existing tests and record the baseline.
-   Identify incomplete, broken and duplicated implementations.

Do not assume the application uses any technology not found in the
repository.

## 2.2 Implementation ownership

Claude Code owns:

-   Architecture decisions.
-   Task breakdown.
-   Code implementation.
-   Database migrations.
-   API changes.
-   UI development.
-   Tests.
-   Debugging.
-   Code review.
-   Documentation.
-   Final verification.

Do not use Antigravity or another coding agent. Do not create a workflow
that depends on another agent consuming credits or usage.

## 2.3 Work continuously

-   Implement all phases sequentially.
-   Do not stop after generating plans or documentation.
-   Do not ask the user to approve routine technical decisions.
-   Make reasonable, reversible decisions consistent with the existing
    stack.
-   Record important decisions in architecture decision records.
-   If a dependency is unavailable, implement the parts that can be
    completed and document the exact blocker.
-   Do not silently skip a requirement.
-   Do not mark a phase complete while its acceptance criteria are
    failing.

## 2.4 Safe handling of existing data

-   Never drop or truncate existing production/user data as a
    convenience.
-   Use backward-compatible migrations where practical.
-   Provide migration and rollback notes.
-   Back up data before any potentially destructive migration.
-   Do not overwrite user configuration or broker connections.
-   Do not print secrets in terminal output, logs or reports.

## 2.5 Progress tracking

Create and maintain:

``` text
docs/
  ZONEBOARD_MASTER_PROMPT.md
  architecture/
    current-state.md
    target-state.md
    decisions/
  roadmap/
    implementation-plan.md
    feature-inventory.md
    progress.md
  testing/
    test-strategy.md
    test-results.md
  operations/
    runbooks.md
```

In `progress.md`, track each feature as:

-   `NOT_STARTED`
-   `IN_PROGRESS`
-   `BLOCKED`
-   `IMPLEMENTED`
-   `TESTED`
-   `VERIFIED`

Update progress after every meaningful task. A feature is complete only
after implementation and verification.

------------------------------------------------------------------------

# 3. TARGET ARCHITECTURE

Use the existing application's stack and architecture where reasonable.
Prefer a modular monolith with isolated background workers initially.
Extract services only when there is a concrete reliability, security,
scaling or operational reason.

Logical modules:

1.  Frontend application.
2.  Core backend and API.
3.  Identity, profiles and permissions.
4.  Broker connection adapters.
5.  Unified market data.
6.  Strategy and indicator engine.
7.  Quantitative research and backtesting.
8.  Bot manager and runtime.
9.  Central risk engine.
10. Order and execution manager.
11. AI research orchestrator.
12. Approval workflow.
13. Portfolio and reporting.
14. Market intelligence and scanners.
15. Scheduler, event processing and notifications.
16. Developer SDK and extension system.

Suggested logical flow:

``` text
Binance ─────┐
             ├── Broker / Market Data Adapters
FYERS ───────┘              │
                            ▼
                  Normalized Market Data
                            │
             ┌──────────────┴──────────────┐
             ▼                             ▼
       Strategy Engine                Trading Terminal
             │
             ▼
        Signal Engine
             │
             ▼
       Central Risk Engine
             │
             ▼
       Order / Execution Manager
             │
             ▼
       Binance / FYERS

Historical Data ──► Backtesting / Replay / Optimization

AI Research ──► Draft / Review / Proposal
                         │
                         ▼
                  Human Approval
                         │
                         ▼
                Authorized Deployment
```

Use PostgreSQL as the transactional source of truth if it fits the
existing stack. Redis may support caching, locks and coordination, but
must not be the only store for orders, fills, approvals or other
critical financial state. Use a durable queue/event mechanism
appropriate to the actual workload.

------------------------------------------------------------------------

# 4. GLOBAL DOMAIN AND ENGINEERING REQUIREMENTS

## 4.1 Canonical domain models

Create or adapt normalized models for:

-   User and trading profile.
-   Trading account.
-   Broker connection.
-   Instrument.
-   Market candle and market event.
-   Strategy and strategy version.
-   Indicator definition.
-   Bot and bot configuration version.
-   Bot runtime instance.
-   Signal.
-   Risk profile and risk decision.
-   Order.
-   Fill.
-   Position.
-   Balance.
-   Backtest run and metrics.
-   Replay session.
-   Optimization run.
-   AI session and proposal.
-   Approval request and decision.
-   Audit event.
-   Notification.

Use decimal-safe types for prices, quantities, balances, fees and P&L.
Avoid binary floating point for financial values. Store timestamps in
UTC and preserve exchange/session timezone information where required.

## 4.2 Broker adapter contract

Create a consistent internal adapter interface for supported provider
capabilities:

``` python
class BrokerAdapter:
    def connect(self, credentials): ...
    def verify_connection(self): ...
    def get_account_info(self): ...
    def get_balances(self): ...
    def get_positions(self): ...
    def get_instruments(self): ...
    def get_historical_data(self, instrument, timeframe, start, end): ...
    def subscribe_market_data(self, instruments, callback): ...
    def place_order(self, order_request, idempotency_key): ...
    def get_order(self, broker_order_id): ...
    def list_orders(self, filters=None): ...
    def cancel_order(self, broker_order_id): ...
    def close(self): ...
```

Adapt the contract to the actual official Binance and FYERS APIs. Expose
capability metadata. Never pretend a broker supports an operation,
market, order type or data feed that it does not support for the user's
account and region.

## 4.3 Strategy/execution separation

A strategy evaluates data and emits a typed signal. It must not call a
broker directly.

``` text
Market Data → Indicators → Strategy → Signal
                                      ↓
                                  Risk Engine
                                      ↓
                               Execution Manager
                                      ↓
                                Broker Adapter
```

The same strategy version should be usable by backtest, replay and paper
engines. Live use requires additional broker compatibility checks and
human approval.

## 4.4 Execution environment isolation

-   Backtest and replay must never load live credentials.
-   Paper execution must use a simulator/paper adapter.
-   Live execution must resolve an explicitly authorized account and
    connection.
-   The backend must determine environment from trusted persisted
    configuration.
-   Workers must not accept a user-controlled environment flag as
    sufficient authorization.
-   Live deployment approvals must identify the exact strategy version,
    account, instruments, risk configuration and capital allocation.

------------------------------------------------------------------------

# 5. COMPLETE FEATURE IMPLEMENTATION SCOPE

## 5.1 Professional Trading Profile and Connections

Implement or improve:

-   User profile and preferences.
-   Multiple named trading accounts.
-   Separate demo/paper and live accounts.
-   Binance API key/secret connection.
-   FYERS OAuth authorization and callback flow.
-   Connection verification and health status.
-   Token expiry and refresh handling where supported.
-   Permission/capability display.
-   Connection test and disconnect/revoke.
-   Last successful synchronization time.
-   Broker balances, positions and account details.
-   Base currency and timezone settings.
-   Notification preferences.
-   Secure secret storage and redaction.

Security requirements:

-   Never store API secrets in plaintext.
-   Prefer a secrets manager or envelope encryption with managed keys.
-   Never request withdrawal permissions for trading keys.
-   Recommend IP restrictions where supported.
-   Never log API keys, access tokens, refresh tokens or OAuth codes.
-   Protect OAuth state and callback validation.
-   Never expose secret values again after initial entry.
-   Handle token expiry and revoked connections gracefully.

## 5.2 Trading Terminal

Implement:

-   Binance crypto and supported FYERS/NSE instrument selection.
-   Candlestick charts and timeframe selection.
-   Volume and supported chart overlays.
-   Watchlists and favorites.
-   Market status and feed health.
-   Order book/depth only where entitled and supported.
-   Manual order ticket.
-   Open orders and order history.
-   Positions and balances.
-   Execution/fill details.
-   Trading journal.
-   Connection and data freshness indicators.
-   Chart-to-research workflow.

All manual orders must pass through the same risk engine as bot orders.

## 5.3 Bot Studio and Bot Catalog

Create a professional bot workspace.

### Bot discovery

Research and provide metadata for widely used frameworks such as:

-   Freqtrade.
-   Hummingbot.
-   Jesse.
-   Superalgos.
-   OctoBot.
-   Gunbot.
-   HaasOnline.

For every entry, show:

-   Name and description.
-   Supported markets/exchanges.
-   Open-source or commercial status.
-   License.
-   Integration status.
-   Documentation link.
-   Compatibility status.
-   Whether it can run natively, through an adapter, or only externally.

Do not claim an external bot is integrated until a real tested
integration exists. Respect licenses and third-party terms.

### Bot creation

Support:

-   Bot name, description and category.
-   Strategy and immutable strategy version.
-   Broker connection.
-   Environment.
-   Instrument universe.
-   Timeframe.
-   Capital allocation.
-   Position sizing.
-   Risk profile.
-   Execution settings.
-   Trading schedule.
-   Notification preferences.
-   Versioned configuration.

### Bot lifecycle

Implement and enforce a backend state machine:

``` text
DRAFT
  ↓
VALIDATING
  ↓
BACKTESTED
  ↓
PAPER
  ↓
PENDING_APPROVAL
  ↓
LIVE
  ↓
PAUSED / STOPPING
  ↓
STOPPED / ERROR
```

Invalid state transitions must be rejected and audited.

### Bot monitoring

Display:

-   Runtime state and uptime.
-   Strategy version.
-   Broker and environment.
-   Capital allocation.
-   Current positions.
-   Orders and fills.
-   Realized and unrealized P&L.
-   Drawdown.
-   Win rate and profit factor with sample size/date range.
-   Logs and errors.
-   Start, pause, resume, stop and emergency-stop controls.

Worker restart must not duplicate orders.

## 5.4 Strategy Builder and Indicator Engine

Implement a reusable indicator library, initially including:

-   SMA, EMA, WMA.
-   RSI.
-   MACD.
-   Bollinger Bands.
-   ATR.
-   ADX.
-   Stochastic and Stochastic RSI.
-   CCI. 
-   VWAP where data supports correct calculation.
-   OBV.
-   Donchian Channel.
-   Supertrend.
-   Ichimoku Cloud.
-   ROC.
-   Z-score.
-   Volume moving average.

For each indicator document formula, parameters, warm-up period,
missing-data behavior and timestamp alignment. Add reference-value
tests.

Strategy builder must support:

-   AND/OR/NOT condition groups.
-   Cross above/below.
-   Greater/less/equal comparisons.
-   Price, volume and indicator conditions.
-   Time/session filters.
-   Entry and exit rules.
-   Stop-loss and take-profit.
-   Position sizing.
-   Long/short only where supported.
-   Reusable templates.
-   Parameter validation.
-   Strategy version history.
-   Import/export.

Published strategy versions must be immutable. Editing creates a new
version.

If custom code is supported, execute it in an isolated, resource-limited
environment. Do not execute arbitrary strategy code in the main backend
process.

## 5.5 Strategy Lab and Backtesting

Implement a reproducible event-driven backtesting engine.

Features:

-   Multiple instruments and timeframes.
-   Long/short where supported.
-   Configurable initial capital.
-   Position sizing and compounding.
-   Fees, spread and slippage.
-   Relevant taxes/levies for Indian markets where applicable.
-   Funding/borrow costs where applicable.
-   Partial fills and rejected-order simulation.
-   Market sessions and calendars.
-   Benchmark comparison.
-   Equity curve and drawdown.
-   Trade-by-trade results.
-   Exportable reports.

Metrics:

-   Gross and net P&L.
-   Return percentage.
-   Trade count.
-   Win rate.
-   Average win/loss.
-   Profit factor.
-   Expectancy.
-   Maximum drawdown.
-   Sharpe and Sortino with documented annualization.
-   Exposure and time in market.
-   Fees and slippage.
-   Consecutive wins/losses.
-   Benchmark-relative performance.

Validation:

-   In-sample/out-of-sample split.
-   Walk-forward testing.
-   Parameter sensitivity.
-   Market-regime segmentation.
-   Monte Carlo resampling.
-   Bootstrap intervals where appropriate.
-   Look-ahead and data-leakage checks.
-   Survivorship and selection-bias warnings.

Every run must store the dataset/version, strategy version, parameters,
engine version, cost model, date range, random seed where relevant,
warnings and output artifacts. Results must be reproducible.

## 5.6 Market Replay

Implement:

-   Instrument and date-range selection.
-   Play, pause, step forward/back and speed control.
-   Historical candles and supported events.
-   Strategy execution during replay.
-   Signal/order/fill visualization.
-   Multiple strategy comparison.
-   Reset and export.

Replay must not expose future data to a strategy. Playback speed must
not alter logical results.

## 5.7 Centralized Risk Engine

All manual, bot and AI-originated order requests must pass through this
mandatory gate.

Controls:

-   Maximum order notional.
-   Maximum position size.
-   Maximum bot allocation.
-   Maximum instrument/asset/sector allocation.
-   Gross/net exposure.
-   Daily loss limit.
-   Portfolio drawdown limit.
-   Maximum open orders and concurrent positions.
-   Consecutive-loss pause.
-   Session restrictions.
-   Instrument allow/deny list.
-   Price deviation checks.
-   Tick/lot/quantity validation.
-   Broker health.
-   Data freshness.
-   Duplicate-order protection.
-   Emergency kill switch.

Risk evaluation:

1.  Resolve trusted account and environment.
2.  Check live approval when applicable.
3.  Verify connection and account status.
4.  Verify instrument and market support.
5.  Verify data freshness.
6.  Normalize price and quantity to broker rules.
7.  Calculate current and post-order exposure.
8.  Evaluate account, bot and portfolio limits.
9.  Check idempotency and duplicate requests.
10. Persist decision and structured reason.
11. Only then permit execution.

Risk failure must fail closed for new live orders.

Define stop policies explicitly: stop new entries, cancel pending
entries, retain protective orders, or flatten positions only when
specifically configured and supported. A normal stop must not silently
liquidate positions.

## 5.8 AI Research Orchestrator

Implement a hybrid AI system with restricted tools.

Capabilities:

-   Market analysis based on verified data.
-   Strategy idea generation.
-   Natural-language strategy drafting.
-   Indicator and parameter recommendations.
-   Strategy code/specification review.
-   Backtest result explanation.
-   Risk analysis.
-   Bot performance explanation.
-   Research report generation.
-   Proposed strategy improvements.

Suggested agent roles may be implemented as prompt/tool profiles in one
orchestrator:

1.  Market Analyst.
2.  Strategy Researcher.
3.  Strategy Developer.
4.  Strategy Reviewer.
5.  Backtest Analyst.
6.  Risk Analyst.
7.  Trading Assistant.

AI workflow:

1.  Receive user objective.
2.  Gather authorized, timestamped context.
3.  Produce a structured strategy proposal.
4.  Validate schema and parameter bounds.
5.  Explain assumptions and unresolved questions.
6.  Create a draft.
7.  Run static checks and tests.
8.  Request backtest.
9.  Explain actual stored results.
10. Request paper test.
11. Submit a live deployment proposal for human review.

AI must never:

-   Approve its own proposal.
-   Enable live trading.
-   Create/reveal broker secrets.
-   Increase account risk limits.
-   Bypass risk validation.
-   Place a live order through a direct tool path.
-   Invent metrics or market facts.

Treat external pages, news, documents and bot descriptions as untrusted
data. Defend against prompt injection. Log provider/model, prompt
version, tool calls and proposals. Provide AI disable controls and
cost/timeout limits.

## 5.9 Human Approval Center

Approval requests must include:

-   Requester.
-   Action type.
-   Account and environment.
-   Instrument scope.
-   Exact strategy version.
-   Configuration diff.
-   Risk profile.
-   Capital allocation.
-   Backtest and paper-test evidence.
-   Warnings.
-   Expiration.
-   Approver and decision.

Approval applies only to the exact reviewed version and scope. Any
material change invalidates it. Rejection, expiry and revocation must be
audited.

## 5.10 Market Intelligence and Scanners

Implement configurable scanners for supported data:

-   Price and percentage movement.
-   Relative volume and volume spikes.
-   Breakout/breakdown candidates.
-   Volatility and ATR changes.
-   Trend and momentum.
-   NSE gap scanner where data supports it.
-   Liquidity/spread filters where data is available.
-   Correlation and relative strength.
-   Crypto funding/open interest where supported.
-   Derivatives/options analytics only when the provider account has the
    required data entitlement.
-   News and economic calendar only from reliable sources.

Every result must show source, timestamp, universe, formula/threshold
and data freshness. Scanner results may create research tasks but must
not bypass approval or risk controls.

## 5.11 Portfolio and Performance

Implement:

-   Unified broker balances.
-   Available cash and margin where supported.
-   Positions and open orders.
-   Realized/unrealized P&L.
-   Fees, taxes and funding costs where available.
-   Allocation by broker, asset, sector and strategy.
-   Bot and strategy attribution.
-   Drawdown and exposure.
-   Equity curves.
-   Daily/weekly/monthly reports.
-   CSV export and transaction reports.

Broker-reported balances are authoritative for live accounts. Paper
balances must remain separate. Reconcile broker snapshots and show
discrepancies instead of hiding them. Currency conversion must record
rate source and timestamp.

## 5.12 Developer Hub

Implement:

-   Strategy SDK.
-   Custom indicator interface.
-   Scoped API tokens.
-   Webhooks and event subscriptions.
-   Strategy import/export.
-   Plugin manifest and compatibility validation.
-   Examples and documentation.
-   Versioned API contracts.
-   Sandboxed developer environment.

Custom code must not access broker secrets or unrestricted
infrastructure.

## 5.13 Alerts, Journal and Notifications

Implement:

-   Bot started/paused/stopped/error alerts.
-   Broker disconnected/token-expired alerts.
-   Stale market data alerts.
-   Order rejected/partially filled/filled alerts.
-   Risk limit and drawdown alerts.
-   Daily performance summaries.
-   Strategy/backtest completion alerts.
-   Trading journal entries linked to orders, strategies and bots.
-   User-configurable notification channels supported by the existing
    application.

Notifications must not reveal secrets or sensitive account data.

------------------------------------------------------------------------

# 6. DATA MODEL AND PERSISTENCE

Adapt the existing schema. Do not create duplicate tables if equivalent
models already exist.

Required logical entities:

### Identity and accounts

-   users
-   user_profiles
-   trading_accounts
-   broker_connections
-   account_snapshots

### Market data

-   instruments
-   market_candles
-   market_events
-   market_data_jobs
-   data_quality_events

### Research

-   strategies
-   strategy_versions
-   indicator_definitions
-   backtest_runs
-   backtest_metrics
-   backtest_trades
-   replay_sessions
-   optimization_runs
-   research_datasets

### Bots and execution

-   bots
-   bot_config_versions
-   bot_runtime_instances
-   signals
-   orders
-   fills
-   positions
-   execution_events

### Risk and approvals

-   risk_profiles
-   risk_events
-   approval_requests
-   approval_decisions

### AI and audit

-   ai_sessions
-   ai_proposals
-   ai_tool_events
-   audit_events

### User experience

-   watchlists
-   watchlist_items
-   alerts
-   notification_preferences
-   trading_journal_entries

Use foreign keys, unique constraints, indexes and appropriate retention
policies. Use database transactions for critical state transitions.
Store large research artifacts and historical datasets outside ordinary
relational rows when appropriate, with references and integrity
metadata.

Secrets must not be stored in plaintext database fields. Use encrypted
secret references or a managed secrets system.

------------------------------------------------------------------------

# 7. API AND EVENT CONTRACTS

Use existing API naming and versioning conventions. Provide OpenAPI
documentation.

Suggested resource groups:

``` text
/api/v1/profiles
/api/v1/accounts
/api/v1/connections
/api/v1/instruments
/api/v1/market-data
/api/v1/watchlists
/api/v1/strategies
/api/v1/indicators
/api/v1/backtests
/api/v1/replays
/api/v1/optimization-runs
/api/v1/bots
/api/v1/orders
/api/v1/positions
/api/v1/risk
/api/v1/approvals
/api/v1/ai
/api/v1/scanners
/api/v1/portfolio
/api/v1/alerts
/api/v1/journal
/api/v1/developer
```

Long-running work must use job IDs and status/progress APIs, not
long-held HTTP requests.

Use stable error codes, server-side validation, pagination, UTC
timestamps, ownership checks, rate limits and idempotency keys for
financial mutations.

Suggested domain events:

``` text
MARKET_DATA_RECEIVED
MARKET_DATA_GAP_DETECTED
MARKET_DATA_STALE
SIGNAL_GENERATED
SIGNAL_REJECTED
RISK_CHECK_PASSED
RISK_CHECK_REJECTED
ORDER_SUBMISSION_REQUESTED
ORDER_ACCEPTED
ORDER_PARTIALLY_FILLED
ORDER_FILLED
ORDER_CANCELLED
ORDER_REJECTED
POSITION_UPDATED
BOT_STARTED
BOT_PAUSED
BOT_STOPPED
BOT_ERROR
BACKTEST_REQUESTED
BACKTEST_COMPLETED
BACKTEST_FAILED
AI_PROPOSAL_CREATED
APPROVAL_REQUESTED
APPROVAL_GRANTED
APPROVAL_REJECTED
LIVE_DEPLOYMENT_AUTHORIZED
```

Assume events may be delivered more than once. Consumers must be
idempotent. Use an outbox pattern where database changes and event
publishing must remain consistent. Never blindly retry an order
submission after an uncertain timeout; reconcile with the broker first.

------------------------------------------------------------------------

# 8. PHASED IMPLEMENTATION --- COMPLETE ALL PHASES

Execute phases in this order. The phases are sequential dependencies,
not optional suggestions. You must continue to Phase 7 after earlier
phases pass their acceptance criteria.

## PHASE 0 --- REPOSITORY AUDIT AND BASELINE

### Objective

Understand the real codebase and establish a safe implementation plan.

### Tasks

1.  Inspect frontend, backend, database, deployment and tests.
2.  Map profiles, accounts, demo/live modes and permissions.
3.  Map Binance and FYERS connection and authentication flows.
4.  Map market data and historical data storage.
5.  Map trading terminal, orders, fills and positions.
6.  Map Bot Studio and strategy implementation.
7.  Identify current risk checks and execution boundaries.
8.  Identify existing queues, workers, schedulers and WebSockets.
9.  Review secrets, authorization, logging and deployment security.
10. Run existing tests, builds, lint and type checks.
11. Create complete feature inventory: existing, partial, missing,
    broken.
12. Produce current-state and target-state diagrams.
13. Identify migration and compatibility risks.
14. Create phase-by-phase task backlog.

### Deliverables

-   Current-state architecture.
-   Target-state architecture.
-   Feature inventory.
-   Test baseline.
-   Architecture decisions.
-   Prioritized implementation backlog.
-   Progress tracker.

### Exit criteria

-   Existing functionality is mapped.
-   Baseline build/test results are recorded.
-   Implementation plan reflects the actual repository.
-   No unnecessary rewrite is proposed.

**After Phase 0 passes, continue automatically to Phase 1.**

## PHASE 1 --- MARKET DATA AND BROKER FOUNDATION

### Objective

Build reliable normalized provider connections and market data.

### Tasks

1.  Review official current Binance and FYERS documentation.
2.  Define instrument and provider capability models.
3.  Harden existing Binance connection.
4.  Harden existing FYERS OAuth and token lifecycle.
5.  Implement connection health and verification.
6.  Normalize symbols, timestamps, quantities, prices and timeframes.
7.  Implement live WebSocket lifecycle and reconnect behavior.
8.  Implement historical candle ingestion.
9.  Detect duplicate, missing, stale and out-of-order data.
10. Add historical backfill and repair jobs.
11. Add rate-limit handling and subscription management.
12. Add source and completeness metadata.
13. Add market-data health UI.
14. Add profile connection management and safe credential handling.

### Tests

-   Broker adapter contract tests.
-   OAuth state and callback tests.
-   Token expiry and revoked credential tests.
-   Timestamp/timezone tests.
-   Duplicate/gap/out-of-order data tests.
-   Reconnect and stale-feed tests.
-   Rate-limit and provider failure tests.
-   Historical/live consistency tests.

### Deliverables

-   Reliable Binance and FYERS adapters.
-   Normalized instrument and market-data APIs.
-   Historical data storage.
-   Connection settings UI.
-   Feed health dashboard.
-   Data quality reports.

### Exit criteria

-   Supported feeds work reliably.
-   Data source and freshness are visible.
-   Missing data is detected.
-   No credentials leak.
-   Existing broker flows still pass regression tests.

## PHASE 2 --- STRATEGY ENGINE AND QUANTITATIVE RESEARCH

### Objective

Create reusable strategies and reliable research/backtesting.

### Tasks

1.  Define strategy input/output contracts.
2.  Implement indicator library.
3.  Add indicator reference tests.
4.  Implement strategy condition evaluator.
5.  Add entry, exit and position sizing abstractions.
6.  Add strategy templates and builder.
7.  Implement immutable strategy versioning.
8.  Build event-driven backtesting.
9.  Model fees, spread, slippage and relevant costs.
10. Model instrument rules and market sessions.
11. Implement trade and portfolio metrics.
12. Add in-sample/out-of-sample validation.
13. Add walk-forward testing.
14. Add parameter sensitivity and regime analysis.
15. Store complete experiment metadata.
16. Build Strategy Lab UI and reports.

### Tests

-   Indicator golden tests.
-   Warm-up and missing-data tests.
-   No-look-ahead tests.
-   Determinism tests.
-   Fee/slippage tests.
-   Position sizing and rounding tests.
-   Partial-fill simulation tests.
-   Metric calculation tests.
-   Dataset/version reproducibility tests.

### Deliverables

-   Strategy engine.
-   Indicator library.
-   Strategy Builder.
-   Strategy registry and version history.
-   Backtest engine and worker.
-   Strategy Lab dashboard.
-   Experiment history and reports.

### Exit criteria

-   Strategies are reusable across simulation modes.
-   Backtest assumptions are visible.
-   Results are reproducible.
-   No future data leakage occurs.

## PHASE 3 --- BOT RUNTIME, PAPER TRADING AND RISK ENGINE

### Objective

Operate bots safely in paper mode with centralized risk controls.

### Tasks

1.  Implement bot state machine.
2.  Implement persistent bot configurations and versions.
3.  Implement worker registration and heartbeats.
4.  Implement bot scheduling and lifecycle APIs.
5.  Connect signals to risk engine.
6.  Implement account, bot and portfolio risk limits.
7.  Implement paper broker/simulator.
8.  Simulate orders, fills, fees and positions.
9.  Implement idempotent order processing.
10. Implement order recovery and reconciliation.
11. Implement safe stop and emergency stop.
12. Build bot performance and logs dashboard.
13. Add execution health monitoring.
14. Test recovery after worker restart and network interruption.

### Tests

-   Invalid lifecycle transitions.
-   Worker restart and recovery.
-   Duplicate event/order protection.
-   Risk boundary tests.
-   Stale data rejection.
-   Partial-fill and timeout tests.
-   Kill-switch tests.
-   Paper/live credential isolation.
-   Order reconciliation.

### Deliverables

-   Bot runtime.
-   Paper execution adapter.
-   Central risk engine.
-   Bot operations dashboard.
-   Execution event history.
-   Operational runbooks.

### Exit criteria

-   Bots run reliably in paper mode.
-   Restart does not duplicate orders.
-   Every order path passes risk validation.
-   Risk engine failure blocks new orders.
-   Stop behavior is predictable and tested.

## PHASE 4 --- AI RESEARCH AND HUMAN APPROVAL

### Objective

Implement hybrid AI assistance with strict live-trading governance.

### Tasks

1.  Implement AI provider/model abstraction.
2.  Add model and prompt versioning.
3.  Implement restricted AI tool permissions.
4.  Build verified market research context.
5.  Build natural-language strategy draft generator.
6.  Convert outputs to typed strategy schemas.
7.  Validate indicators, instruments and parameter bounds.
8.  Implement strategy reviewer.
9.  Implement backtest result explainer using stored metrics.
10. Implement risk analysis assistant.
11. Implement AI proposal storage and lifecycle.
12. Build human Approval Center.
13. Bind approvals to exact strategy version/account/scope.
14. Add audit logging of AI actions and tool calls.
15. Add prompt-injection defenses.
16. Add model timeout, error and usage controls.

### Tests

-   Invalid schema rejection.
-   Unauthorized tool rejection.
-   AI self-approval prevention.
-   Secret access prevention.
-   Direct live-order prevention.
-   Prompt injection tests.
-   Stored-metric-only explanation tests.
-   Approval scope/version mismatch tests.

### Deliverables

-   AI Research workspace.
-   AI strategy generator and reviewer.
-   Backtest explanation.
-   Risk assistant.
-   Proposal workflow.
-   Human Approval Center.
-   AI audit records.

### Exit criteria

-   AI can research, draft and recommend.
-   AI cannot authorize live execution.
-   Metrics come from actual stored results.
-   Approval is exact, scoped and revocable.
-   AI failures cannot weaken safety controls.

## PHASE 5 --- MARKET INTELLIGENCE AND SCANNERS

### Objective

Provide market discovery and configurable scanning.

### Tasks

1.  Build configurable scanner framework.
2.  Implement price and percentage movement.
3.  Implement relative-volume scans.
4.  Implement breakout/breakdown candidates.
5.  Implement volatility and ATR scans.
6.  Implement trend and momentum filters.
7.  Add NSE gap scans where supported.
8.  Add liquidity/spread filters where data exists.
9.  Add correlation and relative strength.
10. Add crypto derivatives metrics where supported.
11. Add derivatives/options analytics only where entitled data exists.
12. Add reliable news/calendar sources if needed.
13. Add source timestamp and freshness indicators.
14. Allow scan results to create research experiments.
15. Add alerts and scan history.

### Tests

-   Formula and threshold tests.
-   Instrument-universe tests.
-   Stale-data handling.
-   Provider capability tests.
-   Alert deduplication.
-   Missing entitlement behavior.

### Deliverables

-   Market Scanner.
-   Market Intelligence dashboard.
-   Configurable scan templates.
-   Alert rules and history.

### Exit criteria

-   Every result shows source, timestamp and formula.
-   Unsupported features are clearly identified.
-   Scanner results cannot bypass risk and approval.

## PHASE 6 --- PORTFOLIO, OPTIMIZATION AND ADVANCED ANALYTICS

### Objective

Provide reliable portfolio insight and robust strategy optimization.

### Tasks

1.  Implement broker balance/position reconciliation.
2.  Consolidate manual and bot trades.
3.  Calculate portfolio allocation and exposure.
4.  Implement strategy and bot attribution.
5.  Implement drawdown and risk-adjusted metrics.
6.  Implement correlation analysis.
7.  Implement Monte Carlo simulation.
8.  Implement parameter sensitivity.
9.  Add genetic and Bayesian optimization as experiment types where
    justified.
10. Add objective functions and constraints.
11. Add overfitting and multiple-testing warnings.
12. Generate capital allocation proposals, not automatic changes.
13. Add daily/weekly/monthly reports.
14. Add exportable reports and transaction data.

### Tests

-   Broker snapshot reconciliation.
-   Currency conversion timestamp.
-   P&L and fee attribution.
-   Correlation edge cases.
-   Monte Carlo reproducibility.
-   Train/test separation.
-   Exposure calculations.
-   Portfolio aggregation.

### Deliverables

-   Portfolio dashboard.
-   Performance attribution.
-   Optimization workspace.
-   Monte Carlo reports.
-   Capital allocation proposals.
-   Exportable reports.

### Exit criteria

-   Broker balances reconcile or discrepancies are visible.
-   Performance is traceable to actual fills.
-   Optimization includes out-of-sample validation.
-   AI cannot silently alter capital allocation.

## PHASE 7 --- DEVELOPER HUB, SDK AND EXTENSIBILITY

### Objective

Enable custom algorithm development without compromising platform
security.

### Tasks

1.  Define Python strategy SDK or the best fit for the existing stack.
2.  Define custom indicator interface.
3.  Build isolated strategy execution.
4.  Add typed API client.
5.  Add scoped API keys and rate limits.
6.  Add webhooks and event subscriptions.
7.  Add strategy import/export.
8.  Add plugin manifest and compatibility checks.
9.  Add developer examples and documentation.
10. Add dependency pinning and security scanning.
11. Add developer sandbox and fixtures.
12. Add API versioning and compatibility tests.

### Tests

-   SDK contract tests.
-   Sandbox isolation tests.
-   CPU/memory/time limits.
-   API permission tests.
-   Dependency vulnerability checks.
-   Import/export compatibility tests.

### Deliverables

-   Developer Hub.
-   Strategy SDK.
-   Custom indicator interface.
-   Developer documentation.
-   Extension/plugin interface.
-   Scoped API access.

### Exit criteria

-   Custom strategies can be developed without changing core modules.
-   Custom code cannot access secrets or unrestricted infrastructure.
-   API tokens are scoped, revocable and audited.

------------------------------------------------------------------------

# 9. UI / NAVIGATION REQUIREMENTS

Preserve the existing ZoneBoard visual language and improve consistency.

Suggested navigation:

``` text
ZONEBOARD
├── MARKETS
│   ├── Trading Terminal
│   ├── Market Scanner
│   ├── Market Intelligence
│   └── Economic Calendar
├── ALGORITHMIC TRADING
│   ├── Bot Studio
│   ├── Strategy Builder
│   ├── Active Bots
│   └── Strategy Catalog
├── QUANTITATIVE RESEARCH
│   ├── Strategy Lab
│   ├── Backtesting
│   ├── Market Replay
│   └── Optimization
├── AI INTELLIGENCE
│   ├── AI Research
│   ├── AI Assistant
│   └── Approval Center
├── PORTFOLIO & CONTROL
│   ├── Portfolio
│   ├── Risk Center
│   ├── Trading Journal
│   └── Alerts
└── SYSTEM
    ├── Connections
    ├── Developer Hub
    └── Settings
```

UI requirements:

-   Make BACKTEST, REPLAY, PAPER and LIVE visually unmistakable.
-   Display broker, account and environment context.
-   Show market data source and freshness.
-   Use clear loading, empty, stale, disconnected and error states.
-   Confirm financially consequential actions.
-   Show strategy version, account, instrument, quantity and risk
    summary before live deployment.
-   Keep risk warnings visible.
-   Support responsive layouts and accessible contrast.
-   Do not add navigation items for features that have no working
    implementation unless clearly marked as planned.

------------------------------------------------------------------------

# 10. SECURITY AND LIVE-TRADING RELEASE GATES

These are mandatory requirements.

## Credentials

-   Encrypt secrets at rest.
-   Use least-privilege API permissions.
-   Disable withdrawal permissions for trading credentials.
-   Use IP restrictions where supported.
-   Redact credentials from logs and telemetry.
-   Support credential revocation and rotation.
-   Protect OAuth state and callbacks.

## Execution

-   Every order goes through risk validation.
-   Use idempotency keys.
-   Reconcile uncertain order submissions before retry.
-   Respect provider rate limits and instrument rules.
-   Reject stale market data for new entries.
-   Handle broker outages and exchange maintenance.
-   Define protective-order behavior.
-   Test process and network failure recovery.

## Live deployment checklist

A strategy can be activated live only if:

-   Exact strategy version is selected.
-   Backtest is complete and reviewed.
-   Paper test meets configured criteria.
-   Broker connection is healthy.
-   Permissions and instrument support are verified.
-   Risk profile is assigned.
-   Capital allocation is within limits.
-   Market data is healthy.
-   No unresolved critical execution issue exists.
-   User explicitly approves.
-   Approval is current and not revoked.

Do not enable real-money execution during development or tests. Do not
place a live order without explicit user authorization.

------------------------------------------------------------------------

# 11. TESTING AND QUALITY REQUIREMENTS

Use a test pyramid:

1.  Unit tests.
2.  Broker and strategy contract tests.
3.  Database and queue integration tests.
4.  Deterministic replay tests.
5.  End-to-end workflow tests.
6.  Failure/recovery tests.
7.  Security and authorization tests.
8.  Regression tests for existing functionality.

Financial correctness test cases must include:

-   Long and short P&L.
-   Average entry after partial fills.
-   Fees in different currencies.
-   Tick size and quantity rounding.
-   Currency conversion.
-   Realized vs unrealized P&L.
-   Position reversal.
-   Stop-loss/take-profit trigger behavior.
-   Session and timezone boundaries.
-   Missing/duplicate candles.
-   Duplicate order submissions.
-   Out-of-order broker updates.
-   Restart and reconciliation.

Test against mocked providers and official sandbox/testnet facilities
where available. Do not assume Binance and FYERS have identical sandbox
support. Paper trading must remain distinct from broker testnet and live
accounts.

A successful build alone does not prove financial correctness.

------------------------------------------------------------------------

# 12. OBSERVABILITY AND OPERATIONS

Implement:

-   Structured logs with correlation IDs.
-   Worker health checks and heartbeats.
-   Market data latency and stale-feed metrics.
-   Queue depth and lag.
-   Backtest duration and failure metrics.
-   Signal-to-risk and risk-to-order latency.
-   Order acknowledgement and fill latency.
-   Reconciliation discrepancy alerts.
-   Bot uptime and error counts.
-   AI usage, latency and cost monitoring.
-   Database backups and restore instructions.
-   Operational runbooks.
-   Manual emergency shutdown.

Set performance targets only after measuring the current hosting and
provider constraints. Do not claim institutional-grade latency or uptime
without evidence.

------------------------------------------------------------------------

# 13. PHASE COMPLETION PROTOCOL

For every phase:

1.  Read the relevant requirements.
2.  Inspect existing code and dependencies.
3.  Break the phase into small tasks.
4.  Implement frontend, backend, persistence and integrations as
    applicable.
5.  Add tests while implementing.
6.  Run unit, integration and regression tests.
7.  Run build, lint and type checks.
8.  Fix failures caused by the changes.
9.  Review security and financial correctness.
10. Update documentation and progress.
11. Verify every acceptance criterion.
12. Continue to the next phase.

At phase completion, record:

``` text
Phase:
Status:
Features implemented:
Files/modules changed:
Database migrations:
API/event changes:
Tests executed:
Test results:
Security review:
Known limitations:
Outstanding blockers:
Acceptance criteria:
Next phase:
```

If a test cannot run because a credential or external service is
unavailable, report it as unverified---not passed. Complete all
offline/mockable work and continue where safe.

------------------------------------------------------------------------

# 14. FINAL END-TO-END ACCEPTANCE TEST

At the end of all phases, verify this complete workflow:

1.  User opens ZoneBoard.
2.  User connects a Binance or FYERS account.
3.  Connection permissions and health are verified.
4.  Market data streams and historical data is available.
5.  User creates or selects a strategy.
6.  Strategy is validated and versioned.
7.  User runs a historical backtest.
8.  Backtest results and assumptions are stored.
9.  User replays a historical session.
10. User creates a bot using the exact strategy version.
11. Bot runs in paper mode with live market data.
12. Signals pass through the risk engine.
13. Simulated orders and fills are recorded.
14. User reviews bot and portfolio performance.
15. AI analyzes stored results and proposes improvements.
16. User reviews the proposal.
17. User explicitly approves a specific live deployment.
18. System revalidates account, strategy, risk and data health.
19. Only then can the approved live bot be activated.
20. Orders, fills, positions, balances and risk events are monitored and
    reconciled.
21. User can pause or stop the bot and use the emergency stop.
22. All sensitive actions are present in the audit trail.

Do not perform step 17 or 18 with real money unless the user explicitly
authorizes it. The final acceptance test can use mocks or paper mode.

------------------------------------------------------------------------

# 15. FINAL DELIVERY REQUIREMENTS

At the end of the complete roadmap, provide:

-   Completed feature inventory.
-   Updated architecture diagrams.
-   Updated database schema and migration history.
-   API documentation.
-   Broker capability matrix.
-   Strategy and indicator documentation.
-   Backtesting methodology and assumptions.
-   AI permissions and governance documentation.
-   Risk policy documentation.
-   Deployment instructions.
-   Environment variable template without secrets.
-   Test strategy and actual test results.
-   Security review.
-   Operational runbooks.
-   Known limitations and external dependencies.
-   Remaining work, if any.

Clearly separate: - Implemented and tested. - Implemented but not fully
verified. - Blocked by external dependency. - Not implemented.

Never report 100% completion if any required feature remains
unimplemented or unverified.

------------------------------------------------------------------------

# 16. OFFICIAL INTEGRATION REFERENCES

Use current official documentation during implementation; verify API
behavior, rate limits, permissions and product availability.

-   Binance API: https://developers.binance.com/
-   FYERS API: https://myapi.fyers.in/docsv3
-   PostgreSQL: https://www.postgresql.org/docs/
-   Redis: https://redis.io/docs/
-   NumPy: https://numpy.org/doc/
-   pandas: https://pandas.pydata.org/docs/
-   Backtrader: https://www.backtrader.com/docu/
-   VectorBT: https://vectorbt.dev/
-   TimescaleDB: https://docs.timescale.com/
-   Prometheus: https://prometheus.io/docs/
-   OpenTelemetry: https://opentelemetry.io/docs/

Review third-party bot frameworks for license, maintenance, security and
actual integration compatibility before adopting them.

------------------------------------------------------------------------

# 17. START EXECUTION NOW

Begin immediately.

**First:** Audit the existing repository and establish the Phase 0
baseline.

**Then:** Implement Phase 0 deliverables, verify them, and continue
automatically through Phases 1, 2, 3, 4, 5, 6 and 7.

Do not stop after planning. Do not ask for approval between routine
phases. Do not delegate to Antigravity or other agents. Do not rewrite
working functionality without a documented reason.

If a blocker requires the user's credentials, an external paid service,
an irreversible operation or authorization for real-money activity,
isolate that blocker, explain exactly what is needed, continue all safe
work, and never fabricate a successful result.

**Your responsibility is to deliver working, tested and documented
ZoneBoard functionality across every phase---not merely to describe how
it could be built.**

**END OF MASTER IMPLEMENTATION PROMPT**
