use ai_usage_monitor::usage::{aggregate, read_session_files, TokenRecord};
use chrono::{TimeZone, Utc};
use std::path::PathBuf;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap()
}
fn record(id: &str, day: u32, total: u64) -> TokenRecord {
    TokenRecord {
        id: id.into(),
        timestamp: Utc.with_ymd_and_hms(2026, 11, day, 10, 0, 0).unwrap(),
        model: "fixture-model".into(),
        input_tokens: total - 2,
        output_tokens: 2,
        cached_tokens: 1,
        cache_creation_tokens: 0,
        cache_read_tokens: 1,
        total_tokens: total,
    }
}

#[test]
fn local_history_deduplicates_updates_and_keeps_billing_unknown() {
    let result = aggregate(
        vec![
            record("one", 6, 10),
            record("one", 6, 12),
            record("two", 7, 20),
        ],
        now(),
        "fixture",
    )
    .unwrap();
    assert_eq!(result.usage.total_tokens, 32);
    assert_eq!(result.history.len(), 2);
    assert_eq!(result.history[0].total_tokens, 12);
    assert_eq!(result.model_usage[0].total_tokens, 32);
    assert_eq!(result.usage.scope, "device");
    assert!(result.usage.spend_usd.is_none());
    assert_eq!(
        result.usage.period_start.as_deref(),
        Some("2026-10-09T00:00:00Z")
    );
}

#[test]
fn empty_old_future_and_overflow_records_do_not_create_known_zero_totals() {
    assert!(aggregate(vec![], now(), "fixture").is_err());
    assert!(aggregate(vec![record("future", 8, 10)], now(), "fixture").is_err());
    let mut huge = record("huge", 7, 10);
    huge.total_tokens = u64::MAX;
    assert!(aggregate(vec![huge], now(), "fixture").is_err());
}

fn scratch(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("widget-usage-tests-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn reader_limits_scope_and_prefers_gemini_jsonl_twins() {
    let home = scratch("twins");
    let chats = home.join(".gemini/tmp/project/chats");
    std::fs::create_dir_all(&chats).unwrap();
    std::fs::write(chats.join("session-one.json"), "legacy").unwrap();
    std::fs::write(chats.join("session-one.jsonl"), "current").unwrap();
    std::fs::write(home.join("unrelated.jsonl"), "private").unwrap();
    let files = read_session_files(&home, "gemini").unwrap();
    assert_eq!(files.files.len(), 1);
    assert_eq!(files.files[0].text, "current");
    assert!(read_session_files(&home, "../").is_err());
}

#[test]
fn gemini_session_reader_and_parser_produce_real_history_end_to_end() {
    let home = scratch("gemini-integrated");
    let chats = home.join(".gemini/tmp/fixture-project/chats");
    std::fs::create_dir_all(&chats).unwrap();
    let text = r#"{"sessionId":"fixture-session","projectHash":"fixture-project","messages":[{"id":"m1","type":"gemini","timestamp":"2026-11-07T10:00:00Z","model":"gemini-fixture","tokens":{"input":100,"output":20,"cached":30,"total":130,"thoughts":10}}]}"#;
    std::fs::write(chats.join("session-one.json"), text).unwrap();
    std::fs::write(chats.join("session-copy.json"), text).unwrap();
    let summary = ai_usage_monitor::usage::load_local_usage(&home, "gemini", now()).unwrap();
    assert_eq!(summary.usage.total_tokens, 130);
    assert_eq!(summary.model_usage[0].model, "gemini-fixture");
    assert_eq!(summary.history[0].date, "2026-11-07");
}

#[test]
fn gemini_local_transcripts_include_day_ninety_but_not_day_ninety_one() {
    let home = scratch("gemini-window");
    let chats = home.join(".gemini/tmp/fixture-project/chats");
    std::fs::create_dir_all(&chats).unwrap();
    let text = r#"{"sessionId":"window","projectHash":"fixture-project","messages":[
      {"id":"m1","type":"gemini","timestamp":"2026-08-10T10:00:00Z","model":"fixture","tokens":{"input":100,"output":20,"cached":0,"total":120}},
      {"id":"m2","type":"gemini","timestamp":"2026-08-09T10:00:00Z","model":"fixture","tokens":{"input":900,"output":0,"cached":0,"total":900}}
    ]}"#;
    std::fs::write(chats.join("session-one.json"), text).unwrap();
    let summary = ai_usage_monitor::usage::load_local_usage(&home, "gemini", now()).unwrap();
    assert_eq!(summary.usage.total_tokens, 120);
    assert_eq!(summary.history.len(), 1);
    assert_eq!(summary.history[0].date, "2026-08-10");
    assert_eq!(summary.history[0].recorded_responses, Some(1));
}

#[cfg(unix)]
#[test]
fn reader_refuses_directory_and_file_symlink_escape() {
    use std::os::unix::fs::symlink;
    let home = scratch("symlink");
    let projects = home.join(".claude/projects");
    std::fs::create_dir_all(&projects).unwrap();
    let outside = scratch("outside");
    std::fs::write(outside.join("secret.jsonl"), "do not read").unwrap();
    symlink(&outside, projects.join("escape")).unwrap();
    symlink(outside.join("secret.jsonl"), projects.join("secret.jsonl")).unwrap();
    let files = read_session_files(&home, "claude").unwrap();
    assert!(files.files.is_empty());
    assert!(!files.warnings.is_empty());
}
