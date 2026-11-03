-- A populated database exactly as schema version 3 leaves it (before accounts). Used by the migration 4 tests.
-- The point of populating it: SQLite refuses `ADD COLUMN ... REFERENCES ... NOT NULL DEFAULT` on a table with rows,
-- so only a populated fixture proves the migration works on a real journal.
CREATE TABLE trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    date TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL,
    qty REAL NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL,
    fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '',
    market TEXT NOT NULL DEFAULT 'india',
    mode TEXT NOT NULL DEFAULT 'live'
);
CREATE TABLE positions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    market TEXT NOT NULL, mode TEXT NOT NULL DEFAULT 'live',
    symbol TEXT NOT NULL, side TEXT NOT NULL,
    qty REAL NOT NULL, entry REAL NOT NULL, fee_pct REAL NOT NULL, opened_at TEXT NOT NULL
);
CREATE TABLE audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_utc TEXT NOT NULL, actor TEXT NOT NULL, action TEXT NOT NULL, entity TEXT NOT NULL, detail_json TEXT NOT NULL
);
CREATE TRIGGER audit_log_no_update BEFORE UPDATE ON audit_log BEGIN SELECT RAISE(ABORT, 'audit_log is append-only: UPDATE is forbidden'); END;
CREATE TRIGGER audit_log_no_delete BEFORE DELETE ON audit_log BEGIN SELECT RAISE(ABORT, 'audit_log is append-only: DELETE is forbidden'); END;
CREATE TRIGGER audit_log_no_replace BEFORE INSERT ON audit_log WHEN EXISTS (SELECT 1 FROM audit_log WHERE id = NEW.id)
    BEGIN SELECT RAISE(ABORT, 'audit_log is append-only: REPLACE is forbidden'); END;
CREATE TABLE settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL, updated_utc TEXT NOT NULL);

-- Paper trades written by the paper engine ('Paper trade'): these move the USDT wallet.
--   BTCUSDT long 0.01: (79000-78000)*0.01 - 1.5 = 8.5
--   ETHUSDT long 1:    (2100-2000)*1 - 1.5700000000000003 = 98.43   (a fee with float noise, absorbed by 8 dp rounding)
INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES
  ('2026-11-01', 'BTCUSDT', 'long', 0.01, 78000, 79000, 1.5, 'Paper trade', 'crypto', 'demo'),
  ('2026-11-02', 'ETHUSDT', 'long', 1, 2000, 2100, 1.5700000000000003, 'Paper trade', 'crypto', 'demo'),
-- Hand-entered demo trades: linked to demo-1 but must NOT move any wallet.
  ('2026-11-02', 'SBIN', 'long', 10, 100, 110, 5, 'typed in by hand', 'india', 'demo'),
  ('2026-11-02', 'BNBUSDT', 'long', 1, 100, 200, 0, 'typed in by hand', 'crypto', 'demo'),
-- A hand-entered REAL trade: belongs to no account.
  ('2026-10-01', 'INFY', 'long', 5, 100, 110, 2, 'bought via my broker', 'india', 'live');

-- An open paper position (reserves 2000 USDT) and an open NSE paper position (reserves 5000 INR).
INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES
  ('crypto', 'demo', 'ETHUSDT', 'long', 1, 2000, 0.1, '2026-11-02T10:00:00Z'),
  ('india', 'demo', 'SBIN', 'short', 10, 500, 0.05, '2026-11-02T10:05:00Z');

PRAGMA user_version = 3;
