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
    market TEXT NOT NULL DEFAULT 'india'
);
INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market) VALUES
    ('2026-10-15', 'TCS', 'short', 5.0, 3500.0, 3450.0, 2.5, 'trade with market but no mode', 'india'),
    ('2026-10-16', 'BTCUSDT', 'long', 0.05, 65000.0, 67000.0, 1.2, 'crypto trade', 'crypto');
