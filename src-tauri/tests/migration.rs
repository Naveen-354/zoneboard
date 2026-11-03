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
fn fixture_v2_migrates_to_v3_with_data_intact() {
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
    assert_eq!(db::user_version(&conn).unwrap(), 3);

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
    let audit_count: i64 = conn.query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0)).unwrap();
    assert_eq!(audit_count, 0);

    let settings_count: i64 = conn.query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0)).unwrap();
    assert_eq!(settings_count, 0);

    // Insert into audit_log and verify append-only trigger works
    conn.execute(
        "INSERT INTO audit_log (id, ts_utc, actor, action, entity, detail_json)
         VALUES (1, '2026-11-03T00:00:00Z', 'system', 'test', 'system', '{}')",
        [],
    ).unwrap();
    let res = conn.execute("UPDATE audit_log SET actor = 'bad' WHERE id = 1", []);
    assert!(res.is_err(), "UPDATE on audit_log must be blocked by trigger");

    drop(conn);
    let _ = fs::remove_file(&db_path);
}
