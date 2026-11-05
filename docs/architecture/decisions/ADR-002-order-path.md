# ADR-002: one engine, two order paths, one risk engine
Status: accepted. Strategies emit decisions through the engine's `Book`. Paper accounts fill synchronously in
`orders.rs`; broker accounts produce intents executed in `execution.rs` after the engine step, through an outbox
row written before sending (deterministic client order id, unique). Both paths call `risk.rs`; closes are never
blocked. An unknown outcome is looked up at the exchange before anything else and stops the bot if unresolved.
