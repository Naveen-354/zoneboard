# Test strategy
- **Unit** (Rust `#[cfg(test)]`): indicators (hand vectors, a published example, prefix/no-look-ahead property),
  strategy validation, engine execution rules, metrics, Monte Carlo seeding, walk-forward isolation, risk gates,
  approvals, tokens, scanner formulas, portfolio attribution, data quality.
- **Integration** (`src-tauri/tests`): HTTP API with the token guard and a route-coverage table; migrations on real
  legacy fixtures; a **fake Binance** (signature checking, skew, 429, withdrawal flag, orders with idempotency and
  dropped replies); a **fake model server** for the AI.
- **Property**: future bars never change past trades, for every template.
- **Mutation checks**: each safety rule was broken on purpose and the suite had to fail (kill switch, paper live
  refusal, bot capital limit, next-open fills, arming, approval, AI self-approval, approval scope, outbox
  idempotency, lost-reply lookup, dev-token management, withdrawal keys).
- **Browser**: `tools/ui_smoke.mjs` (6 scenarios) and end-to-end Playwright drives on sample data.
- **Contract**: `tools/sdk/test_sdk.py` against the real server binary.
- Nothing runs against a real exchange, real key or real money. Owner-only checks are listed in `test-results.md`.
