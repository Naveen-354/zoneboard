use std::fs;
use std::path::PathBuf;
use rusqlite::Connection;
use zoneboard::db::{self, DbError, CURRENT_VERSION};
use zoneboard::journal::Journal;
use std::sync::Mutex;

// Any test touching ZONEBOARD_DATA must take this lock to prevent parallel test races.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn temp_db_path(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("zb_test_{name}_{}_{}.db", std::process::id(), fastrand()));
    let _ = fs::remove_file(&p);
    p
}

fn fastrand() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
}

fn create_from_fixture(fixture_name: &str) -> PathBuf {
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(fixture_name);
    let sql = fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", fixture_path.display()));

    let db_path = temp_db_path(fixture_name);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(&sql).unwrap();
    drop(conn);

    db_path
}

#[test]
fn fixture_v0_no_market_migrates_cleanly() {
    let db_path = create_from_fixture("legacy_v0_no_market.sql");
    let path_str = db_path.to_str().unwrap();

    let mut conn = Connection::open(path_str).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), 0);
    db::migrate(&mut conn).unwrap();

    assert_eq!(db::user_version(&conn).unwrap(), CURRENT_VERSION);

    // Verify trades table has market and mode columns
    let (market, mode, notes): (String, String, String) = conn
        .query_row("SELECT market, mode, notes FROM trades WHERE symbol = 'SBIN'", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(market, "india");
    assert_eq!(mode, "live");
    assert_eq!(notes, "legacy trade without market");

    // Verify positions table exists
    let pos_count: i64 = conn.query_row("SELECT COUNT(*) FROM positions", [], |r| r.get(0)).unwrap();
    assert_eq!(pos_count, 0);

    drop(conn);
    let _ = fs::remove_file(&db_path);
}

#[test]
fn fixture_v0_no_mode_migrates_cleanly() {
    let db_path = create_from_fixture("legacy_v0_no_mode.sql");
    let path_str = db_path.to_str().unwrap();

    let mut conn = Connection::open(path_str).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), 0);
    db::migrate(&mut conn).unwrap();

    assert_eq!(db::user_version(&conn).unwrap(), CURRENT_VERSION);

    let (tcs_mode, tcs_market): (String, String) = conn
        .query_row("SELECT mode, market FROM trades WHERE symbol = 'TCS'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(tcs_mode, "live");
    assert_eq!(tcs_market, "india");

    let (btc_mode, btc_market): (String, String) = conn
        .query_row("SELECT mode, market FROM trades WHERE symbol = 'BTCUSDT'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!(btc_mode, "live");
    assert_eq!(btc_market, "crypto");

    drop(conn);
    let _ = fs::remove_file(&db_path);
}

#[test]
fn fixture_v0_paper_live_migrates_paper_trades_and_positions_to_demo() {
    let db_path = create_from_fixture("legacy_v0_paper_live.sql");
    let path_str = db_path.to_str().unwrap();

    let journal = Journal::open(path_str).unwrap();

    // BTCUSDT was 'Paper trade' under 'live' -> must now be under 'demo'
    let demo_crypto = journal.payload("crypto", "demo");
    assert_eq!(demo_crypto.trades.len(), 1);
    assert_eq!(demo_crypto.trades[0].symbol, "BTCUSDT");
    assert_eq!(demo_crypto.trades[0].mode, "demo");

    let live_crypto = journal.payload("crypto", "live");
    assert_eq!(live_crypto.trades.len(), 0);

    // INFY was a real manual broker trade under 'live' -> must remain 'live'
    let live_india = journal.payload("india", "live");
    assert_eq!(live_india.trades.len(), 1);
    assert_eq!(live_india.trades[0].symbol, "INFY");
    assert_eq!(live_india.trades[0].mode, "live");

    // Position was under 'live' -> must now be under 'demo'
    let live_pos = journal.positions("crypto", "live");
    assert_eq!(live_pos.len(), 0);

    let demo_pos = journal.positions("crypto", "demo");
    assert_eq!(demo_pos.len(), 1);
    assert_eq!(demo_pos[0].symbol, "ETHUSDT");
    assert_eq!(demo_pos[0].mode, "demo");

    drop(journal);
    let _ = fs::remove_file(&db_path);
}

#[test]
fn fixture_v0_clean_no_version_migrates_cleanly() {
    let db_path = create_from_fixture("legacy_v0_clean_no_version.sql");
    let path_str = db_path.to_str().unwrap();

    let conn = db::open(path_str).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), CURRENT_VERSION);

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);

    drop(conn);
    let _ = fs::remove_file(&db_path);
}

#[test]
fn newer_user_version_is_refused_without_modification() {
    let db_path = temp_db_path("newer_ver");
    let path_str = db_path.to_str().unwrap();

    {
        let conn = Connection::open(path_str).unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
    }

    // Attempting to open via db::open or Journal::open must fail
    match db::open(path_str) {
        Err(DbError::VersionTooNew { found, max_supported }) => {
            assert_eq!(found, 99);
            assert_eq!(max_supported, CURRENT_VERSION);
        }
        Err(other) => panic!("expected VersionTooNew error, got: {other:?}"),
        Ok(_) => panic!("expected db::open to fail for newer version"),
    }

    match Journal::open(path_str) {
        Err(j_err) => {
            assert!(j_err.to_string().contains("database version 99 is newer than supported version"));
        }
        Ok(_) => panic!("expected Journal::open to fail for newer version"),
    }

    // Verify database was not modified or downgraded
    let conn = Connection::open(path_str).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), 99);
    drop(conn);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn migrations_are_idempotent() {
    let db_path = temp_db_path("idempotent");
    let path_str = db_path.to_str().unwrap();

    let mut conn = db::open(path_str).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), CURRENT_VERSION);

    // Run migrate again
    db::migrate(&mut conn).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), CURRENT_VERSION);

    // Run migrate a third time
    db::migrate(&mut conn).unwrap();
    assert_eq!(db::user_version(&conn).unwrap(), CURRENT_VERSION);

    drop(conn);
    let _ = fs::remove_file(&db_path);
}

#[test]
fn data_dir_resolution_does_not_create_directory() {
    let _lock = ENV_LOCK.lock().unwrap();
    let unique_dir = std::env::temp_dir().join(format!("zb_non_existent_{}_{}", std::process::id(), fastrand()));
    let _ = fs::remove_dir_all(&unique_dir);

    // Set ZONEBOARD_DATA to point to a non-existent path
    unsafe {
        std::env::set_var("ZONEBOARD_DATA", unique_dir.to_str().unwrap());
    }

    let resolved = db::data_dir();
    assert_eq!(resolved, unique_dir);
    // Verifying rule: data_dir() must NOT create directories when resolving
    assert!(!unique_dir.exists(), "data_dir() must not create directories");

    // ensure_data_dir() DOES create directory
    let ensured = db::ensure_data_dir().unwrap();
    assert_eq!(ensured, unique_dir);
    assert!(unique_dir.exists(), "ensure_data_dir() must create directory");

    let _ = fs::remove_dir_all(&unique_dir);
    unsafe {
        std::env::remove_var("ZONEBOARD_DATA");
    }
}

#[test]
fn fixture_v2_migrates_to_latest_with_data_intact() {
    let db_path = create_from_fixture("legacy_v0_paper_live.sql");
    let path_str = db_path.to_str().unwrap();

    let mut conn = Connection::open(path_str).unwrap();

    // Run migrations 1 and 2 manually to bring the legacy fixture to v2 state
    conn.execute_batch(
        "UPDATE trades SET mode = 'demo' WHERE mode = 'live' AND notes = 'Paper trade';
         UPDATE positions SET mode = 'demo' WHERE mode = 'live';
         PRAGMA user_version = 2;",
    ).unwrap();

    assert_eq!(db::user_version(&conn).unwrap(), 2);

    // Verify v2 data counts before v3 migration
    let trade_count_v2: i64 = conn.query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0)).unwrap();
    let pos_count_v2: i64 = conn.query_row("SELECT COUNT(*) FROM positions", [], |r| r.get(0)).unwrap();
    assert_eq!(trade_count_v2, 2);
    assert_eq!(pos_count_v2, 1);

    // Run migration to v3
    db::migrate(&mut conn).unwrap();

    assert_eq!(db::user_version(&conn).unwrap(), CURRENT_VERSION);
    assert_eq!(db::user_version(&conn).unwrap(), 7);

    // Verify data is completely intact
    let trade_count_v3: i64 = conn.query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0)).unwrap();
    let pos_count_v3: i64 = conn.query_row("SELECT COUNT(*) FROM positions", [], |r| r.get(0)).unwrap();
    assert_eq!(trade_count_v3, trade_count_v2);
    assert_eq!(pos_count_v3, pos_count_v2);

    let (btc_sym, btc_mode): (String, String) = conn.query_row(
        "SELECT symbol, mode FROM trades WHERE symbol = 'BTCUSDT'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(btc_sym, "BTCUSDT");
    assert_eq!(btc_mode, "demo");

    // Verify audit_log and settings tables and triggers are active
    // exactly one row: migration 4's `account_created` for demo-1 (nothing else has written yet)
    let audit_count: i64 = conn.query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0)).unwrap();
    assert_eq!(audit_count, 1);
    let first: String = conn.query_row("SELECT action FROM audit_log", [], |r| r.get(0)).unwrap();
    assert_eq!(first, "account_created");

    let settings_count: i64 = conn.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0)).unwrap();
    assert_eq!(settings_count, 0);

    // Insert into audit_log and verify append-only trigger works (id 1 is migration 4's seed row)
    conn.execute(
        "INSERT INTO audit_log (id, ts_utc, actor, action, entity, detail_json)
         VALUES (2, '2026-11-03T00:00:00Z', 'system', 'test', 'system', '{}')",
        [],
    ).unwrap();
    let res = conn.execute("UPDATE audit_log SET actor = 'bad' WHERE id = 2", []);
    assert!(res.is_err(), "UPDATE on audit_log must be blocked by trigger");
    // the seed row cannot be replaced either
    let res = conn.execute(
        "INSERT INTO audit_log (id, ts_utc, actor, action, entity, detail_json) VALUES (1, 'x', 'bad', 'bad', 'bad', '{}')",
        [],
    );
    assert!(res.is_err(), "the seed audit row must not be replaceable");

    drop(conn);
    let _ = fs::remove_file(&db_path);
}


// ------------------------------------------------------------------ migration 4: accounts (T005)

fn q<T: rusqlite::types::FromSql>(c: &Connection, sql: &str) -> T {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn migration_4_on_a_populated_v3_database_links_demo_rows_and_seeds_the_wallet() {
    let path = create_from_fixture("legacy_v3_demo.sql");
    let mut c = Connection::open(&path).unwrap();
    assert_eq!(db::user_version(&c).unwrap(), 3);
    db::migrate(&mut c).unwrap(); // would fail here if account_id were NOT NULL DEFAULT ... REFERENCES

    assert_eq!(db::user_version(&c).unwrap(), 7);
    // demo rows are linked to demo-1, the live ones belong to no account
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM trades WHERE mode = 'demo' AND account_id = 'demo-1'"), 4);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM trades WHERE mode = 'demo'"), 4);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM trades WHERE mode = 'live' AND account_id IS NULL"), 1);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM positions WHERE account_id = 'demo-1'"), 2);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM trades"), 5, "no trade lost");

    // wallets: only the two paper-engine trades moved USDT (8.5 + 98.43); hand-entered ones and INR did not
    let wallet = |cur: &str, col: &str| -> String { c.query_row(&format!("SELECT {col} FROM demo_wallets WHERE account_id = 'demo-1' AND currency = ?1"), [cur], |r| r.get(0)).unwrap() };
    assert_eq!((wallet("USDT", "credits"), wallet("USDT", "balance")), ("10000".to_string(), "10106.93".to_string()));
    assert_eq!((wallet("INR", "credits"), wallet("INR", "balance")), ("1000000".to_string(), "1000000".to_string()));

    // through the journal: reserved and available are exact
    let j = Journal::open(path.to_str().unwrap()).unwrap();
    let accounts = j.with_conn(|c| zoneboard::accounts::list(c)).unwrap();
    let a = &accounts["accounts"][0];
    let w = |cur: &str| a["wallets"].as_array().unwrap().iter().find(|w| w["currency"] == cur).unwrap().clone();
    assert_eq!((w("USDT")["in_positions"].as_str(), w("USDT")["available"].as_str()), (Some("2000"), Some("8106.93")));
    assert_eq!((w("INR")["in_positions"].as_str(), w("INR")["available"].as_str()), (Some("5000"), Some("995000")));
    assert_eq!(a["virtual"], true);
    assert_eq!(j.payload("crypto", "demo").trades.len(), 3);
    assert_eq!(j.positions("crypto", "demo").len(), 1);

    // exactly one audit row, by `system`, with the seed numbers
    let (n, actor, action, entity, detail): (i64, String, String, String, String) =
        c.query_row("SELECT count(*), actor, action, entity, detail_json FROM audit_log", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap();
    assert_eq!((n, actor.as_str(), action.as_str(), entity.as_str()), (1, "system", "account_created", "account:demo-1"));
    let d: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!((d["linked_trades"].as_i64(), d["linked_positions"].as_i64(), d["skipped_trades"].as_i64()), (Some(4), Some(2), Some(0)));
    assert_eq!(d["seeded_realised"]["USDT"], "106.93");
    assert_eq!(d["credits"]["USDT"], "10000");
    drop(c);
    drop(j);
    let _ = fs::remove_file(&path);
}

#[test]
fn migration_4_on_a_fresh_database_creates_demo_1_with_default_credits() {
    let c = db::open(":memory:").unwrap();
    assert_eq!(db::user_version(&c).unwrap(), 7);
    let (id, kind, label, env, status, archived): (String, String, String, String, String, i64) =
        c.query_row("SELECT id, kind, label, environment, status, archived FROM accounts", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))).unwrap();
    assert_eq!((id.as_str(), kind.as_str(), label.as_str(), env.as_str(), status.as_str(), archived), ("demo-1", "demo", "Demo", "paper", "active", 0));
    assert_eq!(q::<String>(&c, "SELECT credits || '/' || balance FROM demo_wallets WHERE currency = 'USDT'"), "10000/10000");
    assert_eq!(q::<String>(&c, "SELECT credits || '/' || balance FROM demo_wallets WHERE currency = 'INR'"), "1000000/1000000");
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM audit_log WHERE action = 'account_created'"), 1);
}

#[test]
fn migration_4_runs_once() {
    let path = create_from_fixture("legacy_v3_demo.sql");
    for _ in 0..3 {
        let mut c = Connection::open(&path).unwrap();
        db::migrate(&mut c).unwrap();
    }
    let c = Connection::open(&path).unwrap();
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM accounts"), 1);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM audit_log"), 1);
    assert_eq!(q::<String>(&c, "SELECT balance FROM demo_wallets WHERE currency = 'USDT'"), "10106.93", "a re-open must not replay the seed");
    drop(c);
    let _ = fs::remove_file(&path);
}

#[test]
fn legacy_writers_that_add_demo_rows_without_an_account_still_get_demo_1() {
    let c = db::open(":memory:").unwrap();
    c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES ('2026-11-02','BTCUSDT','long',1,1,2,0,'','crypto','demo')", []).unwrap();
    c.execute("INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES ('crypto','demo','BTCUSDT','long',1,1,0.1,'x')", []).unwrap();
    assert_eq!(q::<String>(&c, "SELECT account_id FROM trades"), "demo-1");
    assert_eq!(q::<String>(&c, "SELECT account_id FROM positions"), "demo-1");
    // live rows stay unlinked: the trigger only covers mode = 'demo'
    c.execute("INSERT INTO trades (date, symbol, side, qty, entry, exit, fees, notes, market, mode) VALUES ('2026-11-02','SBIN','long',1,1,2,0,'','india','live')", []).unwrap();
    c.execute("INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at) VALUES ('india','live','SBIN','long',1,1,0.05,'x')", []).unwrap();
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM trades WHERE account_id IS NULL"), 1);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM positions WHERE account_id IS NULL"), 1);
}

#[test]
fn foreign_keys_are_enforced_and_schema_checks_hold() {
    let c = db::open(":memory:").unwrap();
    let bad = c.execute("INSERT INTO positions (market, mode, symbol, side, qty, entry, fee_pct, opened_at, account_id) VALUES ('crypto','demo','BTCUSDT','long',1,1,0.1,'x','nope')", []);
    assert!(bad.is_err(), "a position for a missing account must be refused by the database");
    assert!(c.execute("INSERT INTO demo_wallets (account_id, currency, credits, balance) VALUES ('nope','USDT','1','1')", []).is_err());
    assert!(c.execute("INSERT INTO accounts (id, kind, label, environment, created_utc) VALUES ('x','demo','x','live','now')", []).is_err(), "a demo account is always paper");
    assert!(c.execute("INSERT INTO accounts (id, kind, label, environment, created_utc) VALUES ('x','kraken','x','paper','now')", []).is_err());
}

#[test]
fn migration_5_on_a_populated_database_adds_bots_and_leaves_existing_rows_manual() {
    let path = create_from_fixture("legacy_v3_demo.sql");
    let mut c = Connection::open(&path).unwrap();
    let (trades, positions): (i64, i64) = (q(&c, "SELECT count(*) FROM trades"), q(&c, "SELECT count(*) FROM positions"));
    db::migrate(&mut c).unwrap();
    assert_eq!(db::user_version(&c).unwrap(), 7);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM trades"), trades);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM positions"), positions);
    // every existing row is manual: no bot owns it
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM trades WHERE bot_id IS NOT NULL"), 0);
    assert_eq!(q::<i64>(&c, "SELECT count(*) FROM positions WHERE bot_id IS NOT NULL"), 0);
    for t in ["bots", "bot_orders", "bot_logs", "backtests", "account_limits"] {
        assert_eq!(q::<i64>(&c, &format!("SELECT count(*) FROM {t}")), 0, "{t}");
    }
    // a bot row can be written against the seeded demo account, and a bad mode is refused by the schema
    c.execute("INSERT INTO bots (name, market, exchange, mode, account_id, symbol, interval, spec, allocation, created_utc, updated_utc) VALUES ('b','crypto','binance','demo','demo-1','BTCUSDT','1h','{}','100','x','x')", []).unwrap();
    assert!(c.execute("INSERT INTO bots (name, market, exchange, mode, account_id, symbol, interval, spec, allocation, created_utc, updated_utc) VALUES ('b','crypto','binance','paper','demo-1','BTCUSDT','1h','{}','100','x','x')", []).is_err());
    // re-running is a no-op
    db::migrate(&mut c).unwrap();
    assert_eq!(db::user_version(&c).unwrap(), 7);
    drop(c);
    let _ = std::fs::remove_file(&path);
}
