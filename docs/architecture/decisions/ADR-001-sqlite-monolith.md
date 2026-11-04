# ADR-001: keep SQLite and the single Rust server
Status: accepted (2026-11-04). The master prompt suggests PostgreSQL and Redis "if it fits the existing stack".
ZoneBoard is a single-user desktop app with one process and one data file; SQLite with versioned migrations,
transactions, foreign keys and triggers gives durable financial state without running a database server. The
outbox (`broker_orders`) gives the consistency a queue would. Revisit if a second process or user appears.
