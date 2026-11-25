use ai_usage_monitor::opencode_usage;
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
            "eztyn-opencode-test-{}-{}-{label}",
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

fn ts_ms(year: i32, month: u32, day: u32, hour: u32) -> i64 {
    Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
        .unwrap()
        .timestamp_millis()
}

#[test]
fn reads_sqlite_v1_messages_and_normalizes_tokens_and_models() {
    let dir = TestDir::new("v1");
    let data_dir = dir.path().join(".local/share/opencode");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = data_dir.join("opencode.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            time_created INTEGER,
            data TEXT NOT NULL
        );",
    )
    .unwrap();

    let created_1 = ts_ms(2026, 11, 18, 10);
    let created_2 = ts_ms(2026, 11, 19, 15);
    // Message 1: standard v1 payload with reasoning + cache read/write
    let data_1 = format!(
        r#"{{
            "modelID": "claude-sonnet-4",
            "providerID": "anthropic",
            "time": {{"created": {created_1}}},
            "tokens": {{
                "input": 1000,
                "output": 200,
                "reasoning": 50,
                "cache": {{"read": 300, "write": 100}},
                "total": 1650
            }}
        }}"#
    );
    // Message 2: nested model object, already provider-prefixed, falls back to time_created column,
    // and reported total > component sum
    let data_2 = r#"{
        "model": {"id": "openai:gpt-5", "providerID": "openai"},
        "tokens": {
            "input": 400,
            "output": 100,
            "cache": {"read": 0, "write": 0},
            "total": 600
        }
    }"#;
    // Message 3: zero tokens -> skipped
    let data_3 = format!(
        r#"{{
            "modelID": "claude-sonnet-4",
            "providerID": "anthropic",
            "time": {{"created": {created_1}}},
            "tokens": {{"input": 0, "output": 0}}
        }}"#
    );

    conn.execute(
        "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["msg-1", "sess-1", created_1, data_1],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["msg-2", "sess-1", created_2, data_2],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["msg-zero", "sess-1", created_1, data_3],
    )
    .unwrap();
    // Keep `conn` open so the live WAL/SHM sidecars exist while we read strictly read-only,
    // and also place orphan sidecar files to prove discovery skips -wal/-shm/-journal files.
    std::fs::write(data_dir.join("opencode-orphan.db-wal"), b"ignored").unwrap();
    std::fs::write(data_dir.join("opencode-orphan.db-shm"), b"ignored").unwrap();
    std::fs::write(data_dir.join("opencode-orphan.db-journal"), b"ignored").unwrap();

    let summary = opencode_usage::load(dir.path(), now()).unwrap();
    drop(conn);
    assert_eq!(summary.usage.source, "OpenCode local database (ccusage method)");
    assert_eq!(summary.usage.input_tokens, 1400);
    assert_eq!(summary.usage.output_tokens, 350); // 200 + 50 + 100
    assert_eq!(summary.usage.cache_creation_tokens, Some(100));
    assert_eq!(summary.usage.cache_read_tokens, Some(300));
    assert_eq!(summary.usage.cached_tokens, 400);
    assert_eq!(summary.usage.total_tokens, 2250); // 1650 + 600
    assert_eq!(summary.history.len(), 2);

    let models: std::collections::BTreeMap<_, _> = summary
        .model_usage
        .iter()
        .map(|m| (m.model.as_str(), m.total_tokens))
        .collect();
    assert_eq!(models.get("anthropic:claude-sonnet-4"), Some(&1650));
    assert_eq!(models.get("openai:gpt-5"), Some(&600));
}

#[test]
fn reads_sqlite_v2_excludes_fork_copies_and_failed_compactions() {
    let dir = TestDir::new("v2-forks");
    let data_dir = dir.path().join(".local/share/opencode");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = data_dir.join("opencode-beta.db");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session_v2 (
            id TEXT PRIMARY KEY,
            fork_session_id TEXT,
            fork_boundary TEXT
        );
        CREATE TABLE session_message (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            type TEXT NOT NULL,
            seq INTEGER,
            time_created INTEGER,
            data TEXT NOT NULL
        );",
    )
    .unwrap();

    let t = ts_ms(2026, 11, 19, 10);

    // Parent session with sparse seq values: 1, 5, 10
    conn.execute(
        "INSERT INTO session_v2 (id, fork_session_id, fork_boundary) VALUES ('parent', NULL, NULL)",
        [],
    )
    .unwrap();
    // Fork A: "through" message p2 (seq 5) -> skip seq <= 5, keep seq > 5
    conn.execute(
        "INSERT INTO session_v2 (id, fork_session_id, fork_boundary) VALUES ('fork-through', 'parent', '{\"type\":\"through\",\"messageID\":\"p2\"}')",
        [],
    )
    .unwrap();
    // Fork B: "before" message p3 (seq 10) -> preceding parent seq is 5 -> skip seq <= 5, keep seq = 6
    conn.execute(
        "INSERT INTO session_v2 (id, fork_session_id, fork_boundary) VALUES ('fork-before', 'parent', '{\"type\":\"before\",\"messageID\":\"p3\"}')",
        [],
    )
    .unwrap();

    let make_msg = |input: u64, output: u64, status: Option<&str>| -> String {
        let status_part = status
            .map(|s| format!(r#","status":"{s}""#))
            .unwrap_or_default();
        format!(
            r#"{{"model":{{"id":"claude-opus-4","providerID":"anthropic"}},"time":{{"created":{t}}},"tokens":{{"input":{input},"output":{output}}}{status_part}}}"#
        )
    };

    // Parent rows
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["p1", "parent", "user", 1, t, make_msg(999, 999, None)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["p2", "parent", "assistant", 5, t, make_msg(100, 20, None)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["p3", "parent", "assistant", 10, t, make_msg(200, 40, None)],
    )
    .unwrap();

    // Fork-through: copied row seq=5 (must skip!) + fork-local seq=6 (must keep!)
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["ft-copy", "fork-through", "assistant", 5, t, make_msg(100, 20, None)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["ft-new", "fork-through", "assistant", 6, t, make_msg(300, 60, None)],
    )
    .unwrap();

    // Fork-before: copied row seq=5 (must skip!) + fork-local seq=6 (must keep!)
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["fb-copy", "fork-before", "assistant", 5, t, make_msg(100, 20, None)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["fb-new", "fork-before", "assistant", 6, t, make_msg(400, 80, None)],
    )
    .unwrap();

    // Compactions: completed (keep) vs failed (skip) vs missing status (skip)
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["cmp-ok", "parent", "compaction", 11, t, make_msg(50, 10, Some("completed"))],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["cmp-fail", "parent", "compaction", 12, t, make_msg(500, 100, Some("failed"))],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["cmp-none", "parent", "compaction", 13, t, make_msg(500, 100, None)],
    )
    .unwrap();
    drop(conn);

    let summary = opencode_usage::load(dir.path(), now()).unwrap();
    // Expected kept: p2(100,20) + p3(200,40) + ft-new(300,60) + fb-new(400,80) + cmp-ok(50,10)
    assert_eq!(summary.usage.input_tokens, 1050);
    assert_eq!(summary.usage.output_tokens, 210);
    assert_eq!(summary.usage.total_tokens, 1260);
    assert_eq!(summary.history[0].recorded_responses, Some(5));
}

#[test]
fn reads_legacy_json_fallback_and_deduplicates_against_sqlite_claimed_ids() {
    let dir = TestDir::new("legacy-dedupe");
    let data_dir = dir.path().join(".local/share/opencode");
    let msg_dir = data_dir.join("storage/message/sess-1");
    std::fs::create_dir_all(&msg_dir).unwrap();

    let t = ts_ms(2026, 11, 19, 12);

    // SQLite DB claims msg-shared
    let conn = Connection::open(data_dir.join("opencode.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, data TEXT NOT NULL);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
        rusqlite::params![
            "msg-shared",
            "sess-1",
            format!(
                r#"{{"modelID":"gpt-5","providerID":"openai","time":{{"created":{t}}},"tokens":{{"input":100,"output":25}}}}"#
            )
        ],
    )
    .unwrap();
    drop(conn);

    // Legacy JSON with same stem (must be skipped!)
    std::fs::write(
        msg_dir.join("msg-shared.json"),
        format!(
            r#"{{"id":"msg-shared","modelID":"gpt-5","providerID":"openai","time":{{"created":{t}}},"tokens":{{"input":9999,"output":9999}}}}"#
        ),
    )
    .unwrap();

    // Legacy JSON unique message (must be included!)
    std::fs::write(
        msg_dir.join("msg-legacy-only.json"),
        format!(
            r#"{{"modelID":"gemini-2.5-pro","time":{{"created":{t}}},"tokens":{{"input":50,"output":10,"cache":{{"read":5,"write":15}}}}}}"#
        ),
    )
    .unwrap();

    let summary = opencode_usage::load(dir.path(), now()).unwrap();
    assert_eq!(summary.usage.input_tokens, 150);
    assert_eq!(summary.usage.output_tokens, 35);
    assert_eq!(summary.usage.cache_creation_tokens, Some(15));
    assert_eq!(summary.usage.cache_read_tokens, Some(5));
    assert_eq!(summary.usage.total_tokens, 205);
    assert_eq!(summary.history[0].recorded_responses, Some(2));
    let models: std::collections::BTreeMap<_, _> = summary
        .model_usage
        .iter()
        .map(|m| (m.model.as_str(), m.total_tokens))
        .collect();
    assert_eq!(models.get("openai:gpt-5"), Some(&125));
    assert_eq!(models.get("gemini-2.5-pro"), Some(&80));
}

#[cfg(unix)]
#[test]
fn refuses_symlinks_for_database_and_legacy_json() {
    use std::os::unix::fs::symlink;

    let home = TestDir::new("symlink-home");
    let outside = TestDir::new("symlink-outside");
    let t = ts_ms(2026, 11, 19, 12);

    // Create a valid DB outside home
    let outside_db = outside.path().join("secret.db");
    let conn = Connection::open(&outside_db).unwrap();
    conn.execute_batch(
        "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, data TEXT NOT NULL);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message (id, session_id, data) VALUES ('secret-msg', 's1', ?1)",
        rusqlite::params![format!(
            r#"{{"modelID":"gpt-5","time":{{"created":{t}}},"tokens":{{"input":500,"output":500}}}}"#
        )],
    )
    .unwrap();
    drop(conn);

    // Create a valid JSON outside home
    let outside_json = outside.path().join("secret.json");
    std::fs::write(
        &outside_json,
        format!(
            r#"{{"id":"secret-json","modelID":"gpt-5","time":{{"created":{t}}},"tokens":{{"input":500,"output":500}}}}"#
        ),
    )
    .unwrap();

    let data_dir = home.path().join(".local/share/opencode");
    let msg_dir = data_dir.join("storage/message/sess-1");
    std::fs::create_dir_all(&msg_dir).unwrap();
    symlink(&outside_db, data_dir.join("opencode.db")).unwrap();
    symlink(&outside_json, msg_dir.join("secret.json")).unwrap();

    let err = opencode_usage::load(home.path(), now()).err().unwrap();
    assert!(!err.contains("secret.db"));
    assert!(!err.contains(&outside.path().to_string_lossy().to_string()));

    // Also test symlinked root directory
    let home2 = TestDir::new("symlink-root");
    std::fs::create_dir_all(home2.path().join(".local/share")).unwrap();
    symlink(outside.path(), home2.path().join(".local/share/opencode")).unwrap();
    assert!(opencode_usage::load(home2.path(), now()).is_err());
}

#[test]
fn load_from_roots_honors_cancellation() {
    let dir = TestDir::new("cancel");
    let root = dir.path().join("custom");
    std::fs::create_dir_all(&root).unwrap();
    let err = opencode_usage::load_from_roots(&[root], now(), &|| true)
        .err()
        .unwrap();
    assert!(err.to_ascii_lowercase().contains("cancel"));
}
