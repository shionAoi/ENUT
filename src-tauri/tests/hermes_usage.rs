use ai_usage_monitor::hermes_usage;
use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "eztyn-hermes-test-{}-{}-{label}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap()
}

fn ts_secs(year: i32, month: u32, day: u32, hour: u32) -> f64 {
    Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
        .unwrap()
        .timestamp() as f64
}

fn create_state_db(path: &Path) -> Connection {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL DEFAULT 'cli',
            model TEXT,
            billing_provider TEXT,
            started_at REAL NOT NULL,
            message_count INTEGER DEFAULT 0,
            input_tokens INTEGER DEFAULT 0,
            output_tokens INTEGER DEFAULT 0,
            cache_read_tokens INTEGER DEFAULT 0,
            cache_write_tokens INTEGER DEFAULT 0,
            reasoning_tokens INTEGER DEFAULT 0
        );",
    )
    .unwrap();
    conn
}

#[test]
fn does_not_double_count_reasoning_tokens_and_skips_null_or_empty_models() {
    let dir = TestDir::new("reasoning-and-nulls");
    let db_path = dir.path().join(".hermes/state.db");
    let conn = create_state_db(&db_path);
    let t = ts_secs(2026, 11, 19, 10) + 0.25;

    // Valid row: reasoning_tokens (300) is a subset of output_tokens (500) -> total = 1000 + 500 + 200 + 50 = 1750
    conn.execute(
        "INSERT INTO sessions (id, model, billing_provider, started_at, message_count, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params!["sess-1", "gpt-5", "openai", t, 12, 1000, 500, 200, 50, 300],
    )
    .unwrap();
    // NULL model -> skipped
    conn.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('sess-null', NULL, ?1, 999, 999)",
        rusqlite::params![t],
    )
    .unwrap();
    // Whitespace-only model -> skipped
    conn.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('sess-empty', '   ', ?1, 999, 999)",
        rusqlite::params![t],
    )
    .unwrap();
    // Zero-token row -> skipped
    conn.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('sess-zero', 'gpt-5', ?1, 0, 0)",
        rusqlite::params![t],
    )
    .unwrap();
    // Invalid timestamp with canary text -> skipped without leaking canary
    conn.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('sess-bad-ts', 'gpt-5', 'SECRET_CANARY_VALUE', 100, 50)",
        [],
    )
    .unwrap();
    drop(conn);

    let summary = hermes_usage::load(dir.path(), now()).unwrap();
    assert_eq!(
        summary.usage.source,
        "Hermes local state database (ccusage method)"
    );
    assert_eq!(summary.usage.input_tokens, 1000);
    assert_eq!(summary.usage.output_tokens, 500);
    assert_eq!(summary.usage.cache_creation_tokens, Some(50));
    assert_eq!(summary.usage.cache_read_tokens, Some(200));
    assert_eq!(summary.usage.cached_tokens, 250);
    assert_eq!(summary.usage.total_tokens, 1750);
    assert_eq!(summary.history.len(), 1);
    assert_eq!(summary.history[0].date, "2026-11-19");
    assert_eq!(summary.history[0].recorded_responses, Some(1));
    assert_eq!(summary.model_usage[0].model, "gpt-5");
    assert!(summary
        .warnings
        .iter()
        .all(|w| !w.contains("SECRET_CANARY_VALUE")));
}

#[test]
fn first_wins_dedupe_across_multiple_roots_and_survives_broken_db() {
    let dir = TestDir::new("first-wins");
    let root1 = dir.path().join("first");
    let root2 = dir.path().join("second");
    let broken = dir.path().join("broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("state.db"), b"not a sqlite database").unwrap();

    let t1 = ts_secs(2026, 11, 18, 8);
    let t2 = ts_secs(2026, 11, 19, 16);

    let c1 = create_state_db(&root1.join("state.db"));
    c1.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('shared', 'claude-sonnet-4', ?1, 100, 20)",
        rusqlite::params![t1],
    )
    .unwrap();
    drop(c1);

    // Second root has SAME session_id with newer timestamp and larger counts -> MUST NOT overwrite first root!
    // Plus a distinct session -> MUST BE INCLUDED.
    let c2 = create_state_db(&root2.join("state.db"));
    c2.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('shared', 'claude-opus-4', ?1, 9000, 9000)",
        rusqlite::params![t2],
    )
    .unwrap();
    c2.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('second-only', 'gpt-5', ?1, 200, 40)",
        rusqlite::params![t2],
    )
    .unwrap();
    drop(c2);

    let summary = hermes_usage::load_from_roots(&[root1, broken, root2], now(), &|| false).unwrap();
    assert_eq!(summary.usage.input_tokens, 300);
    assert_eq!(summary.usage.output_tokens, 60);
    assert_eq!(summary.usage.total_tokens, 360);
    let models: std::collections::BTreeMap<_, _> = summary
        .model_usage
        .iter()
        .map(|m| (m.model.as_str(), m.total_tokens))
        .collect();
    assert_eq!(models.get("claude-sonnet-4"), Some(&120));
    assert_eq!(models.get("gpt-5"), Some(&240));
    assert!(!models.contains_key("claude-opus-4"));
    assert!(summary
        .warnings
        .iter()
        .all(|w| !w.contains(&dir.path().to_string_lossy().to_string())));
}

#[cfg(unix)]
#[test]
fn refuses_symlinks_for_hermes_dir_and_state_db() {
    use std::os::unix::fs::symlink;

    let home = TestDir::new("symlink-home");
    let outside = TestDir::new("symlink-outside");
    let t = ts_secs(2026, 11, 19, 10);

    let outside_db = outside.path().join("state.db");
    let conn = create_state_db(&outside_db);
    conn.execute(
        "INSERT INTO sessions (id, model, started_at, input_tokens, output_tokens) VALUES ('secret', 'gpt-5', ?1, 500, 500)",
        rusqlite::params![t],
    )
    .unwrap();
    drop(conn);

    // Symlinked state.db
    let hermes_dir = home.path().join(".hermes");
    std::fs::create_dir_all(&hermes_dir).unwrap();
    symlink(&outside_db, hermes_dir.join("state.db")).unwrap();

    let err = hermes_usage::load(home.path(), now()).err().unwrap();
    assert!(!err.contains("state.db"));
    assert!(!err.contains(&outside.path().to_string_lossy().to_string()));

    // Symlinked .hermes dir
    let home2 = TestDir::new("symlink-dir");
    symlink(outside.path(), home2.path().join(".hermes")).unwrap();
    assert!(hermes_usage::load(home2.path(), now()).is_err());
}

#[test]
fn load_from_roots_honors_cancellation() {
    let dir = TestDir::new("cancel");
    let root = dir.path().join(".hermes");
    std::fs::create_dir_all(&root).unwrap();
    let err = hermes_usage::load_from_roots(&[root], now(), &|| true)
        .err()
        .unwrap();
    assert!(err.to_ascii_lowercase().contains("cancel"));
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_sqlite_wal_sidecar_without_reading_or_leaking_target() {
    let dir = TestDir::new("symlink-wal");
    let hermes_dir = dir.path().join(".hermes");
    std::fs::create_dir_all(&hermes_dir).unwrap();
    let db_path = hermes_dir.join("state.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE sessions (
             id TEXT PRIMARY KEY,
             model TEXT,
             billing_provider TEXT,
             started_at REAL NOT NULL,
             message_count INTEGER DEFAULT 0,
             input_tokens INTEGER DEFAULT 0,
             output_tokens INTEGER DEFAULT 0,
             cache_read_tokens INTEGER DEFAULT 0,
             cache_write_tokens INTEGER DEFAULT 0,
             reasoning_tokens INTEGER DEFAULT 0
         );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions VALUES ('s1', 'claude-sonnet-4', 'anthropic', ?1, 1, 100, 20, 10, 5, 3)",
        [1_795_000_000.0_f64],
    )
    .unwrap();
    drop(conn);

    let outside = dir.path().join("outside-secret-canary");
    std::fs::write(&outside, b"PRIVATE-WAL-CANARY").unwrap();
    let wal_path = hermes_dir.join("state.db-wal");
    let _ = std::fs::remove_file(&wal_path);
    std::os::unix::fs::symlink(&outside, &wal_path).unwrap();

    let err = hermes_usage::load(dir.path(), now()).err().unwrap();
    assert!(!err.contains("outside-secret-canary") && !err.contains("PRIVATE-WAL-CANARY"));
    assert_eq!(std::fs::read(&outside).unwrap(), b"PRIVATE-WAL-CANARY");
}
