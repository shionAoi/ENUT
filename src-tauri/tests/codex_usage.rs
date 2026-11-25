use ai_usage_monitor::usage::load_local_usage;
use ai_usage_monitor::{local_usage_observation, Discovery, Observation, Service, StatusReader};
use chrono::Utc;
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

fn record(response: &str, input: u64) -> Value {
    json!({"timestamp":Utc::now().to_rfc3339(),"type":"token_usage_record","payload":{
        "session_id":"root","thread_id":"thread","turn_id":"turn","root_turn_id":"turn",
        "response_id":response,"usage":{"input_tokens":input,"cached_input_tokens":3,
        "output_tokens":4,"reasoning_output_tokens":2,"total_tokens":input+4},
        "turn_total_token_usage":{"total_tokens":8000},"thread_total_token_usage":{"total_tokens":9000}}})
}

fn fixture() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "codex-rollouts-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir_all(path.join(".codex/sessions/2026/11/21")).unwrap();
    fs::create_dir_all(path.join(".codex/archived_sessions")).unwrap();
    path
}

struct CodexTools;
impl Discovery for CodexTools {
    fn installed(&self, id: &str) -> bool {
        id == "codex"
    }
}
struct Reader(PathBuf);
impl StatusReader for Reader {
    fn read(
        &self,
        id: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
    {
        let root = self.0.clone();
        Box::pin(async move {
            if id == "codex" {
                local_usage_observation(&root, &id, Utc::now())
            } else {
                Ok(Observation::default())
            }
        })
    }
}

#[tokio::test]
async fn codex_import_uses_general_permission_and_keeps_prior_seen_events_across_restart() {
    let home = fixture();
    let db = home.join("monitor");
    let file = home.join(".codex/sessions/2026/11/21/rollout-actual.jsonl");
    let first = record("r1", 10);
    fs::write(&file, format!("{first}\n")).unwrap();
    let observed = |snapshot: ai_usage_monitor::Snapshot| {
        snapshot
            .agents
            .into_iter()
            .find(|a| a.id == "codex")
            .unwrap()
    };
    {
        let service = Service::with_storage(CodexTools, Reader(home.clone()), &db);
        let init = service.scan_agents();
        assert_eq!(init.agents.len(), 1);
        assert!(observed(service.refresh_usage().await).usage.is_none());
        service.set_permission("codex", true).unwrap();
        assert_eq!(
            observed(service.refresh_usage().await)
                .usage
                .unwrap()
                .total_tokens,
            14
        );
    }
    let second = record("r2", 20);
    fs::write(&file, format!("{second}\n")).unwrap();
    {
        let service = Service::with_storage(CodexTools, Reader(home.clone()), &db);
        assert_eq!(
            observed(service.scan_agents()).usage.unwrap().total_tokens,
            14
        );
        assert_eq!(
            observed(service.refresh_usage().await)
                .usage
                .unwrap()
                .total_tokens,
            38
        );
        assert!(
            observed(service.set_permission("codex", false).unwrap())
                .usage
                .is_none()
        );
    }
    let reopened = Service::with_storage(CodexTools, Reader(home), &db);
    assert!(!observed(reopened.scan_agents()).monitoring_allowed);
    let conn = rusqlite::Connection::open(db.join("monitor-usage.sqlite3")).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM usage_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(fs::read_to_string(file).unwrap(), format!("{second}\n"));
}

#[tokio::test]
async fn newer_codex_counters_replace_the_same_response_without_counting_it_twice() {
    let home = fixture();
    let storage = home.join("monitor");
    let file = home.join(".codex/sessions/2026/11/21/rollout-reconciled.jsonl");
    let start = Utc::now() - chrono::Duration::seconds(10);
    let write = |input, seconds| {
        let mut row = record("same-response", input);
        row["timestamp"] = json!((start + chrono::Duration::seconds(seconds)).to_rfc3339());
        fs::write(&file, format!("{row}\n")).unwrap();
    };
    for (input, seconds, total) in [(7, 1, 11), (15, 2, 19), (7, 1, 19)] {
        write(input, seconds);
        let service = Service::with_storage(CodexTools, Reader(home.clone()), &storage);
        service.scan_agents();
        service.set_permission("codex", true).unwrap();
        let snapshot = service.refresh_usage().await;
        let codex = snapshot
            .agents
            .iter()
            .find(|a| a.id == "codex")
            .unwrap();
        assert_eq!(codex.usage.as_ref().unwrap().total_tokens, total);
        assert_eq!(codex.usage_history[0].recorded_responses, Some(1));
    }
    let old = fs::read_to_string(&file).unwrap();
    write(20, 3);
    let new = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{old}{new}")).unwrap();
    let summary = load_local_usage(&home, "codex", Utc::now()).unwrap();
    assert_eq!(summary.usage.total_tokens, 24);
    assert_eq!(summary.history[0].recorded_responses, Some(1));
}

#[test]
fn direct_codex_records_count_by_response_not_rollup_or_archive_copy() {
    let home = fixture();
    let session = home.join(".codex/sessions/2026/11/21/rollout-one.jsonl");
    let first = record("r1", 10);
    let second = record("r2", 20);
    let legacy = json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":20000}}}});
    fs::write(
        session,
        format!("{first}\n{second}\n{legacy}\n{}", record("unfinished", 40)),
    )
    .unwrap();
    fs::write(
        home.join(".codex/archived_sessions/rollout-copy.jsonl"),
        format!("{first}\n"),
    )
    .unwrap();
    fs::write(
        home.join(".codex/session-secret.jsonl"),
        format!("{}\n", record("ignored", 90)),
    )
    .unwrap();
    let result = load_local_usage(&home, "codex", Utc::now()).unwrap();
    assert_eq!(
        (
            result.usage.input_tokens,
            result.usage.output_tokens,
            result.usage.cached_tokens,
            result.usage.total_tokens
        ),
        (24, 8, 6, 38)
    );
    assert_eq!(result.history[0].recorded_responses, Some(2));
    assert_eq!(result.model_usage[0].model, "Unreported model");
}

#[test]
fn token_count_events_extract_delta_once_and_empty_snapshots_stay_cumulative_only() {
    let home = fixture();
    let file = home.join(".codex/sessions/2026/11/21/rollout-snapshots.jsonl");
    let counters = json!({"input_tokens":20,"cached_input_tokens":4,"output_tokens":5,
        "reasoning_output_tokens":2,"total_tokens":25});
    let snapshot = json!({"timestamp":Utc::now(),"type":"event_msg","payload":{"type":"token_count",
        "info":{"total_token_usage":counters,"last_token_usage":counters,"model_context_window":272000},
        "rate_limits":null}});
    fs::write(&file, format!("{snapshot}\n{snapshot}\n")).unwrap();
    let summary = load_local_usage(&home, "codex", Utc::now()).unwrap();
    assert_eq!(
        (
            summary.usage.input_tokens,
            summary.usage.output_tokens,
            summary.usage.cached_tokens,
            summary.usage.total_tokens
        ),
        (16, 5, 4, 25)
    );
    assert_eq!(summary.history[0].recorded_responses, Some(1));
    assert_eq!(summary.model_usage[0].model, "gpt-5");

    // Cumulative-only snapshot without component token counts still reports unavailable
    let total_only = json!({"timestamp":Utc::now(),"type":"event_msg","payload":{"type":"token_count",
        "info":{"total_token_usage":{"total_tokens":20000}}}});
    fs::write(&file, format!("{total_only}\n")).unwrap();
    let error = load_local_usage(&home, "codex", Utc::now())
        .err()
        .unwrap();
    assert!(
        error.contains("Codex reported cumulative snapshots but no eligible per-response counters")
    );
    assert!(error.contains("Daily usage cannot be calculated"));

    fs::write(&file, "{}\n").unwrap();
    let error = load_local_usage(&home, "codex", Utc::now())
        .err()
        .unwrap();
    assert!(error.contains("No supported Codex per-response counters"));
    assert!(!error.contains("reported cumulative snapshots"));
}

#[cfg(unix)]
#[tokio::test]
async fn codex_snapshot_cannot_create_history_or_replace_saved_direct_records() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let home = fixture();
    let db = home.join("monitor");
    fs::create_dir(&db).unwrap();
    let legacy = db.join("monitor-state.json");
    fs::write(&legacy, br#"{"version":1,"tools":{"codex":{"allowed":false,"history":[],"models":[],"model_history":[]}}}"#).unwrap();
    fs::set_permissions(&legacy, fs::Permissions::from_mode(0o600)).unwrap();
    let hash = |p: &std::path::Path| Sha256::digest(fs::read(p).unwrap());
    let live = home.join(".codex/sessions/2026/11/21/rollout-readonly.jsonl");
    let archive = home.join(".codex/archived_sessions/rollout-readonly-copy.jsonl");
    let write = |p: &std::path::Path, text: &str| {
        if p.exists() {
            fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(p, text).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o400)).unwrap();
    };
    let now = Utc::now();
    let snap = |days| {
        json!({"timestamp":now - chrono::Duration::days(days),
      "type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":8000},"last_token_usage":{"total_tokens":600}}}})
    };
    let snapshots = format!("{}\n{}\n", snap(0), snap(2));
    write(&live, &snapshots);
    let open = || Service::with_storage(CodexTools, Reader(home.clone()), &db);
    let get = |s: ai_usage_monitor::Snapshot, id: &str| {
        s.agents.into_iter().find(|a| a.id == id).unwrap()
    };
    let database = db.join("monitor-usage.sqlite3");
    let wal = db.join("monitor-usage.sqlite3-wal");
    let store_hash = || (hash(&database), wal.exists().then(|| hash(&wal)));
    {
        let service = open();
        let init = service.scan_agents();
        assert!(!get(init, "codex").monitoring_allowed);
        let denied = get(service.refresh_usage().await, "codex");
        assert!(denied.usage.is_none());
        assert_eq!(denied.status, "detected");
        service.set_permission("codex", true).unwrap();
        let before = (store_hash(), hash(&live));
        let state = service.refresh_usage().await;
        let codex = get(state, "codex");
        assert_eq!(codex.message, "Codex reported cumulative snapshots but no eligible per-response counters. Daily usage cannot be calculated from these snapshots.");
        assert!(
            codex.usage.is_none()
                && codex.usage_history.is_empty()
                && codex.model_history.is_empty()
        );
        assert_eq!((store_hash(), hash(&live)), before);
    }
    let direct = |id, input, days| {
        let mut row = record(id, input);
        row["timestamp"] = json!(now - chrono::Duration::days(days));
        row
    };
    let eligible = direct("retained-edge", 17, 89);
    write(
        &live,
        &format!(
            "{snapshots}{eligible}\n{}\n",
            direct("expired-edge", 99, 90)
        ),
    );
    write(&archive, &format!("{eligible}\n"));
    let original = (hash(&live), hash(&archive));
    {
        let service = open();
        service.scan_agents();
        let codex = get(service.refresh_usage().await, "codex");
        assert_eq!(codex.usage.unwrap().total_tokens, 21);
        assert_eq!(codex.usage_history.len(), 1);
        assert_eq!(codex.usage_history[0].recorded_responses, Some(1));
        assert_eq!((hash(&live), hash(&archive)), original);
    }
    write(&live, &snapshots);
    write(&archive, &snapshots);
    let service = open();
    let state = service.scan_agents();
    assert!(get(state.clone(), "codex").monitoring_allowed);
    assert_eq!(get(state, "codex").usage.unwrap().total_tokens, 21);
    let before = store_hash();
    let codex = get(service.refresh_usage().await, "codex");
    assert!(codex.history_cached && !codex.connected);
    assert_eq!(codex.usage.unwrap().total_tokens, 21);
    assert_eq!(codex.usage_history.len(), 1);
    assert_eq!(store_hash(), before);
    assert!(!legacy.try_exists().unwrap());
}

#[test]
fn codex_turn_context_model_attribution_saturating_sub_and_auto_review_reroute() {
    let lines = [
        json!({"timestamp":"2026-04-01T10:00:00Z","type":"turn_context","payload":{"model":"o3-pro"}}),
        // First turn uses saturating_sub from total_token_usage (no last_token_usage)
        json!({"timestamp":"2026-04-01T10:00:05Z","type":"event_msg","payload":{"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20,"total_tokens":120}}}}),
        // Duplicate unchanged total_token_usage snapshot must be ignored
        json!({"timestamp":"2026-04-01T10:00:06Z","type":"event_msg","payload":{"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20,"total_tokens":120},
                    "last_token_usage":{"input_tokens":100,"cached_input_tokens":40,"output_tokens":20,"total_tokens":120}}}}),
        // Second turn switches to codex-auto-review (routed to gpt-5.4 on 2026-04-01) and advances totals without last_token_usage
        json!({"timestamp":"2026-04-01T10:01:00Z","type":"turn_context","payload":{"model":"codex-auto-review"}}),
        json!({"timestamp":"2026-04-01T10:01:05Z","type":"event_msg","payload":{"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":60,"output_tokens":35,"total_tokens":185}}}}),
    ];
    let text = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let file = ai_usage_monitor::usage::SessionFile {
        hint: String::new(),
        path_hint: "root0/sessions/rollout-1.jsonl".into(),
        text,
    };
    let (parsed, cumulative_only) = ai_usage_monitor::codex_usage::parse_files(&[file]);
    assert!(!cumulative_only);
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 2);
    for r in &parsed.records {
        assert_eq!(r.id.len(), 64);
        assert!(r.id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }
    let first = parsed.records.iter().find(|r| r.model == "o3-pro").unwrap();
    assert_eq!(
        (first.input_tokens, first.output_tokens, first.cached_tokens, first.total_tokens),
        (60, 20, 40, 120)
    );
    let second = parsed.records.iter().find(|r| r.model == "gpt-5.4").unwrap();
    assert_eq!(
        (second.input_tokens, second.output_tokens, second.cached_tokens, second.total_tokens),
        (30, 15, 20, 65)
    );
}

#[test]
fn codex_token_usage_record_compaction_and_non_double_counting_with_token_count() {
    let lines = [
        json!({"timestamp":"2026-04-01T10:00:00Z","type":"turn_context","payload":{"model":"gpt-5.4"}}),
        // Turn 1: token_usage_record followed by matching advancing token_count -> count ONCE
        json!({"timestamp":"2026-04-01T10:00:01Z","type":"token_usage_record","payload":{
            "session_id":"s1","thread_id":"t1","turn_id":"turn1","response_id":"resp_local",
            "usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130},
            "thread_total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130}}}),
        json!({"timestamp":"2026-04-01T10:00:02Z","type":"event_msg","payload":{"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130},
                    "last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":30,"total_tokens":130}}}}),
        json!({"timestamp":"2026-04-01T10:00:03Z","type":"compacted","payload":{"compaction_response_id":"resp_local"}}),
        // Turn 2: remote v2 compaction (token_usage_record + compacted, NO token_count) -> MUST be counted ONCE
        json!({"timestamp":"2026-04-01T10:01:01Z","type":"token_usage_record","payload":{
            "session_id":"s1","thread_id":"t1","turn_id":"turn2","response_id":"resp_remote",
            "usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":15,"total_tokens":65}}}),
        json!({"timestamp":"2026-04-01T10:01:02Z","type":"compacted","payload":{"compaction_response_id":"resp_remote"}}),
    ];
    let text = lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let file = ai_usage_monitor::usage::SessionFile {
        hint: String::new(),
        path_hint: "root0/sessions/rollout-compact.jsonl".into(),
        text,
    };
    let (parsed, cumulative_only) = ai_usage_monitor::codex_usage::parse_files(&[file]);
    assert!(!cumulative_only);
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 2);
    let total_sum: u64 = parsed.records.iter().map(|r| r.total_tokens).sum();
    assert_eq!(total_sum, 195);
}

#[test]
fn codex_headless_exec_json_and_fork_replay_plus_archive_dedupe() {
    let parent_lines = [
        json!({"timestamp":"2026-04-01T10:00:00Z","type":"session_meta","payload":{"id":"sess_parent","model":"gpt-5.3-codex"}}),
        json!({"timestamp":"2026-04-01T10:00:05Z","type":"event_msg","payload":{"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"total_tokens":110},
                    "last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"total_tokens":110}}}}),
    ];
    let parent_text = parent_lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    // Child forks from sess_parent, replays parent's turn, then adds its own turn
    let fork_lines = [
        json!({"timestamp":"2026-04-01T10:02:00Z","type":"session_meta","payload":{"id":"sess_child","forked_from_id":"sess_parent","model":"gpt-5.3-codex"}}),
        json!({"timestamp":"2026-04-01T10:02:01Z","type":"event_msg","payload":{"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"total_tokens":110},
                    "last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"total_tokens":110}}}}),
        json!({"timestamp":"2026-04-01T10:03:00Z","type":"event_msg","payload":{"type":"token_count",
            "info":{"total_token_usage":{"input_tokens":160,"cached_input_tokens":50,"output_tokens":25,"total_tokens":185},
                    "last_token_usage":{"input_tokens":60,"cached_input_tokens":30,"output_tokens":15,"total_tokens":75}}}}),
    ];
    let fork_text = fork_lines
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let headless_text = format!(
        "{}\n",
        json!({
            "type": "turn.completed",
            "timestamp": "2026-04-01T10:05:00Z",
            "model": "gpt-5.4",
            "usage": {"input_tokens": 40, "cached_input_tokens": 10, "output_tokens": 5}
        })
    );
    let files = vec![
        ai_usage_monitor::usage::SessionFile {
            hint: String::new(),
            path_hint: "root0/sessions/parent.jsonl".into(),
            text: parent_text.clone(),
        },
        ai_usage_monitor::usage::SessionFile {
            hint: String::new(),
            path_hint: "root0/archived_sessions/parent.jsonl".into(),
            text: parent_text,
        },
        ai_usage_monitor::usage::SessionFile {
            hint: String::new(),
            path_hint: "root0/sessions/fork.jsonl".into(),
            text: fork_text,
        },
        ai_usage_monitor::usage::SessionFile {
            hint: String::new(),
            path_hint: "root0/sessions/headless.jsonl".into(),
            text: headless_text,
        },
    ];
    let (parsed, cumulative_only) = ai_usage_monitor::codex_usage::parse_files(&files);
    assert!(!cumulative_only);
    assert!(parsed.warnings.is_empty());
    // 1 parent turn (110, archived copy collapsed) + 1 fork-only turn (75, replayed prefix stripped) + 1 headless turn (45) = 3 records, total 230
    assert_eq!(parsed.records.len(), 3);
    let total_sum: u64 = parsed.records.iter().map(|r| r.total_tokens).sum();
    assert_eq!(total_sum, 230);
}
