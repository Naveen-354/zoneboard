# Architecture: target state

The master prompt's target (sections 3 and 16 modules) mapped onto the existing stack. Kept: a modular monolith
in Rust with SQLite (single user, single machine: PostgreSQL/Redis would add operations without a benefit here,
see ADR-001). Next steps, in order:

1. **FYERS adapter** (docs/fyers/SPEC.md stages S1 to S8) once U1 to U3 are verified with the owner's app.
2. **Market-data hub**: server-side Binance WebSocket with reconnect, a persistent candle store with backfill and
   repair jobs, feeding the runner and scanner instead of per-request fetches.
3. **Risk additions**: price-deviation check against the last trade, instrument allow/deny lists,
   consecutive-loss pause, portfolio drawdown limit.
4. **Notifications**: bot / order / risk / stale-data alerts (desktop notifications first; the setting exists).
5. **Cost models**: Indian STT, exchange fees, GST, stamp duty for NSE; session calendars and holidays.
6. **Live Binance soak** (ROADMAP gate G3) on the testnet, then the owner's tiny live run (G4/G5 already exist).
