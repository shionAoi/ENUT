use ai_usage_monitor::antigravity_usage;
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
            "eztyn-antigravity-test-{}-{}-{label}",
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

fn ts_secs(year: i32, month: u32, day: u32, hour: u32, min: u32) -> u64 {
    Utc.with_ymd_and_hms(year, month, day, hour, min, 0)
        .unwrap()
        .timestamp() as u64
}

// --- Hand-encoded Protobuf fixture builders ---

#[derive(Clone, Copy, Default)]
struct UsageFixture {
    model_id: Option<u64>,
    input_tokens: u64,
    total_output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    visible_output_tokens: u64,
    provider: Option<u64>,
    message_id: Option<&'static str>,
    response_id: Option<&'static str>,
    provider_assigned_message_id: Option<&'static str>,
}

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(((value as u8) & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn field_varint(number: u64, value: u64, out: &mut Vec<u8>) {
    encode_varint(number << 3, out);
    encode_varint(value, out);
}

fn field_bytes(number: u64, value: &[u8], out: &mut Vec<u8>) {
    encode_varint((number << 3) | 2, out);
    encode_varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

fn model_usage_blob(u: UsageFixture) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(id) = u.model_id {
        field_varint(1, id, &mut out);
    }
    for (num, val) in [
        (2, u.input_tokens),
        (3, u.total_output_tokens),
        (4, u.cache_creation_tokens),
        (5, u.cache_read_tokens),
        (9, u.reasoning_tokens),
        (10, u.visible_output_tokens),
    ] {
        if val > 0 {
            field_varint(num, val, &mut out);
        }
    }
    if let Some(p) = u.provider {
        field_varint(6, p, &mut out);
    }
    if let Some(m) = u.message_id {
        field_bytes(7, m.as_bytes(), &mut out);
    }
    if let Some(r) = u.response_id {
        field_bytes(11, r.as_bytes(), &mut out);
    }
    if let Some(pm) = u.provider_assigned_message_id {
        field_bytes(12, pm.as_bytes(), &mut out);
    }
    out
}

fn retry_info_blob(u: UsageFixture) -> Vec<u8> {
    let inner = model_usage_blob(u);
    let mut out = Vec::new();
    field_bytes(2, &inner, &mut out);
    out
}

fn timestamp_blob(seconds: u64, nanos: u64) -> Vec<u8> {
    let mut out = Vec::new();
    field_varint(1, seconds, &mut out);
    if nanos > 0 {
        field_varint(2, nanos, &mut out);
    }
    out
}

fn gen_metadata_blob(
    model: Option<&str>,
    chat_model_id: Option<u64>,
    usage: Option<UsageFixture>,
    timestamp: Option<(u64, u64)>,
    retries: &[UsageFixture],
    step_indices: &[i64],
) -> Vec<u8> {
    let mut chat_model = Vec::new();
    if let Some(id) = chat_model_id {
        field_varint(3, id, &mut chat_model);
    }
    if let Some(u) = usage {
        let blob = model_usage_blob(u);
        field_bytes(4, &blob, &mut chat_model);
    }
    if let Some((secs, nanos)) = timestamp {
        let ts = timestamp_blob(secs, nanos);
        let mut gen_info = Vec::new();
        field_bytes(4, &ts, &mut gen_info);
        field_bytes(9, &gen_info, &mut chat_model);
    }
    for retry in retries {
        let r = retry_info_blob(*retry);
        field_bytes(17, &r, &mut chat_model);
    }
    if let Some(m) = model {
        field_bytes(19, m.as_bytes(), &mut chat_model);
    }

    let mut root = Vec::new();
    field_bytes(1, &chat_model, &mut root);
    for &idx in step_indices {
        field_varint(2, idx as u64, &mut root);
    }
    root
}

fn step_metadata_blob(
    model: Option<&str>,
    model_id: Option<u64>,
    usage: Option<UsageFixture>,
    timestamp: Option<(u64, u64)>,
    retries: &[UsageFixture],
    provider: Option<u64>,
) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some((secs, nanos)) = timestamp {
        let ts = timestamp_blob(secs, nanos);
        field_bytes(8, &ts, &mut out);
    }
    if let Some(u) = usage {
        let blob = model_usage_blob(u);
        field_bytes(9, &blob, &mut out);
    }
    if model.is_some() || model_id.is_some() || provider.is_some() {
        let mut info = Vec::new();
        if let Some(id) = model_id {
            field_varint(1, id, &mut info);
        }
        if let Some(p) = provider {
            field_varint(7, p, &mut info);
        }
        if let Some(m) = model {
            field_bytes(12, m.as_bytes(), &mut info);
        }
        field_bytes(24, &info, &mut out);
    }
    for retry in retries {
        let r = retry_info_blob(*retry);
        field_bytes(28, &r, &mut out);
    }
    out
}

fn trajectory_blob(seconds: u64, nanos: u64) -> Vec<u8> {
    let ts = timestamp_blob(seconds, nanos);
    let mut out = Vec::new();
    field_bytes(2, &ts, &mut out);
    out
}

fn create_conversation_db(
    path: &Path,
    gen_rows: &[(i64, Vec<u8>)],
    step_rows: &[(i64, Vec<u8>)],
    traj_rows: &[Vec<u8>],
) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);
         CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
         CREATE TABLE trajectory_metadata_blob (data BLOB);",
    )
    .unwrap();
    for (idx, data) in gen_rows {
        conn.execute(
            "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
            rusqlite::params![idx, data],
        )
        .unwrap();
    }
    for (idx, data) in step_rows {
        conn.execute(
            "INSERT INTO steps (idx, metadata) VALUES (?1, ?2)",
            rusqlite::params![idx, data],
        )
        .unwrap();
    }
    for data in traj_rows {
        conn.execute(
            "INSERT INTO trajectory_metadata_blob (data) VALUES (?1)",
            rusqlite::params![data],
        )
        .unwrap();
    }
}

#[test]
fn resolves_placeholders_and_deduplicates_retry_mirror_vs_genuine_retry() {
    let dir = TestDir::new("retry-and-models");
    let db_path = dir
        .path()
        .join(".gemini/antigravity/conversations/session-1.db");
    let t1 = ts_secs(2026, 11, 18, 10, 0);
    let t2 = ts_secs(2026, 11, 19, 11, 0);

    // Primary usage with placeholder model_placeholder_m26 -> claude-opus-4-6
    // Plus a mirror retry (identical counts + subset identities -> MUST BE DROPPED)
    // Plus a genuine retry (different counts -> MUST BE KEPT)
    let primary = UsageFixture {
        input_tokens: 1000,
        total_output_tokens: 0, // tests max(total_output, visible + reasoning)
        visible_output_tokens: 200,
        reasoning_tokens: 50,
        cache_creation_tokens: 100,
        cache_read_tokens: 300,
        response_id: Some("resp-1"),
        message_id: Some("msg-1"),
        ..Default::default()
    };
    let mirror = UsageFixture {
        input_tokens: 1000,
        total_output_tokens: 0,
        visible_output_tokens: 200,
        reasoning_tokens: 50,
        cache_creation_tokens: 100,
        cache_read_tokens: 300,
        response_id: Some("resp-1"),
        ..Default::default()
    };
    let genuine_retry = UsageFixture {
        input_tokens: 400,
        total_output_tokens: 60,
        response_id: Some("resp-1-retry"),
        ..Default::default()
    };

    // Row 2: undated gen_metadata with model_id 246 -> gemini-2.5-pro, falls back to trajectory_metadata_blob
    let id_usage = UsageFixture {
        model_id: Some(246),
        input_tokens: 500,
        total_output_tokens: 120,
        response_id: Some("resp-2"),
        ..Default::default()
    };

    create_conversation_db(
        &db_path,
        &[
            (
                0,
                gen_metadata_blob(
                    Some("model_placeholder_m26"),
                    None,
                    Some(primary),
                    Some((t1, 0)),
                    &[mirror, genuine_retry],
                    &[],
                ),
            ),
            (
                1,
                gen_metadata_blob(None, Some(246), Some(id_usage), None, &[], &[]),
            ),
        ],
        &[],
        &[trajectory_blob(t2, 0)],
    );

    // Non-conversations summary DB outside conversations/ must be ignored for default roots.
    let ignored_summary = dir
        .path()
        .join(".gemini/antigravity/conversation_summaries.db");
    create_conversation_db(
        &ignored_summary,
        &[(
            0,
            gen_metadata_blob(
                Some("gemini-2.5-pro"),
                None,
                Some(UsageFixture {
                    input_tokens: 99999,
                    total_output_tokens: 1,
                    response_id: Some("should-ignore"),
                    ..Default::default()
                }),
                Some((t1, 0)),
                &[],
                &[],
            ),
        )],
        &[],
        &[],
    );

    let summary = antigravity_usage::load(dir.path(), now()).unwrap();
    assert_eq!(
        summary.usage.source,
        "Antigravity local conversations (ccusage method)"
    );
    // primary: in=1000, out=250, cc=100, cr=300, tot=1650 (claude-opus-4-6)
    // genuine_retry: in=400, out=60, tot=460 (claude-opus-4-6)
    // id_usage: in=500, out=120, tot=620 (gemini-2.5-pro)
    assert_eq!(summary.usage.input_tokens, 1900);
    assert_eq!(summary.usage.output_tokens, 430);
    assert_eq!(summary.usage.cache_creation_tokens, Some(100));
    assert_eq!(summary.usage.cache_read_tokens, Some(300));
    assert_eq!(summary.usage.cached_tokens, 400);
    assert_eq!(summary.usage.total_tokens, 2730);

    let models: std::collections::BTreeMap<_, _> = summary
        .model_usage
        .iter()
        .map(|m| (m.model.as_str(), m.total_tokens))
        .collect();
    assert_eq!(models.get("claude-opus-4-6"), Some(&2110));
    assert_eq!(models.get("gemini-2.5-pro"), Some(&620));
}

#[test]
fn transitive_multi_key_dedupe_merges_steps_gen_metadata_and_multiple_dbs() {
    let dir = TestDir::new("transitive-dedupe");
    let db_a = dir
        .path()
        .join(".gemini/antigravity/conversations/chat.db");
    let db_b = dir
        .path()
        .join(".gemini/antigravity-backup/conversations/chat-backup.db");

    let t_early = ts_secs(2026, 11, 18, 9, 0);
    let t_late = ts_secs(2026, 11, 19, 14, 0);

    // DB A:
    // - step 0 has response:r1 with placeholder model & lower input, later timestamp
    // - gen 0 has provider:p1 with unresolved model & higher output, earlier timestamp
    create_conversation_db(
        &db_a,
        &[(
            0,
            gen_metadata_blob(
                Some("model_placeholder_m999"),
                None,
                Some(UsageFixture {
                    input_tokens: 100,
                    total_output_tokens: 80,
                    provider_assigned_message_id: Some("p1"),
                    ..Default::default()
                }),
                Some((t_early, 0)),
                &[],
                &[],
            ),
        )],
        &[(
            0,
            step_metadata_blob(
                None,
                None,
                Some(UsageFixture {
                    input_tokens: 150,
                    total_output_tokens: 40,
                    response_id: Some("r1"),
                    ..Default::default()
                }),
                Some((t_late, 0)),
                &[],
                None,
            ),
        )],
        &[],
    );

    // DB B bridges response:r1 AND provider:p1, supplying resolved model (ID 246 -> gemini-2.5-pro)
    // and cache_read_tokens = 200
    create_conversation_db(
        &db_b,
        &[(
            0,
            gen_metadata_blob(
                None,
                Some(246),
                Some(UsageFixture {
                    model_id: Some(246),
                    input_tokens: 120,
                    total_output_tokens: 60,
                    cache_read_tokens: 200,
                    response_id: Some("r1"),
                    provider_assigned_message_id: Some("p1"),
                    ..Default::default()
                }),
                Some((t_late, 0)),
                &[],
                &[],
            ),
        )],
        &[],
        &[],
    );

    let summary = antigravity_usage::load(dir.path(), now()).unwrap();
    // All 3 events must collapse into 1 merged event:
    // input = max(100, 150, 120) = 150
    // output = max(80, 40, 60) = 80
    // cache_read = max(0, 0, 200) = 200
    // total = 430
    // model = gemini-2.5-pro (Resolved > Placeholder > Default)
    // date = 2026-11-18 (earliest timestamp at same rank 3)
    assert_eq!(summary.usage.input_tokens, 150);
    assert_eq!(summary.usage.output_tokens, 80);
    assert_eq!(summary.usage.cache_read_tokens, Some(200));
    assert_eq!(summary.usage.total_tokens, 430);
    assert_eq!(summary.history.len(), 1);
    assert_eq!(summary.history[0].date, "2026-11-18");
    assert_eq!(summary.history[0].recorded_responses, Some(1));
    assert_eq!(summary.model_usage.len(), 1);
    assert_eq!(summary.model_usage[0].model, "gemini-2.5-pro");
}

#[test]
fn load_from_roots_accepts_direct_conversations_dir_and_dedupes_canonical_paths() {
    let dir = TestDir::new("direct-roots");
    let conv_dir = dir.path().join("custom/conversations");
    let db = conv_dir.join("s1.db");
    let t = ts_secs(2026, 11, 19, 10, 0);
    create_conversation_db(
        &db,
        &[(
            0,
            gen_metadata_blob(
                Some("Gemini 3.6 Flash (High)"),
                None,
                Some(UsageFixture {
                    input_tokens: 300,
                    total_output_tokens: 50,
                    ..Default::default()
                }),
                Some((t, 0)),
                &[],
                &[],
            ),
        )],
        &[],
        &[],
    );

    // Pass both parent and direct conversations dir -> canonical dedupe reads the DB once.
    let summary = antigravity_usage::load_from_roots(
        &[dir.path().join("custom"), conv_dir],
        now(),
        &|| false,
    )
    .unwrap();
    assert_eq!(summary.usage.total_tokens, 350);
    assert_eq!(summary.model_usage[0].model, "gemini-3.6-flash-high");
}

#[cfg(unix)]
#[test]
fn refuses_symlinks_for_conversation_dbs_and_directories() {
    use std::os::unix::fs::symlink;

    let home = TestDir::new("symlink-home");
    let outside = TestDir::new("symlink-outside");
    let t = ts_secs(2026, 11, 19, 10, 0);

    let outside_db = outside.path().join("secret.db");
    create_conversation_db(
        &outside_db,
        &[(
            0,
            gen_metadata_blob(
                Some("gemini-2.5-pro"),
                None,
                Some(UsageFixture {
                    input_tokens: 500,
                    total_output_tokens: 100,
                    response_id: Some("secret"),
                    ..Default::default()
                }),
                Some((t, 0)),
                &[],
                &[],
            ),
        )],
        &[],
        &[],
    );

    let conv = home.path().join(".gemini/antigravity/conversations");
    std::fs::create_dir_all(&conv).unwrap();
    symlink(&outside_db, conv.join("linked.db")).unwrap();

    let err = antigravity_usage::load(home.path(), now()).err().unwrap();
    assert!(!err.contains("secret.db"));
    assert!(!err.contains(&outside.path().to_string_lossy().to_string()));

    // Symlinked conversations directory
    let home2 = TestDir::new("symlink-dir");
    std::fs::create_dir_all(home2.path().join(".gemini/antigravity")).unwrap();
    symlink(
        outside.path(),
        home2.path().join(".gemini/antigravity/conversations"),
    )
    .unwrap();
    assert!(antigravity_usage::load(home2.path(), now()).is_err());
}

#[test]
fn links_undated_generation_via_step_indices_and_resolves_numeric_placeholder_id() {
    let dir = TestDir::new("step-link");
    let db = dir
        .path()
        .join(".gemini/antigravity-cli/conversations/linked.db");
    let t = ts_secs(2026, 11, 17, 8, 30);

    // Step 4 has timestamp + model_id 1026 (1000 + 26 -> model_placeholder_m26 -> claude-opus-4-6)
    // Gen 0 is undated and has NO response_id, linked to step 4 via step_indices=[4], with higher tokens
    create_conversation_db(
        &db,
        &[(
            0,
            gen_metadata_blob(
                None,
                Some(1026),
                Some(UsageFixture {
                    model_id: Some(1026),
                    input_tokens: 700,
                    total_output_tokens: 150,
                    ..Default::default()
                }),
                None,
                &[],
                &[4],
            ),
        )],
        &[(
            4,
            step_metadata_blob(
                None,
                None,
                Some(UsageFixture {
                    input_tokens: 500,
                    total_output_tokens: 100,
                    ..Default::default()
                }),
                Some((t, 0)),
                &[],
                None,
            ),
        )],
        &[],
    );

    let summary = antigravity_usage::load(dir.path(), now()).unwrap();
    assert_eq!(summary.usage.input_tokens, 700);
    assert_eq!(summary.usage.output_tokens, 150);
    assert_eq!(summary.usage.total_tokens, 850);
    assert_eq!(summary.history.len(), 1);
    assert_eq!(summary.history[0].date, "2026-11-17");
    assert_eq!(summary.history[0].recorded_responses, Some(1));
    assert_eq!(summary.model_usage[0].model, "claude-opus-4-6");
}

#[test]
fn load_from_roots_honors_cancellation() {
    let dir = TestDir::new("cancel");
    let root = dir.path().join("custom");
    std::fs::create_dir_all(&root).unwrap();
    let err = antigravity_usage::load_from_roots(&[root], now(), &|| true)
        .err()
        .unwrap();
    assert!(err.to_ascii_lowercase().contains("cancel"));
}
