# Feature inventory
Before this work (main `f41de44`): scanner board, charts with zones, journal, order-block backtest, paper ticket,
demo accounts with virtual credits, API token, audit log, settings, OS-keychain vault (unused), smoke harness.
Added: everything in `progress.md` marked VERIFIED / TESTED / IMPLEMENTED. Missing: rows marked BLOCKED or
NOT_STARTED there. Duplicates: two backtesters exist on purpose (`backtest.rs` = order-block rule kept for parity
with the frozen Python reference; `backtest2.rs` = bot strategies).
