# T005: Accounts model and the Demo account

Phase 1 · Depends: T001 (T003 for audit entries) · Touches: `accounts.rs` (new), `db.rs` (migration 4), `journal.rs`, `server.rs`

## Goal
A real accounts model. The practice account has **configurable virtual credits**, clearly marked virtual. Live brokers plug into the same model later.

## In scope
1. **Migration 4:** `accounts(id TEXT PRIMARY KEY, kind 'demo'|'binance'|'fyers', label, environment 'paper'|'testnet'|'live', status, created_utc, archived)` and `demo_wallets(account_id, currency, balance TEXT)` (decimal strings, T001 money type). Add `account_id` to `positions` and `trades`; **backfill** existing `mode='demo'` rows to a default demo account `demo-1`. Keep the old `mode` column working for legacy code.
2. First run creates `demo-1` with default credits **10,000 USDT and 1,000,000 INR**, both editable.
3. API: `GET /api/accounts`, `POST /api/accounts` (demo only for now), `PATCH /api/accounts/{id}` (label, credits), `POST /api/accounts/{id}/reset` (restore credits, remove open paper positions, keep trade history, **audit it**).
4. Paper order placement now checks **available funds** for the demo account: available = credits + realised P&L - notional of open positions. Reject an order that exceeds it with a clear error. Shorts reserve their full notional.
5. Every demo balance returned by the API carries `"virtual": true`. **Never present virtual funds as real.** This is a hard rule from the product brief.

## Out of scope
Live/broker accounts (T007), any UI (T006), changing how fills are priced (Phase 2).

## Acceptance criteria
- Migration test from a database that has demo trades and positions; trades are linked to `demo-1`.
- Funds tests: order within funds accepted, above funds rejected, closing releases funds and applies P&L, reset restores.
- All existing paper-trading tests still pass; fee maths unchanged.
- No secret or credit change goes unaudited (if T003 is present).
