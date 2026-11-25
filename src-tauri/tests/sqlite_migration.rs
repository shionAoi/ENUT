use ai_usage_monitor::{Discovery, Observation, Service, StatusReader};
use std::{
    future::Future,
    ops::Deref,
    path::{Path, PathBuf},
    pin::Pin,
};

struct Found;
impl Discovery for Found {
    fn installed(&self, id: &str) -> bool {
        id == "opencode"
    }
}
struct Offline;
impl StatusReader for Offline {
    fn read(&self, _: String) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
        Box::pin(async { Err("Offline".into()) })
    }
}

struct TestRoot(PathBuf);
impl Deref for TestRoot {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn root(name: &str) -> TestRoot {
    let root = std::env::temp_dir().join(format!("sqlite-migration-{}-{name}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    TestRoot(root)
}

const STORAGE_ERROR: &str = "Local history storage unavailable. Saved permissions/history could not be confirmed; check local storage and retry. Revoke failures may leave saved data on disk.";

fn private_file(_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn write_legacy(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    private_file(path);
}

fn legacy_opencode(total: u64) -> Vec<u8> {
    let stamp = "2026-11-20T00:00:00Z";
    serde_json::to_vec(&serde_json::json!({"version":1,"tools":{
        "opencode":{"allowed":true,"usage":{"inputTokens":total - 3,"outputTokens":2,"cachedTokens":1,"totalTokens":total,
        "spendUsd":null,"source":"Local statistics","scope":"device","asOf":stamp,"periodStart":stamp,"periodEnd":stamp},
        "history":[],"models":[],"model_history":[],"partial":null,"updated_at":stamp}
    }})).unwrap()
}

fn assert_storage_unavailable(service: &Service<Found, Offline>) {
    let snapshot = service.scan_agents();
    assert!(snapshot.warnings.iter().any(|w| w == STORAGE_ERROR));
    assert!(!snapshot.agents[0].monitoring_allowed);
    assert!(snapshot.agents[0].usage.is_none());
    assert_eq!(
        service.set_permission("opencode", true).err().as_deref(),
        Some(STORAGE_ERROR)
    );
    assert_eq!(
        service.set_permission("opencode", false).err().as_deref(),
        Some(STORAGE_ERROR)
    );
}

#[test]
fn imports_legacy_usage_once_and_does_not_restore_revoked_state() {
    let root = root("once");
    let legacy = root.join("monitor-state.json");
    write_legacy(&legacy, &legacy_opencode(13));
    {
        let service = Service::with_storage(Found, Offline, &root);
        let snapshot = service.scan_agents();
        assert!(snapshot.agents[0].monitoring_allowed && snapshot.agents[0].history_cached);
        assert_eq!(snapshot.agents[0].usage.as_ref().unwrap().total_tokens, 13);
        assert!(!legacy.try_exists().unwrap());
    }
    let database = root.join("monitor-usage.sqlite3");
    assert!(std::fs::read(&database)
        .unwrap()
        .starts_with(b"SQLite format 3\0"));
    {
        let service = Service::with_storage(Found, Offline, &root);
        assert_eq!(
            service.scan_agents().agents[0]
                .usage
                .as_ref()
                .unwrap()
                .total_tokens,
            13
        );
        service.set_permission("opencode", false).unwrap();
    }
    let service = Service::with_storage(Found, Offline, &root);
    let snapshot = service.scan_agents();
    assert!(!snapshot.agents[0].monitoring_allowed && snapshot.agents[0].usage.is_none());
    assert!(!snapshot.agents[0].history_cached);
    assert!(!legacy.try_exists().unwrap());
}

#[test]
fn already_migrated_install_discards_stale_copy_without_restoring_its_contents() {
    let root = root("already-migrated");
    let legacy = root.join("monitor-state.json");
    write_legacy(&legacy, &legacy_opencode(13));
    {
        let service = Service::with_storage(Found, Offline, &root);
        service.scan_agents();
        service.set_permission("claude", true).unwrap();
    }
    write_legacy(&legacy, &legacy_opencode(99));

    let service = Service::with_storage(Found, Offline, &root);
    let snapshot = service.scan_agents();
    let opencode = &snapshot.agents[0];
    assert!(opencode.monitoring_allowed && opencode.history_cached);
    assert_eq!(opencode.usage.as_ref().unwrap().total_tokens, 13);
    assert!(!legacy.try_exists().unwrap());
    service.set_permission("opencode", false).unwrap();
    drop(service);
    let service = Service::with_storage(BothFound, Offline, &root);
    let snapshot = service.scan_agents();
    let claude = snapshot.agents.iter().find(|a| a.id == "claude").unwrap();
    assert!(claude.monitoring_allowed);
    let opencode = snapshot.agents.iter().find(|a| a.id == "opencode").unwrap();
    assert!(!opencode.monitoring_allowed && !opencode.history_cached);
}

struct BothFound;
impl Discovery for BothFound {
    fn installed(&self, id: &str) -> bool {
        matches!(id, "claude" | "opencode")
    }
}

#[test]
fn blocked_sqlite_import_keeps_the_original_for_the_next_startup() {
    let root = root("blocked-import");
    let legacy = root.join("monitor-state.json");
    let original = legacy_opencode(13);
    write_legacy(&legacy, &original);
    let database = root.join("monitor-usage.sqlite3");
    let blocker = rusqlite::Connection::open(&database).unwrap();
    private_file(&database);
    blocker
        .execute_batch("PRAGMA journal_mode = WAL; BEGIN IMMEDIATE;")
        .unwrap();

    {
        let service = Service::with_storage(Found, Offline, &root);
        assert_storage_unavailable(&service);
        assert_eq!(std::fs::read(&legacy).unwrap(), original);
    }
    drop(blocker);

    let service = Service::with_storage(Found, Offline, &root);
    let snapshot = service.scan_agents();
    assert!(snapshot.agents[0].monitoring_allowed);
    assert_eq!(snapshot.agents[0].usage.as_ref().unwrap().total_tokens, 13);
    assert!(!legacy.try_exists().unwrap());
}

#[test]
fn unsafe_legacy_entry_surfaces_cleanup_failure_and_is_retried_on_startup() {
    let root = root("blocked-cleanup");
    let legacy = root.join("monitor-state.json");
    write_legacy(&legacy, &legacy_opencode(13));
    {
        let service = Service::with_storage(Found, Offline, &root);
        assert!(service.scan_agents().agents[0].monitoring_allowed);
    }
    // A non-file entry is refused even when tests run with elevated privileges.
    let saved = root.join("stale-copy");
    write_legacy(&legacy, &legacy_opencode(99));
    std::fs::rename(&legacy, &saved).unwrap();
    std::fs::create_dir(&legacy).unwrap();
    std::fs::write(legacy.join("untouched"), b"fixture").unwrap();

    {
        let service = Service::with_storage(Found, Offline, &root);
        assert_storage_unavailable(&service);
        assert_eq!(std::fs::read(legacy.join("untouched")).unwrap(), b"fixture");
    }
    std::fs::rename(&legacy, root.join("blocked-entry")).unwrap();
    std::fs::rename(&saved, &legacy).unwrap();

    let service = Service::with_storage(Found, Offline, &root);
    let snapshot = service.scan_agents();
    assert!(snapshot.agents[0].monitoring_allowed && snapshot.agents[0].history_cached);
    assert_eq!(snapshot.agents[0].usage.as_ref().unwrap().total_tokens, 13);
    assert!(!legacy.try_exists().unwrap());
    assert!(!snapshot.warnings.iter().any(|w| w == STORAGE_ERROR));
}

#[cfg(unix)]
#[test]
fn migrated_store_will_not_clean_up_linked_legacy_entries() {
    use std::os::unix::fs::symlink;

    let outside = root("link-target");
    let target = outside.join("synthetic");
    let original = legacy_opencode(99);
    write_legacy(&target, &original);
    for (name, symbolic) in [("symbolic-link", true), ("hard-link", false)] {
        let root = root(name);
        let legacy = root.join("monitor-state.json");
        {
            let service = Service::with_storage(Found, Offline, &root);
            service.set_permission("opencode", true).unwrap();
        }
        if symbolic {
            symlink(&target, &legacy).unwrap();
        } else {
            std::fs::hard_link(&target, &legacy).unwrap();
        }
        let service = Service::with_storage(Found, Offline, &root);
        assert_storage_unavailable(&service);
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert!(std::fs::symlink_metadata(&legacy).is_ok());
    }
}

#[cfg(unix)]
#[test]
fn first_import_refuses_hard_link_without_committing_sqlite() {
    let root = root("linked-import");
    let target = root.join("synthetic-backup.json");
    let legacy = root.join("monitor-state.json");
    let original = legacy_opencode(13);
    write_legacy(&target, &original);
    std::fs::hard_link(&target, &legacy).unwrap();

    let service = Service::with_storage(Found, Offline, &root);
    assert_storage_unavailable(&service);
    assert_eq!(std::fs::read(&target).unwrap(), original);
    assert_eq!(std::fs::read(&legacy).unwrap(), original);
    let database = rusqlite::Connection::open(root.join("monitor-usage.sqlite3")).unwrap();
    let version: i64 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 0,
        "refused legacy input cannot become authoritative"
    );
}

#[test]
fn sqlite_with_orphaned_history_cannot_authorize_legacy_cleanup() {
    for version in [3, 2] {
        let root = root(&format!("orphaned-history-v{version}"));
        let legacy = root.join("monitor-state.json");
        let original = legacy_opencode(13);
        write_legacy(&legacy, &original);
        let path = root.join("monitor-usage.sqlite3");
        {
            let database = rusqlite::Connection::open(&path).unwrap();
            database
                .execute_batch(include_str!("../src/usage_schema.sql"))
                .unwrap();
            database
                .execute_batch(&format!(
                    "PRAGMA user_version = {version}; PRAGMA foreign_keys = OFF;
                     INSERT INTO usage_tools VALUES ('opencode', 1, NULL, 0, NULL, '[]', NULL);"
                ))
                .unwrap();
            database
                .execute(
                    "INSERT INTO usage_daily VALUES ('gemini', ?1, 14, 5, 3, NULL, NULL, 22, 4)",
                    [chrono::Utc::now().date_naive().to_string()],
                )
                .unwrap();
        }
        private_file(&path);
        let before = std::fs::read(&path).unwrap();

        let service = Service::with_storage(Found, Offline, &root);
        let snapshot = service.scan_agents();
        assert_eq!(
            (
                legacy.try_exists().unwrap(),
                snapshot.warnings.iter().any(|w| w == STORAGE_ERROR),
            ),
            (true, true),
            "invalid SQLite v{version} must not authorize removal"
        );
        assert_storage_unavailable(&service);
        assert_eq!(std::fs::read(&legacy).unwrap(), original);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
fn legacy_daily_counters_with_more_cache_than_input_are_refused() {
    let root = root("invalid-counters");
    let legacy = root.join("monitor-state.json");
    let body = serde_json::to_vec(&serde_json::json!({"version":1,"tools":{
        "gemini":{"allowed":true,"history":[{"date":chrono::Utc::now().date_naive().to_string(),
        "inputTokens":3,"outputTokens":2,"cachedTokens":17,"totalTokens":450}],"models":[],"model_history":[]}
    }})).unwrap();
    write_legacy(&legacy, &body);
    let service = Service::with_storage(Found, Offline, &root);
    assert_storage_unavailable(&service);
    assert_eq!(
        service.set_permission("gemini", true).err().as_deref(),
        Some(STORAGE_ERROR)
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), body);
}

#[test]
fn committed_wal_state_survives_exit_without_closing_the_database() {
    if let Some(path) = std::env::var_os("MONITOR_SQLITE_TEST_CHILD") {
        let service = Service::with_storage(Found, Offline, std::path::Path::new(&path));
        service.scan_agents();
        service.set_permission("opencode", true).unwrap();
        std::process::exit(0);
    }
    let root = root("unclean-exit");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("committed_wal_state_survives_exit_without_closing_the_database")
        .env("MONITOR_SQLITE_TEST_CHILD", &*root)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        root.join("monitor-usage.sqlite3-wal")
            .metadata()
            .unwrap()
            .len()
            > 0
    );
    let service = Service::with_storage(Found, Offline, &root);
    assert!(service.scan_agents().agents[0].monitoring_allowed);
}

#[cfg(unix)]
#[test]
fn refuses_linked_database_and_sidecars_without_touching_the_target() {
    for file in [
        "monitor-usage.sqlite3",
        "monitor-usage.sqlite3-wal",
        "monitor-usage.sqlite3-shm",
    ] {
        let root = root(file);
        let target = root.join("untouched");
        std::fs::write(&target, b"fixture").unwrap();
        std::os::unix::fs::symlink(&target, root.join(file)).unwrap();
        let service = Service::with_storage(Found, Offline, &root);
        assert!(service.set_permission("opencode", true).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"fixture");
    }
}

#[test]
fn upgrades_sqlite_v1_to_v2_and_persists_ccusage_cache_splits() {
    let root = root("v1-to-v2");
    let db_path = root.join("monitor-usage.sqlite3");
    let today = chrono::Utc::now().date_naive().to_string();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE usage_tools (
                tool TEXT PRIMARY KEY,
                allowed INTEGER NOT NULL CHECK (allowed IN (0, 1)),
                period_json TEXT,
                period_verified INTEGER NOT NULL CHECK (period_verified IN (0, 1)),
                partial_json TEXT,
                models_json TEXT NOT NULL,
                updated_at TEXT
            ) STRICT;
            CREATE TABLE usage_daily (
                tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
                date TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cached_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                recorded_responses INTEGER,
                PRIMARY KEY (tool, date)
            ) STRICT;
            CREATE TABLE usage_model_daily (
                tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
                date TEXT NOT NULL,
                model TEXT NOT NULL,
                total_tokens INTEGER NOT NULL,
                input_tokens INTEGER,
                output_tokens INTEGER,
                PRIMARY KEY (tool, date, model)
            ) STRICT;
            CREATE TABLE usage_events (
                tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
                id TEXT NOT NULL,
                timestamp_utc TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cached_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                PRIMARY KEY (tool, id)
            ) STRICT;
            PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_tools VALUES ('opencode', 1, NULL, 0, NULL, '[]', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_daily VALUES ('opencode', ?1, 10, 5, 2, 17, 1)",
            [&today],
        )
        .unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let service = Service::with_storage(Found, Offline, &root);
    let snap = service.scan_agents();
    let opencode = snap.agents.iter().find(|a| a.id == "opencode").unwrap();
    assert!(opencode.monitoring_allowed && opencode.history_cached);
    assert_eq!(opencode.usage_history.len(), 1);
    assert_eq!(opencode.usage_history[0].total_tokens, 17);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
}

#[tokio::test]
async fn v1_to_v3_normalizes_codex_events_so_same_day_high_cache_v2_merge_succeeds() {
    use ai_usage_monitor::{local_usage_observation, Discovery, Observation, StatusReader};
    let root = root("v1-codex-merge");
    let db_path = root.join("monitor-usage.sqlite3");
    let now = chrono::Utc::now();
    let today = now.date_naive().to_string();
    let old_id = "a".repeat(64);
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE usage_tools (
                tool TEXT PRIMARY KEY,
                allowed INTEGER NOT NULL CHECK (allowed IN (0, 1)),
                period_json TEXT,
                period_verified INTEGER NOT NULL CHECK (period_verified IN (0, 1)),
                partial_json TEXT,
                models_json TEXT NOT NULL,
                updated_at TEXT
            ) STRICT;
            CREATE TABLE usage_daily (
                tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
                date TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cached_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                recorded_responses INTEGER,
                PRIMARY KEY (tool, date)
            ) STRICT;
            CREATE TABLE usage_model_daily (
                tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
                date TEXT NOT NULL,
                model TEXT NOT NULL,
                total_tokens INTEGER NOT NULL,
                input_tokens INTEGER,
                output_tokens INTEGER,
                PRIMARY KEY (tool, date, model)
            ) STRICT;
            CREATE TABLE usage_events (
                tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
                id TEXT NOT NULL,
                timestamp_utc TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cached_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                PRIMARY KEY (tool, id)
            ) STRICT;
            PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_tools VALUES ('codex-local', 1, NULL, 0, NULL, '[]', NULL)",
            [],
        )
        .unwrap();
        // Legacy v1 codex event stored input_tokens inclusive of cached_tokens (10 includes 3 cached, total=14).
        conn.execute(
            "INSERT INTO usage_events VALUES ('codex-local', ?1, ?2, 10, 4, 3, 14)",
            rusqlite::params![old_id, now.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_daily VALUES ('codex-local', ?1, 10, 4, 3, 14, 1)",
            [&today],
        )
        .unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let fake_home = root.join("home");
    std::fs::create_dir_all(fake_home.join(".codex/sessions/2026/11/23")).unwrap();
    let rollout = format!(
        "{}\n{}\n",
        serde_json::json!({"timestamp":now.to_rfc3339(),"type":"turn_context","payload":{"model":"gpt-5.4"}}),
        serde_json::json!({"timestamp":now.to_rfc3339(),"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":95,"output_tokens":10,"total_tokens":110},"last_token_usage":{"input_tokens":100,"cached_input_tokens":95,"output_tokens":10,"total_tokens":110}}}})
    );
    std::fs::write(
        fake_home.join(".codex/sessions/2026/11/23/rollout-v2.jsonl"),
        rollout,
    )
    .unwrap();
    struct CodexOnly;
    impl Discovery for CodexOnly {
        fn installed(&self, id: &str) -> bool {
            id == "codex"
        }
    }
    struct CodexReader(PathBuf);
    impl StatusReader for CodexReader {
        fn read(
            &self,
            id: String,
        ) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
            let h = self.0.clone();
            Box::pin(async move { local_usage_observation(&h, &id, chrono::Utc::now()) })
        }
    }
    {
        let service = Service::with_storage(CodexOnly, CodexReader(fake_home.clone()), &root);
        service.scan_agents();
        let snap = service.refresh_usage().await;
        assert!(
            !snap.warnings.iter().any(|w| w.contains("storage")),
            "unexpected storage error: {:?}",
            snap.warnings
        );
        let codex = snap.agents.iter().find(|a| a.id == "codex").unwrap();
        assert!(codex.monitoring_allowed);
        let u = codex.usage.as_ref().unwrap();
        // v1 normalized (7 in, 4 out, 3 cache, 14 tot) + v2 (5 in, 10 out, 95 cache, 110 tot)
        assert_eq!((u.input_tokens, u.output_tokens, u.cached_tokens, u.total_tokens), (12, 14, 98, 124));
    }
    let reopened = Service::with_storage(CodexOnly, Offline, &root);
    let codex = reopened.scan_agents().agents.remove(0);
    let u = codex.usage.unwrap();
    assert_eq!((u.input_tokens, u.output_tokens, u.cached_tokens, u.total_tokens), (12, 14, 98, 124));
}

#[test]
fn v2_to_v3_merges_codex_local_into_codex_when_both_exist() {
    let root = root("v2-to-v3-both");
    let db_path = root.join("monitor-usage.sqlite3");
    let now = chrono::Utc::now();
    let today = now.date_naive().to_string();
    let event_id = "c".repeat(64);
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(include_str!("../src/usage_schema.sql"))
            .unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        // Pre-existing quota-only codex grant + codex-local usage rows
        conn.execute(
            "INSERT INTO usage_tools VALUES ('codex', 1, NULL, 0, NULL, '[]', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_tools VALUES ('codex-local', 1, NULL, 0, NULL, '[]', ?1)",
            [now.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_daily VALUES ('codex-local', ?1, 20, 10, 5, 0, 5, 35, 1)",
            [&today],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_model_daily VALUES ('codex-local', ?1, 'gpt-5.4', 35, 20, 10, 5, 0, 5)",
            [&today],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events VALUES ('codex-local', ?1, ?2, 'gpt-5.4', 20, 10, 5, 0, 5, 35)",
            rusqlite::params![event_id, now.to_rfc3339()],
        )
        .unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    struct CodexFound;
    impl Discovery for CodexFound {
        fn installed(&self, id: &str) -> bool {
            id == "codex"
        }
    }
    let service = Service::with_storage(CodexFound, Offline, &root);
    let snap = service.scan_agents();
    let codex = snap.agents.iter().find(|a| a.id == "codex").unwrap();
    assert!(codex.monitoring_allowed && codex.history_cached);
    assert_eq!(codex.usage_history.len(), 1);
    assert_eq!(codex.usage_history[0].total_tokens, 35);
    assert_eq!(codex.model_history[0].model, "gpt-5.4");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let legacy_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM usage_tools WHERE tool = 'codex-local'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_count, 0);
}

#[test]
fn rejects_invalid_cache_splits_in_v2_events_and_usage_period() {
    let bad_event_root = root("bad-event-cache");
    let now = chrono::Utc::now().to_rfc3339();
    {
        let service = Service::with_storage(Found, Offline, &bad_event_root);
        service.scan_agents();
        service.set_permission("codex", true).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(bad_event_root.join("monitor-usage.sqlite3")).unwrap();
        conn.execute(
            "INSERT INTO usage_events VALUES ('codex', ?1, ?2, 'gpt-5', 10, 5, 3, -1, 4, 18)",
            rusqlite::params!["b".repeat(64), now],
        )
        .unwrap();
    }
    let service = Service::with_storage(Found, Offline, &bad_event_root);
    assert!(service.set_permission("opencode", true).is_err());

    let bad_usage_root = root("bad-usage-cache");
    {
        let service = Service::with_storage(Found, Offline, &bad_usage_root);
        service.scan_agents();
        service.set_permission("opencode", true).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(bad_usage_root.join("monitor-usage.sqlite3")).unwrap();
        let bad_period = serde_json::json!({
            "inputTokens": 10, "outputTokens": 5, "cachedTokens": 3,
            "cacheCreationTokens": 9007199254740992u64, "cacheReadTokens": 0,
            "totalTokens": 18, "spendUsd": null, "source": "Fixture", "scope": "device",
            "asOf": now
        })
        .to_string();
        conn.execute(
            "UPDATE usage_tools SET period_json = ?1 WHERE tool = 'opencode'",
            [bad_period],
        )
        .unwrap();
    }
    let service = Service::with_storage(Found, Offline, &bad_usage_root);
    assert!(service.set_permission("opencode", true).is_err());
}

#[test]
fn v2_to_v3_rolls_back_user_version_when_validation_fails() {
    let root = root("v2-rollback");
    let db_path = root.join("monitor-usage.sqlite3");
    let legacy = root.join("monitor-state.json");
    let original = legacy_opencode(13);
    write_legacy(&legacy, &original);
    let now = chrono::Utc::now();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(include_str!("../src/usage_schema.sql"))
            .unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute(
            "INSERT INTO usage_tools VALUES ('codex-local', 1, NULL, 0, NULL, '[]', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_events VALUES ('codex-local', ?1, ?2, 'gpt-5', 10, 5, 3, -1, 4, 18)",
            rusqlite::params!["e".repeat(64), now.to_rfc3339()],
        )
        .unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let service = Service::with_storage(Found, Offline, &root);
    assert_storage_unavailable(&service);
    assert_eq!(std::fs::read(&legacy).unwrap(), original);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let codex_local: i64 = conn
        .query_row(
            "SELECT count(*) FROM usage_tools WHERE tool = 'codex-local'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(codex_local, 1);
}

#[test]
fn v2_to_v3_handles_overlapping_codex_daily_without_model_and_codex_local_with_model() {
    let root = root("v2-overlap-daily");
    let db_path = root.join("monitor-usage.sqlite3");
    let now = chrono::Utc::now();
    let today = now.date_naive().to_string();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(include_str!("../src/usage_schema.sql"))
            .unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute(
            "INSERT INTO usage_tools VALUES ('codex', 1, NULL, 0, NULL, '[]', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_daily VALUES ('codex', ?1, 10, 4, 0, 0, 0, 14, 1)",
            [&today],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_tools VALUES ('codex-local', 1, NULL, 0, NULL, '[]', ?1)",
            [now.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_daily VALUES ('codex-local', ?1, 20, 10, 5, 0, 5, 35, 1)",
            [&today],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO usage_model_daily VALUES ('codex-local', ?1, 'gpt-5.4', 35, 20, 10, 5, 0, 5)",
            [&today],
        )
        .unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    struct CodexOverlap;
    impl Discovery for CodexOverlap {
        fn installed(&self, id: &str) -> bool {
            id == "codex"
        }
    }
    let service = Service::with_storage(CodexOverlap, Offline, &root);
    let snap = service.scan_agents();
    assert!(!snap
        .warnings
        .iter()
        .any(|w| w.contains("Local history storage unavailable")));
    let codex = snap.agents.iter().find(|a| a.id == "codex").unwrap();
    assert_eq!(codex.usage_history.len(), 1);
    assert_eq!(codex.usage_history[0].total_tokens, 14);
    assert!(codex.model_history.is_empty());
}
