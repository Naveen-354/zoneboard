CREATE TABLE trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    qty INTEGER NOT NULL,
    entry REAL NOT NULL,
    exit REAL NOT NULL,
    fees REAL NOT NULL DEFAULT 0,
    notes TEXT NOT NULL DEFAULT ''
);
INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes) VALUES
    ('2026-10-01', 'SBIN', 'long', 10, 100.0, 110.0, 5.0, 'legacy trade without market');
