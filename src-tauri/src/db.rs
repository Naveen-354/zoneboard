//! Database management: schema migrations via `PRAGMA user_version` and application data directory resolution.

use std::path::PathBuf;
use rusqlite::Connection;

/// The latest schema version supported by this application.
pub const CURRENT_VERSION: u32 = 4;

/// Database error type covering schema version mismatches, SQLite errors, and IO issues.
#[derive(Debug)]
pub enum DbError {
    /// Attempted to open a database created by a newer version of the application.
    VersionTooNew { found: u32, max_supported: u32 },
    /// SQLite engine error.
    Sqlite(rusqlite::Error),
    /// File system or IO error.
    Io(std::io::Error),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VersionTooNew { found, max_supported } => {
                write!(
                    f,
                    "database version {found} is newer than supported version {max_supported}"
                )
            }
            Self::Sqlite(e) => write!(f, "database error: {e}"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::VersionTooNew { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Resolves the application data directory without creating it.
///
/// Priority:
/// 1. `ZONEBOARD_DATA` environment variable (if non-empty)
/// 2. OS standard app data folder (`dirs::data_dir()` / `ZoneBoard`)
pub fn data_dir() -> PathBuf {
    if let Ok(val) = std::env::var("ZONEBOARD_DATA") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("ZoneBoard")
}

/// Ensures the application data directory exists on disk, returning its path.
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Returns the current `user_version` of the SQLite database.
pub fn user_version(conn: &Connection) -> Result<u32, DbError> {
    let v: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    Ok(v)
}

/// Runs all pending schema migrations inside explicit transactions.
///
/// Refuses to modify or downgrade any database whose `user_version` exceeds [`CURRENT_VERSION`].
/// Each migration updates `user_version` inside its own transaction before committing.
pub fn migrate(conn: &mut Connection) -> Result<(), DbError> {
    let mut ver = user_version(conn)?;
    if ver > CURRENT_VERSION {
        return Err(DbError::VersionTooNew {
            found: ver,
            max_supported: CURRENT_VERSION,
        });
    }

    if ver < 1 {
        let tx = conn.transaction()?;
        // Migration 1: baseline tables as created today
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                date TEXT NOT NULL, symbol TEXT NOT NULL, side TEXT NOT NULL,
                qty REAL NOT NULL, entry REAL NOT NULL, exit REAL NOT NULL,
                fees REAL NOT NULL DEFAULT 0, notes TEXT NOT NULL DEFAULT '',
                market TEXT NOT NULL DEFAULT 'india',
                mode TEXT NOT NULL DEFAULT 'live'
            );
            CREATE TABLE IF NOT EXISTS positions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                market TEXT NOT NULL, mode TEXT NOT NULL DEFAULT 'live',
                symbol TEXT NOT NULL, side TEXT NOT NULL,
                qty REAL NOT NULL, entry REAL NOT NULL, fee_pct REAL NOT NULL, opened_at TEXT NOT NULL
            );",
        )?;

        // If the database was created by an unmigrated legacy build, trades might lack market or mode.
        let cols: Vec<String> = {
            let mut st = tx.prepare("PRAGMA table_info(trades)")?;
            let mapped = st.query_map([], |r| r.get::<_, String>(1))?;
            mapped.filter_map(Result::ok).collect()
        };
        if !cols.iter().any(|c| c == "market") {
            tx.execute(
                "ALTER TABLE trades ADD COLUMN market TEXT NOT NULL DEFAULT 'india'",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "mode") {
            tx.execute(
                "ALTER TABLE trades ADD COLUMN mode TEXT NOT NULL DEFAULT 'live'",
                [],
            )?;
        }

        // Set user_version inside the transaction
        tx.execute_batch("PRAGMA user_version = 1;")?;
        tx.commit()?;
        ver = 1;
    }

    if ver < 2 {
        let tx = conn.transaction()?;
        // Migration 2: one-off relabel of old paper trades from live to demo
        tx.execute(
            "UPDATE trades SET mode = 'demo' WHERE mode = 'live' AND notes = 'Paper trade'",
            [],
        )?;
        tx.execute(
            "UPDATE positions SET mode = 'demo' WHERE mode = 'live'",
            [],
        )?;
        tx.execute_batch("PRAGMA user_version = 2;")?;
        tx.commit()?;
        ver = 2;
    }

    if ver < 3 {
        let tx = conn.transaction()?;
        // Migration 3: audit_log table with append-only triggers, and settings store.
        // Triggers stop accidental and application-level edits; someone with direct file access
        // can still drop the triggers or table (a cryptographic hash chain is a later phase).
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS audit_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_utc TEXT NOT NULL,
                actor TEXT NOT NULL,
                action TEXT NOT NULL,
                entity TEXT NOT NULL,
                detail_json TEXT NOT NULL
            );

            CREATE TRIGGER IF NOT EXISTS audit_log_no_update
            BEFORE UPDATE ON audit_log
            BEGIN
                SELECT RAISE(ABORT, 'audit_log is append-only: UPDATE is forbidden');
            END;

            CREATE TRIGGER IF NOT EXISTS audit_log_no_delete
            BEFORE DELETE ON audit_log
            BEGIN
                SELECT RAISE(ABORT, 'audit_log is append-only: DELETE is forbidden');
            END;

            CREATE TRIGGER IF NOT EXISTS audit_log_no_replace
            BEFORE INSERT ON audit_log
            WHEN EXISTS (SELECT 1 FROM audit_log WHERE id = NEW.id)
            BEGIN
                SELECT RAISE(ABORT, 'audit_log is append-only: REPLACE is forbidden');
            END;

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL,
                updated_utc TEXT NOT NULL
            );

            PRAGMA user_version = 3;",
        )?;
        tx.commit()?;
        ver = 3;
    }

    if ver < 4 {
        let tx = conn.transaction()?;
        // Migration 4: accounts and the virtual demo wallets (T005).
        // `account_id` is added NULLABLE on purpose: SQLite refuses `ADD COLUMN ... REFERENCES ... NOT NULL
        // DEFAULT` on a table that already has rows, which would pass on an empty test database and fail on
        // a real journal. Legacy `mode = 'live'` rows (hand-entered real trades) keep NULL: they belong to no
        // account until a broker account exists.
        tx.execute_batch(
            "CREATE TABLE accounts (
                id          TEXT PRIMARY KEY CHECK (length(id) BETWEEN 1 AND 40),
                kind        TEXT NOT NULL CHECK (kind IN ('demo','binance','fyers')),
                label       TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 60),
                environment TEXT NOT NULL CHECK (environment IN ('paper','testnet','live')),
                status      TEXT NOT NULL DEFAULT 'active',
                created_utc TEXT NOT NULL,
                archived    INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0,1)),
                CHECK (kind <> 'demo' OR environment = 'paper')
            );
            CREATE TABLE demo_wallets (
                account_id TEXT NOT NULL REFERENCES accounts(id),
                currency   TEXT NOT NULL CHECK (currency IN ('USDT','INR')),
                credits    TEXT NOT NULL,
                balance    TEXT NOT NULL,
                PRIMARY KEY (account_id, currency)
            );",
        )?;
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        tx.execute(
            "INSERT INTO accounts (id, kind, label, environment, status, created_utc, archived) VALUES ('demo-1','demo','Demo','paper','active',?1,0)",
            [now],
        )?;
        tx.execute_batch(
            "INSERT INTO demo_wallets (account_id, currency, credits, balance) VALUES
                ('demo-1','USDT','10000','10000'), ('demo-1','INR','1000000','1000000');
            ALTER TABLE positions ADD COLUMN account_id TEXT REFERENCES accounts(id);
            ALTER TABLE trades ADD COLUMN account_id TEXT REFERENCES accounts(id);
            UPDATE positions SET account_id = 'demo-1' WHERE mode = 'demo';
            UPDATE trades SET account_id = 'demo-1' WHERE mode = 'demo';
            CREATE INDEX idx_positions_account ON positions(account_id);
            CREATE INDEX idx_trades_account ON trades(account_id);
            -- safety net for legacy writers (the frozen Python app, an old script) that add demo rows with no account
            CREATE TRIGGER positions_demo_default_account AFTER INSERT ON positions
              WHEN NEW.mode = 'demo' AND NEW.account_id IS NULL
              BEGIN UPDATE positions SET account_id = 'demo-1' WHERE id = NEW.id; END;
            CREATE TRIGGER trades_demo_default_account AFTER INSERT ON trades
              WHEN NEW.mode = 'demo' AND NEW.account_id IS NULL
              BEGIN UPDATE trades SET account_id = 'demo-1' WHERE id = NEW.id; END;",
        )?;
        crate::accounts::seed_from_legacy(&tx)?;
        tx.execute_batch("PRAGMA user_version = 4;")?;
        tx.commit()?;
    }

    Ok(())
}

/// Opens a SQLite connection at `path` and runs all pending migrations.
pub fn open(path: &str) -> Result<Connection, DbError> {
    let mut conn = Connection::open(path)?;
    migrate(&mut conn)?;
    Ok(conn)
}
