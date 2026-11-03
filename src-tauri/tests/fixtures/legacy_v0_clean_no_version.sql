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
    ('2026-10-20', 'RELIANCE', 'long', 15.0, 2500.0, 2550.0, 4.0, 'clean trade', 'india', 'live');
