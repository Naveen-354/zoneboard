# T005 plan: accounts model and the Demo account

Plan only. No source file was edited. Written against `main` at `b4c4da4`. Spec: `docs/platform/tasks/T005-accounts-demo.md`.
Status: **waiting for the owner's answers to section 9 and a go-ahead** (AGENTS.md rule 1).

Nothing in this plan needs a credential, a network call or a new dependency. It uses only crates already in `Cargo.toml` (`rusqlite`, `rust_decimal`, `serde_json`, `chrono`, `axum`). Nothing here touches the vault (T004); a demo account has no secrets.

## 1. Facts I checked (not assumed)

| Fact | How checked | Why it matters |
|---|---|---|
| Bundled SQLite is **3.54.1** and `PRAGMA foreign_keys` is **ON by default** (`libsqlite3-sys` 0.39.1 `build.rs:171` sets `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`; version from its `sqlite3.h:149`). Versions taken from `src-tauri/Cargo.lock`. Nothing in `src-tauri/src` turns foreign keys on or off, so the default stands. | Downloaded the crate and read it | Foreign keys I add are really enforced; a bad `account_id` is rejected by the database. |
| `ALTER TABLE ... ADD COLUMN x TEXT NOT NULL DEFAULT 'demo-1' REFERENCES accounts(id)` **works on an empty table and fails on a table with rows**: `Cannot add a REFERENCES column with non-NULL default value` (reproduced with Python's SQLite 3.53.1; I did not fetch the sqlite.org wording, the behaviour itself was reproduced). | Ran it both ways | The obvious migration passes a fresh-database test and breaks on the owner's real `journal.db`. The migration must add a **nullable** column then `UPDATE`. A migration test on an empty DB would not catch this; it must run on a populated one. |
| `rust_decimal` 1.43.0 operators `*`, `+`, `-` **panic on overflow** (`arithmetic_impls.rs:161,232,259`: `panic!("Multiplication overflowed")` etc.). `Money`'s `Mul/Add/Sub` (`money.rs`) use those operators. | Read the crate source | `qty * price` with `1e20 * 1e20` would panic while holding `Journal`'s `Mutex`, poisoning it; every later request then panics on `.lock().unwrap()`. New money code must use `Decimal::checked_*`. |
| `audit::record(&tx, ...)` takes `&Connection`; a `Transaction` derefs to it (`settings.rs` already does this). | Read `audit.rs`, `settings.rs` | Account changes and their audit row can be one atomic transaction, as in T003. |
| `is_secret_field` redacts by key name. `credits`, `balance`, `old`, `new`, `positions_removed` are **not** matched. | Read `logging.rs:37-77` | Audit detail for credit changes stays readable. A test pins this so a future over-broad rule cannot silently blank it. |
| All default crypto symbols end in `USDT` (`data.rs` test line 281), but the list file accepts any 5 to 20 character `A-Z0-9` name (`data.rs:62-64`). | Read `data.rs` | A crypto position's currency has to be derived and can fail (see 3.3). |
| The UI sends `mode` only, never an account (`trading.js:125,256`; `journal.js:36`). | Read the JS | The API must keep working with no `account_id`. |

Not run: the three check suites (no Windows toolchain here; the plan does not depend on a baseline run). The implementer must run them at step 0.

## 2. Things in the spec that look wrong or are missing

1. **`demo_wallets(account_id, currency, balance)` cannot support "restore credits".** The spec says available = credits + realised P&L - open notional, credits are "editable", and reset restores credits. With one column, once P&L lands in `balance` the original credits are lost. **Fix: add `credits TEXT NOT NULL`.** `credits` = the number the owner configured; `balance` = cash = credits + realised P&L since the last reset. This keeps the spec's formula exactly (see 3.2). *Needs your OK (Q1).*
2. **"Add `account_id` ... backfill `mode='demo'` rows" with a NOT NULL default is impossible** on a populated table (section 1). Column is nullable. Legacy `mode='live'` rows (hand-entered real trades) keep `NULL`; they belong to no account until T007. Spec is silent on them.
3. **"First run creates `demo-1`" must happen inside migration 4**, not at startup: the backfill needs the row to exist (foreign key), and a startup hook would leave a window with unlinked rows. A fresh database gets it from the same migration.
4. **The spec never says how an order names its account.** Orders carry `mode` only. Proposal: optional `account_id`; absent means `demo-1`. Same for `GET /api/positions` and `/api/journal` (Q3).
5. **No currency rule.** Wallets are per currency (USDT, INR) but nothing says which one an order draws on. Proposal in 3.3.
6. **`status` and `archived` are specified as columns with no values and no API.** Proposal: `status` is `'active'` only in this task; `archived` is always 0; no archive endpoint (nothing to design against yet).
7. **Two existing tests will fail for legitimate reasons and must be edited, not loosened:** `tests/api.rs:497-500` asserts the audit log has exactly one entry (`app_start`), and `tests/migration.rs:252` asserts `user_version == 3`. Seeding `demo-1` writes an audit row (the spec's last criterion says no credit change goes unaudited), and the version becomes 4. New assertions are stricter, not weaker (section 6).
8. **The funds check trusts a price the browser sends.** Fills are still priced by the page (Phase 2 fixes this: T022). A page can send `price: 0.0001` and pass any check. This is a limit of the roadmap order, not something T005 can fix. The funds check is practice bookkeeping, not a security boundary. State it in the code comment and README.
9. **Shorts reserving full notional with no collateral model**, plus fees only charged at close, mean `balance` can legitimately go **negative** after a large adverse short move. Spec does not say what then. Proposal: allowed and shown as is (never hide a loss); `available` is negative so every new order is rejected until reset or a credit increase (Q5).
10. **Roadmap/PROGRESS say T005 "builds on T003 and T004".** The spec says T001 (+T003 for audit). T004 is unused here. No action, noted so nobody waits on it.
11. **`PATCH` needs `axum::routing::patch`**, not imported in `server.rs` today, and every new route must be added to `API_ROUTES` in `tests/api.rs`, because `source_scan_all_routes_are_protected_or_allowed` fails when a route is missing there (that guard is working as intended).
12. **`preferences.default_account` (T003 setting) is free text** and not validated against accounts. Out of scope; flagged for T006 (Q11).

## 3. Design

### 3.1 Migration 4 (in `db.rs`, one transaction, `PRAGMA user_version = 4` inside it, `CURRENT_VERSION = 4`)

```sql
CREATE TABLE accounts (
  id           TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 40),
  kind         TEXT NOT NULL CHECK (kind IN ('demo','binance','fyers')),
  label        TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 60),
  environment  TEXT NOT NULL CHECK (environment IN ('paper','testnet','live')),
  status       TEXT NOT NULL DEFAULT 'active',
  created_utc  TEXT NOT NULL,
  archived     INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0,1)),
  CHECK (kind <> 'demo' OR environment = 'paper')
);
CREATE TABLE demo_wallets (
  account_id TEXT NOT NULL REFERENCES accounts(id),
  currency   TEXT NOT NULL CHECK (currency IN ('USDT','INR')),
  credits    TEXT NOT NULL,          -- decimal string, owner-configured   (added to spec, Q1)
  balance    TEXT NOT NULL,          -- decimal string, credits + realised P&L since last reset
  PRIMARY KEY (account_id, currency)
);
INSERT INTO accounts VALUES ('demo-1','demo','Demo','paper','active',<now>,0);
INSERT INTO demo_wallets VALUES ('demo-1','USDT','10000','10000'), ('demo-1','INR','1000000','1000000');

ALTER TABLE positions ADD COLUMN account_id TEXT REFERENCES accounts(id);   -- nullable on purpose
ALTER TABLE trades    ADD COLUMN account_id TEXT REFERENCES accounts(id);
UPDATE positions SET account_id = 'demo-1' WHERE mode = 'demo';
UPDATE trades    SET account_id = 'demo-1' WHERE mode = 'demo';
CREATE INDEX idx_positions_account ON positions(account_id);
CREATE INDEX idx_trades_account    ON trades(account_id);

-- safety net for legacy writers (the frozen Python app, an old script) that insert demo rows without an account:
CREATE TRIGGER positions_demo_default_account AFTER INSERT ON positions
  WHEN NEW.mode = 'demo' AND NEW.account_id IS NULL
  BEGIN UPDATE positions SET account_id = 'demo-1' WHERE id = NEW.id; END;
CREATE TRIGGER trades_demo_default_account AFTER INSERT ON trades
  WHEN NEW.mode = 'demo' AND NEW.account_id IS NULL
  BEGIN UPDATE trades SET account_id = 'demo-1' WHERE id = NEW.id; END;
```

Then, still inside the same transaction, Rust code (`accounts::seed_from_legacy(&tx)`):

- For each trade with `account_id='demo-1'` and `notes='Paper trade'` (the exact marker `close_position` writes and migration 2 already keys on), compute realised P&L with the **same function** later used at close time (3.2), per currency, and set `balance = credits + sum`. Hand-entered demo trades are linked but do **not** move the wallet (Q2).
- Trades whose currency cannot be derived are linked but skipped; their count goes in the audit detail and a `tracing::warn!`.
- Write one audit row: actor `system`, action `account_created`, entity `account:demo-1`, detail `{account_id, kind, label, environment, credits:{USDT,INR}, seeded_realised:{USDT,INR}, linked_trades, linked_positions, skipped_trades}`.

Existing demo positions keep their `qty/entry` (still `REAL`); their notional is simply reserved against the wallet. If they already exceed the credits, `available` is negative and ordering is blocked until the owner raises credits or resets. Say so in the audit detail via `linked_positions`; do not auto-close anything.

Migration is irreversible: an older binary refuses a v4 file (`VersionTooNew`, existing behaviour). It is transactional, so a failure leaves the file at v3. The owner should copy `journal.db` first (Q8).

### 3.2 Funds maths (new `accounts.rs`, all `Decimal`, no `f64` except at the legacy-column boundary)

- Convert the legacy `REAL` columns once, with `Money::from_legacy_f64` (shortest decimal text, already tested in T001). No raw binary conversion.
- `notional(position) = qty * entry`, using `checked_mul`; overflow or non-finite is a clean 400, never a panic. Hard cap: order notional must be <= 1e15 (documented, tested).
- `open_notional(account, currency) = sum` of notional of that account's open positions in that currency, `checked_add`.
- `available = balance - open_notional`. Equals the spec's credits + realised - open notional because `balance = credits + realised` is maintained (below). Shorts reserve the same full notional.
- `realised(side, qty, entry, exit, fees)`: `gross = (exit - entry) * qty * (+1 long | -1 short)`; `net = gross - fees`; fees are the **stored `f64` fees column** converted the same way, so the fee formula in `journal.rs` stays byte-for-byte unchanged ("fee maths unchanged"). Rounded `round_half_even(8)` (Q12). One function, used by both close and migration seed, with a test that they agree on the same row.
- Order accepted iff `notional <= available` (equal is accepted).
- **Close**: in the existing close transaction, after inserting the trade, `UPDATE demo_wallets SET balance = balance + net` for the position's currency. Position delete and wallet update commit together or not at all.
- **PATCH credits**: new `credits` replaces old; `balance += (new - old)` so realised P&L is preserved. Validated per currency: finite, >= 0, <= 1e15, at most 8 decimals for USDT and 2 for INR (reject, never silently round). Whole PATCH is all-or-nothing.
- **Reset**: delete the account's open positions (no trades written), `balance = credits` for every wallet, **keep all trades**. Trade history no longer sums to `balance - credits` after a reset; that is expected and documented.
- `journal` deleting a trade (`DELETE /api/journal/{id}`) does **not** change the wallet (stored, not derived). Deliberate: otherwise deleting a journal row would conjure funds. Tested.

### 3.3 Currency of an order

`india` -> `INR`. `crypto` -> `USDT` if the symbol ends in `USDT`; **any other crypto quote is rejected** with: `no virtual wallet for <SYMBOL>: only USDT-quoted crypto and NSE (INR) can be traded on the demo account yet` (Q4). Legacy open positions whose currency can't be derived are skipped in `open_notional` with a `tracing::warn!`; reset still removes them.

### 3.4 Which account

- `POST /api/positions`: optional `account_id` (string). Absent -> `demo-1`. Must exist, `kind='demo'`, `archived=0`, else 400 `unknown or archived demo account`. `mode` must still be `demo`; the existing "live orders aren't supported" error is untouched and still comes first.
- `GET /api/positions` and `GET /api/journal`: optional `account_id`. Absent with `mode=demo` -> `demo-1` (Q3). `mode=live` queries behave exactly as today.
- `Position` and `Trade` JSON gain an additive `account_id` (nullable). No field removed or renamed; `qty/entry/fees/pnl` stay numbers, so `static/` keeps working with no change.
- `mode='demo'` is still written on every new row ("keep the old `mode` column working").

### 3.5 API (all under `/api`, so the T002 token guard and the T002 Origin check already cover them; `PATCH` is already in the guard's method list)

| Route | Behaviour |
|---|---|
| `GET /api/accounts` | `{ "accounts": [ { id, kind, label, environment, status, created_utc, archived, "virtual": true, open_positions, wallets: [ { currency, credits, balance, in_positions, available, "virtual": true } ] } ], "notice": "Virtual practice credits. This app holds no money." }`. Money fields are **strings** (the `Money` serializer). |
| `POST /api/accounts` | `{kind:"demo", label, credits?:{USDT?,INR?}}` -> 201 with the account. Defaults 10000 / 1000000. `kind` binance/fyers -> 400 `only demo accounts can be created yet; broker accounts arrive with T007`. `environment` if given must be `paper`. Id `demo-<n>`, next free integer. Max 10 demo accounts (Q6). Audit `account_created`. |
| `PATCH /api/accounts/{id}` | Only `label` and `credits` are accepted; **any other key is a 400** (so `balance`, `virtual`, `kind`, `id`, `status` cannot be smuggled). Unknown id -> 404. A no-op writes nothing and audits nothing (same rule as T003). Audit `account_updated` with `old`/`new` of the changed fields only. |
| `POST /api/accounts/{id}/reset` | As in 3.2. 404 unknown. Audit `account_reset` with `positions_removed`, a capped (50) list of `{symbol,side,qty,entry}`, `balance_before`, `balance_after`. Returns the account. |

Errors are `{"error": "..."}`. Body parsing reuses `json_body`; a non-object body is a 400 `request body must be a JSON object` (same wording as settings). Audit `actor` is `user` for API calls and `system` for the migration seed, matching T003.

`virtual: true` is set in code where the wallet and account structs are built (one place), not per handler, so no future endpoint can return a demo balance without it.

Insufficient funds -> **400** `insufficient virtual funds: this order needs 780 USDT but only 500 USDT of virtual credit is available` (Q9 on 400 vs 422). The words "virtual" are in the message on purpose.

## 4. Files and order

Work in a worktree on `task/T005`; every step ends with `cargo test --no-default-features` green and zero warnings, one cargo command at a time.

| # | Step | Files | Ends green because |
|---|---|---|---|
| 0 | Baseline: run the three checks, record counts | none | known starting point (expect 85 Rust, 30 Python per PROGRESS) |
| 1 | `accounts.rs`: types, `Currency`, `currency_for(market, symbol)`, `realised(...)`, `open_notional`, `available`, credit parsing and caps, all pure; unit tests. Add `pub mod accounts;` | `accounts.rs` (new), `lib.rs` | nothing else calls it yet |
| 2 | Migration 4 + `seed_from_legacy`; `CURRENT_VERSION = 4`; new fixture `legacy_v3_demo.sql`; update the two stale assertions; new migration tests | `db.rs`, `accounts.rs`, `tests/migration.rs`, `tests/fixtures/`, `tests/api.rs:497` | all existing tests pass with the corrected assertions |
| 3 | DB layer in `accounts.rs`: `list`, `create`, `update`, `reset`, each a transaction that writes its audit row | `accounts.rs` | unit tests on `:memory:` |
| 4 | `journal.rs`: account resolution, funds check, wallet update on close, `account_id` fields, `account_id` filters; keep every existing assertion | `journal.rs` | existing paper-trading tests unchanged and passing |
| 5 | `server.rs`: four handlers, `patch` import, `account_id` on positions/journal queries, `API_ROUTES` additions | `server.rs`, `tests/api.rs` | route-coverage and CSRF tests include the new routes |
| 6 | README (API list, "virtual credits, no money"), `docs/platform/PROGRESS.md` is Claude's to update | `README.md` | docs only |
| 7 | Mutation round (section 6.4), then the final report | none | |

`static/` is **not** touched (UI is T006), so no release re-embed is needed and the JS-vs-Python zone parity tests are unaffected.

## 5. Spec acceptance criteria, and how each is met

| Criterion | Where met |
|---|---|
| Migration test from a database with demo trades and positions; trades linked to `demo-1` | M1 below: new fixture at v3 with demo trades, demo positions, a hand-entered demo trade, a live trade; asserts exact `account_id`s and exact seeded balances |
| Funds tests: within funds accepted, above rejected, close releases and applies P&L, reset restores | F1 to F7 below |
| Existing paper-trading tests pass; fee maths unchanged | Steps 4 and 5 may not edit any assertion in `journal.rs` or `api.rs` paper tests; reviewer checks `git diff` shows only additions there. F8 is a golden fee test |
| No secret or credit change unaudited | A1 to A4 below: create, patch, reset and the migration seed each write exactly one audit row in the same transaction; failed requests write none |
| (In scope 5) every demo balance carries `"virtual": true` | A5 walks the whole JSON of every accounts response |

## 6. Tests, with the breakage that must turn each red

Per `WORKFLOW.md` 6.4: one mutation at a time, each classed CAUGHT / SURVIVED / INCONCLUSIVE (a linker failure is INCONCLUSIVE, not CAUGHT). Any SURVIVED is a defect in the test.

### 6.1 Migration (`tests/migration.rs`)

| Id | Test | Mutation that must fail it |
|---|---|---|
| M1 | Populated v3 DB (fixture: BTCUSDT paper trade fees 1.5 -> pnl 10 - 1.5; ETHUSDT demo position qty 1 @ 2000; SBIN hand-entered demo trade; INFY live trade). Expect version 4; demo rows -> `demo-1`; live rows `NULL`; USDT wallet credits 10000, balance **10008.5**, in_positions 2000, available **8008.5**; INR untouched; one `account_created` audit row | remove the two `UPDATE ... SET account_id`; make the backfill `WHERE 1`; seed ignores fees; seed includes the hand-entered trade |
| M2 | Same fixture is *populated*, so the real SQLite rule is exercised | change `ADD COLUMN` to `NOT NULL DEFAULT 'demo-1'`: migration errors, test red |
| M3 | Fresh DB: `demo-1`, label, 10000 USDT / 1000000 INR, `virtual`, one audit row | change a default amount |
| M4 | Re-open a v4 DB: no second `demo-1`, no second audit row; v99 still refused untouched | drop the `ver < 4` guard |
| M5 | Legacy-writer trigger: insert demo trade/position with no `account_id` -> `demo-1`; live stays `NULL` | drop a trigger; drop the `WHEN` clause |
| M6 | Foreign key is live: insert a position with `account_id='nope'` fails | n/a (would catch a build where keys are off) |
| M7 | Existing `fixture_v2_migrates_to_v3...` now reaches v4 with data intact (literal `3` becomes `4`) | |

### 6.2 Funds (`journal.rs` unit tests on `:memory:` + HTTP tests)

Numbers are chosen so float noise would show (0.1% fee on 78000 x 0.01).

| Id | Test | Mutation |
|---|---|---|
| F1 | 780 USDT order on 10000 accepted; wallet shows in_positions 780, available 9220 | skip the check |
| F2 | Boundary: notional == available accepted; one smallest unit more rejected with the "virtual funds" message and **nothing inserted** | `<=` to `<`; `>=` |
| F3 | Short reserves full notional: short 6000 then long 5000 rejected on 10000 | shorts reserve 0 |
| F4 | Two open positions reserve cumulatively | reserve only the latest |
| F5 | Close releases and applies P&L and fees exactly (win, loss, short win) against hand-computed `Decimal`s, and equals the trade row's `pnl` within 0.005 | forget to add P&L; forget fees; release without P&L; round half-away |
| F6 | Reset: wallets back to credits, positions gone, trade count unchanged, other accounts untouched, one audit row | not deleting positions; deleting trades; not restoring; resetting every account |
| F7 | Currency and account isolation: INR order does not touch USDT; `demo-2` funds independent; unknown quote (e.g. `ETHBTC`) rejected with the explicit message | single shared pool |
| F8 | Fee golden test: fees for BTCUSDT 0.01 @ 78000 -> 79000 equal `0.1/100*0.01*(78000+79000)` as before | change fee formula |
| F9 | Overflow: qty `1e20`, price `1e20` returns 400 **and the next request still succeeds** (mutex not poisoned); non-finite and zero still rejected | replace `checked_mul` with `*` (panics) |
| F10 | Deleting a paper trade from the journal does not change the wallet | derive balance from trades |
| F11 | Negative balance: a large losing short drives `balance` below zero, `available` negative, a new order is rejected | clamp at zero |

### 6.3 API (`tests/api.rs`)

| Id | Test | Mutation |
|---|---|---|
| A1 | Create writes exactly one `account_created` audit row, with the credits in clear | skip the audit |
| A2 | PATCH credits: one `account_updated` row, `old`/`new` correct, `balance` shifted by the delta so earlier realised P&L survives; PATCH of a label alone leaves wallets alone; no-op PATCH writes **zero** audit rows | set `balance = credits`; audit on no-op |
| A3 | Reset writes one `account_reset` row with the removed positions and balances before/after | skip the audit |
| A4 | All-or-nothing: `credits {"USDT":"5","INR":"abc"}` -> 400, USDT unchanged, no audit row. `credits` / `balance` keys are not redacted in stored detail | apply sequentially without a transaction; widen `is_secret_field` |
| A5 | Recursive walk of every `/api/accounts*` response: every object containing `balance` or `credits` also has `"virtual": true`; money values are JSON strings | delete the field in the one constructor |
| A6 | POST: kind `binance`/`fyers` -> 400; `environment:"live"` -> 400; empty/61-char label; negative, non-numeric, `1e40`, 9-decimal USDT, 3-decimal INR credits; 11th account -> 400; nothing written in any failure | drop a validation |
| A7 | PATCH rejects `balance`, `virtual`, `kind`, `id`, `status`, `archived`; unknown id 404; body not an object 400 | accept any key |
| A8 | Orders: `account_id` of a missing account -> 400; `mode=live` still returns the existing "live orders" error before any account lookup | reorder checks |
| A9 | `API_ROUTES` gains the four routes + a `PATCH` entry; the existing token-coverage and source-scan tests cover them; CSRF test gains a cross-origin `PATCH /api/accounts/demo-1` -> 403 | omit a route from the table (scan test goes red) |
| A10 | No secret-shaped field or token in any accounts response or audit row (reuse the T002/T003 helper) | |

### 6.4 Mutation round

Before review, the implementer runs and lists in the report at least: `<=` to `<` (F2); shorts reserve 0 (F3); omit wallet update on close (F5); omit fees (F5); reset skips position delete (F6); `checked_mul` to `*` (F9); drop backfill `UPDATE` (M1); `NOT NULL DEFAULT` column (M2); drop a trigger (M5); drop `virtual` (A5); skip each of the three audit calls (A1-A3). Reviewer re-runs at least five of them.

## 7. Risks

1. **Irreversible migration on the owner's real `journal.db`.** Tests never open it (AGENTS.md). Migration is transactional; an old binary will refuse the file. Copy the file first (Q8).
2. **Float history.** Trades are stored as `REAL`; the seed converts via shortest text, which is deterministic but can carry noise digits such as `1.5700000000000003` in a fee. Rounding to 8 dp at the wallet boundary absorbs this; test M1 uses fees that are exactly representable and a second case that is not.
3. **Panic poisons the journal mutex.** Every `.lock().unwrap()` in `journal.rs` would then panic forever. Mitigation: `checked_*`, input caps, test F9. Not changing the `unwrap`s (out of scope).
4. **Journal and wallet can diverge** (deleted paper trades, reset). Deliberate; documented in README and code.
5. **Python backend** (`app.py`) shares the schema and ignores funds. The triggers keep its demo rows linked; it will not be funds-checked. It is frozen (T009). Do not point it at the real journal after T005.
6. **Client-supplied prices** (2.8). Not fixable here.
7. **Open positions over credits after upgrade** (3.1) block new orders until reset or a credit raise. The owner sees this on first run of T006; mention in the report.
8. **Concurrency.** One connection behind a `Mutex`, so check-and-insert is already serialised. I still use an `IMMEDIATE` transaction for the order path so a second connection (Python) cannot interleave. No `busy_timeout` change.
9. **T007 will need `account_id` on live rows** and a decision about the `NULL` legacy live trades. Not decided here.
10. **Scope creep.** No UI, no archive/delete endpoint, no balance for non-demo kinds, no fill-price change.

## 8. Dependencies

None added. D8 list untouched.

## 9. Questions for the owner (each has a default I will use if you say "your call")

1. **Q1** Add a `credits` column to `demo_wallets` (spec lists only `balance`)? Default: yes. Without it reset cannot restore credits.
2. **Q2** Should hand-entered journal trades with mode `demo` move the wallet? Default: no, only trades the paper engine wrote (`notes = 'Paper trade'`); they are still linked to `demo-1`.
3. **Q3** Orders, positions and journal with no `account_id`: use `demo-1`? Default: yes. (Alternative: all demo accounts for reads. Rejected: a position shown but belonging to another account is confusing.)
4. **Q4** Reject crypto symbols not quoted in USDT on the demo account? Default: reject with a clear message. Alternative: add more currencies, which means more wallets.
5. **Q5** Allow `balance` to go negative after big losses (shown honestly, orders blocked)? Default: yes. Alternative: floor at zero, which hides losses.
6. **Q6** Cap of 10 demo accounts and labels 1 to 60 characters? Default: yes.
7. **Q7** Allow lowering credits below the notional of open positions? Default: yes, `available` goes negative until reset.
8. **Q8** Should the app copy `journal.db` to `journal.db.v3.bak` before migration 4? Default: no automatic file creation; you copy it yourself first (`copy journal.db journal.db.v3.bak`). Say yes if you want it automated (small extra code, one new file next to your data).
9. **Q9** Insufficient funds as HTTP 400 (matches every other order error, the UI shows `e.message` for any non-OK) or 422? Default: 400.
10. **Q10** Should the entry-side fee also be reserved at order time? Default: no (spec says notional only; the fee is charged at close, unchanged).
11. **Q11** Should `preferences.default_account` be validated against real account ids now? Default: no, leave to T006.
12. **Q12** Wallet rounding: 8 decimal places for both currencies internally, API strings trimmed? Default: yes.
13. **Q13** Keep the two `AFTER INSERT` triggers as a safety net for legacy writers? Default: yes.

## 10. Report (WORKFLOW.md section 5 format, for this planning run)

```
## Report: T005 (plan only)
Summary: Concrete plan for the accounts model and funds-checked Demo account. Found 12 spec issues, the main ones being a missing credits column, a migration form that fails on populated databases, overflow panics in Money, and 2 existing tests that must change legitimately.
Files changed: docs/platform/plans/T005-plan.md (new, on branch plan/T005). No source file edited.
Checks run: none of the cargo / pytest / live_check suites (plan only; no Windows toolchain here). Facts below were checked by reading crate source and running SQLite.
Spec deviations: proposed (need approval) credits column; nullable account_id; optional account_id on orders; demo-1 created inside the migration.
Verified against official docs: bundled SQLite 3.54.1 with foreign keys on by default: libsqlite3-sys 0.39.1 build.rs and sqlite3.h from static.crates.io; ADD COLUMN REFERENCES non-NULL default fails on populated tables: reproduced in SQLite 3.53.1; rust_decimal 1.43.0 operators panic on overflow: src/arithmetic_impls.rs. The SQLite ALTER TABLE wording itself was not fetched from sqlite.org; behaviour was reproduced instead.
Open questions / risks: section 9 (13 questions) and section 7.
Commit: see branch plan/T005.
```
