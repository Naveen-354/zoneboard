//! Database management: schema migrations via `PRAGMA user_version` and application data directory resolution.

use std::path::PathBuf;
use rusqlite::Connection;

/// The latest schema version supported by this application.
pub const CURRENT_VERSION: u32 = 7;

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
        ver = 4;
    }

    if ver < 5 {
        let tx = conn.transaction()?;
        // Migration 5: bots, their orders and logs, saved backtests, and per-account risk limits (Bot Studio).
        // Bot positions and trades live in the existing `positions` / `trades` tables, tagged with `bot_id`, so
        // they reserve the same virtual funds as manual orders and show in the Journal. New columns are
        // nullable: existing rows are manual and stay that way. Money columns are decimal TEXT.
        tx.execute_batch(
            "CREATE TABLE bots (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 60),
                description  TEXT NOT NULL DEFAULT '' CHECK (length(description) <= 500),
                avatar       TEXT NOT NULL DEFAULT '',
                market       TEXT NOT NULL CHECK (market IN ('crypto','india')),
                exchange     TEXT NOT NULL CHECK (exchange IN ('binance','fyers')),
                mode         TEXT NOT NULL CHECK (mode IN ('demo','live')),
                account_id   TEXT NOT NULL REFERENCES accounts(id),
                symbol       TEXT NOT NULL,
                symbol2      TEXT,
                interval     TEXT NOT NULL,
                spec         TEXT NOT NULL,
                template     TEXT,
                source       TEXT,
                allocation   TEXT NOT NULL,
                max_daily_loss TEXT,
                max_open_positions INTEGER NOT NULL DEFAULT 10 CHECK (max_open_positions BETWEEN 1 AND 100),
                slippage_pct REAL NOT NULL DEFAULT 0.05,
                status       TEXT NOT NULL DEFAULT 'stopped' CHECK (status IN ('stopped','running','paused')),
                state        TEXT NOT NULL DEFAULT '{}',
                last_price   REAL,
                last_price_utc TEXT,
                last_error   TEXT,
                created_utc  TEXT NOT NULL,
                updated_utc  TEXT NOT NULL,
                started_utc  TEXT
            );
            CREATE TABLE bot_orders (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                bot_id      INTEGER NOT NULL,
                ts_utc      TEXT NOT NULL,
                bar_time    INTEGER,
                symbol      TEXT NOT NULL,
                side        TEXT NOT NULL,
                action      TEXT NOT NULL CHECK (action IN ('open','close')),
                qty         REAL NOT NULL,
                price       REAL NOT NULL,
                status      TEXT NOT NULL CHECK (status IN ('filled','rejected')),
                reason      TEXT NOT NULL DEFAULT '',
                position_id INTEGER,
                trade_id    INTEGER
            );
            CREATE INDEX idx_bot_orders_bot ON bot_orders(bot_id, id);
            CREATE TABLE bot_logs (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                bot_id  INTEGER NOT NULL,
                ts_utc  TEXT NOT NULL,
                level   TEXT NOT NULL,
                message TEXT NOT NULL
            );
            CREATE INDEX idx_bot_logs_bot ON bot_logs(bot_id, id);
            CREATE TABLE backtests (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                bot_id      INTEGER,
                created_utc TEXT NOT NULL,
                request     TEXT NOT NULL,
                metrics     TEXT NOT NULL,
                report      TEXT NOT NULL
            );
            CREATE INDEX idx_backtests_bot ON backtests(bot_id, id);
            CREATE TABLE account_limits (
                account_id         TEXT NOT NULL REFERENCES accounts(id),
                currency           TEXT NOT NULL CHECK (currency IN ('USDT','INR')),
                max_order_notional TEXT,
                max_daily_loss     TEXT,
                max_exposure       TEXT,
                PRIMARY KEY (account_id, currency)
            );
            ALTER TABLE positions ADD COLUMN bot_id INTEGER;
            ALTER TABLE positions ADD COLUMN leg INTEGER;
            ALTER TABLE positions ADD COLUMN stop REAL;
            ALTER TABLE positions ADD COLUMN target REAL;
            ALTER TABLE positions ADD COLUMN trail TEXT;
            ALTER TABLE positions ADD COLUMN trail_ref REAL;
            ALTER TABLE positions ADD COLUMN tag TEXT;
            ALTER TABLE positions ADD COLUMN bar_time INTEGER;
            ALTER TABLE trades ADD COLUMN bot_id INTEGER;
            CREATE INDEX idx_positions_bot ON positions(bot_id);
            CREATE INDEX idx_trades_bot ON trades(bot_id);
            PRAGMA user_version = 5;",
        )?;
        tx.commit()?;
        ver = 5;
    }

    if ver < 6 {
        let tx = conn.transaction()?;
        // Migration 6: broker connections (T007). Only non-secret metadata: the key hint (last 4 characters)
        // and sync times. Credentials live in the OS vault, never here.
        tx.execute_batch(
            "ALTER TABLE accounts ADD COLUMN key_hint TEXT;
            ALTER TABLE accounts ADD COLUMN connected_utc TEXT;
            ALTER TABLE accounts ADD COLUMN last_sync_utc TEXT;
            ALTER TABLE accounts ADD COLUMN last_error TEXT;
            PRAGMA user_version = 6;",
        )?;
        tx.commit()?;
        ver = 6;
    }

    if ver < 7 {
        let tx = conn.transaction()?;
        // Migration 7: immutable strategy versions, bot lifecycle stage and order idempotency, live-deployment
        // approvals, AI proposals and their audit, scoped developer tokens (hash only), scan history and worker
        // heartbeats. Additive only; existing rows keep their meaning.
        tx.execute_batch(
            "CREATE TABLE strategies (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 60),
                created_utc TEXT NOT NULL
            );
            CREATE TABLE strategy_versions (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                strategy_id INTEGER NOT NULL REFERENCES strategies(id),
                version     INTEGER NOT NULL,
                spec        TEXT NOT NULL,
                hash        TEXT NOT NULL,
                note        TEXT NOT NULL DEFAULT '',
                created_utc TEXT NOT NULL,
                UNIQUE (strategy_id, version),
                UNIQUE (strategy_id, hash)
            );
            CREATE TRIGGER strategy_versions_immutable_update BEFORE UPDATE ON strategy_versions
              BEGIN SELECT RAISE(ABORT, 'strategy versions are immutable: publish a new version'); END;
            CREATE TRIGGER strategy_versions_immutable_delete BEFORE DELETE ON strategy_versions
              BEGIN SELECT RAISE(ABORT, 'strategy versions are immutable'); END;
            ALTER TABLE bots ADD COLUMN strategy_version_id INTEGER;
            ALTER TABLE bots ADD COLUMN stage TEXT NOT NULL DEFAULT 'draft';
            ALTER TABLE bot_orders ADD COLUMN client_order_id TEXT;
            ALTER TABLE bot_orders ADD COLUMN broker_order_id TEXT;
            ALTER TABLE bot_orders ADD COLUMN environment TEXT;
            CREATE UNIQUE INDEX idx_bot_orders_client_id ON bot_orders(client_order_id) WHERE client_order_id IS NOT NULL AND status = 'filled';
            ALTER TABLE backtests ADD COLUMN strategy_version_id INTEGER;
            ALTER TABLE backtests ADD COLUMN engine_version TEXT;
            ALTER TABLE backtests ADD COLUMN dataset_hash TEXT;
            CREATE TABLE approvals (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                bot_id        INTEGER NOT NULL,
                strategy_version_id INTEGER NOT NULL,
                spec_hash     TEXT NOT NULL,
                account_id    TEXT NOT NULL,
                scope         TEXT NOT NULL,
                requested_by  TEXT NOT NULL,
                requested_utc TEXT NOT NULL,
                expires_utc   TEXT NOT NULL,
                status        TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','revoked','expired')),
                decided_by    TEXT,
                decided_utc   TEXT,
                reason        TEXT,
                evidence      TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX idx_approvals_bot ON approvals(bot_id, id);
            CREATE TABLE ai_proposals (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                created_utc  TEXT NOT NULL,
                provider     TEXT NOT NULL,
                model        TEXT NOT NULL,
                prompt_version TEXT NOT NULL,
                kind         TEXT NOT NULL,
                objective    TEXT NOT NULL,
                output       TEXT NOT NULL,
                spec         TEXT,
                valid        INTEGER NOT NULL DEFAULT 0,
                errors       TEXT,
                status       TEXT NOT NULL DEFAULT 'draft'
            );
            CREATE TABLE ai_events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_utc      TEXT NOT NULL,
                proposal_id INTEGER,
                kind        TEXT NOT NULL,
                detail      TEXT NOT NULL
            );
            CREATE TABLE api_tokens (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT NOT NULL,
                scopes       TEXT NOT NULL,
                token_hash   TEXT NOT NULL UNIQUE,
                hint         TEXT NOT NULL,
                created_utc  TEXT NOT NULL,
                last_used_utc TEXT,
                revoked_utc  TEXT
            );
            CREATE TABLE scan_runs (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                created_utc TEXT NOT NULL,
                template    TEXT NOT NULL,
                request     TEXT NOT NULL,
                results     TEXT NOT NULL
            );
            CREATE TABLE workers (
                name          TEXT PRIMARY KEY,
                started_utc   TEXT NOT NULL,
                heartbeat_utc TEXT NOT NULL,
                detail        TEXT NOT NULL DEFAULT '{}'
            );
            PRAGMA user_version = 7;",
        )?;
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
