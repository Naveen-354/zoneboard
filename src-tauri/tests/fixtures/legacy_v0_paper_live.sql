CREATE TABLE trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    qty REAL NOT NULL,
    entry REAL NOT NULL,
    exit REAL NOT NULL,
    fees REAL NOT NULL DEFAULT 0,
    notes TEXT NOT NULL DEFAULT '',
    market TEXT NOT NULL DEFAULT 'india',
    mode TEXT NOT NULL DEFAULT 'live'
);
CREATE TABLE positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market TEXT NOT NULL,
    mode TEXT NOT NULL DEFAULT 'live',
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    qty REAL NOT NULL,
    entry REAL NOT NULL,
    fee_pct REAL NOT NULL,
    opened_at TEXT NOT NULL
);
INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES
    ('2026-11-01', 'BTCUSDT', 'long', 0.01, 78000.0, 79000.0, 1.5, 'Paper trade', 'crypto', 'live'),
    ('2026-11-01', 'INFY', 'long', 20.0, 1800.0, 1850.0, 3.0, 'real manual trade', 'india', 'live');
INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES
    ('crypto', 'live', 'ETHUSDT', 'long', 1.0, 2000.0, 0.1, '2026-11-01T10:00:00Z');
