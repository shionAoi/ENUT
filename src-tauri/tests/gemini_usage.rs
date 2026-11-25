use ai_usage_monitor::usage;
#[path = "../src/gemini_usage.rs"]
mod adapter;
use serde_json::{json, Value};

fn message(id: &str) -> Value {
    json!({"id":id,"type":"gemini","timestamp":"2026-11-07T12:00:00Z",
        "model":"gemini-2.5-pro","content":"PRIVATE synthetic prompt",
        "tokens":{"input":100,"output":20,"cached":30,"thoughts":7,"tool":3,"total":130}})
}
fn legacy(messages: Vec<Value>) -> String {
    json!({"sessionId":"synthetic-session","projectHash":"synthetic-project",
        "startTime":"2026-11-07T11:00:00Z","messages":messages})
    .to_string()
}
fn log(records: Vec<Value>) -> String {
    records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn legacy_preserves_reported_categories_and_total() {
    let parsed = adapter::parse(&legacy(vec![message("m1")]), "session-a").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 1);
    let r = &parsed.records[0];
    assert_eq!(r.id, "session-a:m1");
    assert_eq!(r.timestamp.to_rfc3339(), "2026-11-07T12:00:00+00:00");
    assert_eq!(r.model, "gemini-2.5-pro");
    assert_eq!(
        (
            r.input_tokens,
            r.output_tokens,
            r.cached_tokens,
            r.cache_creation_tokens,
            r.cache_read_tokens,
            r.total_tokens
        ),
        (73, 20, 30, 0, 30, 130)
    );
}

#[test]
fn jsonl_replaces_updates_without_summing_and_keeps_first_insertion_order() {
    let mut updated = message("m1");
    updated["tokens"]["total"] = json!(150);
    let parsed = adapter::parse(
        &log(vec![message("m1"), message("m2"), updated]),
        "session-a",
    )
    .unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(
        parsed
            .records
            .iter()
            .map(|r| (r.id.as_str(), r.total_tokens))
            .collect::<Vec<_>>(),
        vec![("session-a:m1", 150), ("session-a:m2", 130)]
    );
}

#[test]
fn replay_rewind_is_inclusive_and_unknown_target_clears_history() {
    for (target, expected) in [("m2", vec!["session-a:m1"]), ("missing", vec![])] {
        let parsed = adapter::parse(
            &log(vec![
                message("m1"),
                message("m2"),
                message("m3"),
                json!({"$rewindTo":target}),
            ]),
            "session-a",
        )
        .unwrap();
        assert!(parsed.warnings.is_empty());
        assert_eq!(
            parsed
                .records
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn replay_patch_removal_order_and_content_only_updates() {
    let parsed = adapter::parse(&log(vec![message("m1"),message("m2"),message("m3"),
        json!({"$patch":{"id":"m1","content":"PRIVATE replacement","updates":[{"id":"m3","toolCalls":[{"id":"tool","result":"PRIVATE"}]}],"removeIds":["m2"],"orderIds":["m3","m1"]}}),
        json!({"$rewindTo":"m1"})]),"session-a").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].id, "session-a:m3");
    assert_eq!(parsed.records[0].total_tokens, 130);
}

#[test]
fn metadata_set_checkpoint_replaces_history_without_timestamp_inheritance() {
    let mut missing_time = message("m3");
    missing_time.as_object_mut().unwrap().remove("timestamp");
    let parsed = adapter::parse(&log(vec![json!({"sessionId":"s","projectHash":"p","startTime":"2026-11-07T12:00:00Z"}),message("m1"),
        json!({"$set":{"messages":[message("m2"),missing_time],"lastUpdated":"2026-11-07T13:00:00Z"}}),
        json!({"$set":{"summary":"PRIVATE summary"}})]),"session-a").unwrap();
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].id, "session-a:m2");
    assert_eq!(
        parsed.warnings,
        vec!["Gemini usage metadata is incomplete or invalid."]
    );
}

#[test]
fn missing_null_or_invalid_accounting_is_not_known_zero_and_update_removes_old() {
    let mut records = Vec::new();
    for tokens in [
        Value::Null,
        json!({}),
        json!({"input":100,"output":20,"total":120}),
        json!({"input":-1,"output":20,"cached":0,"total":120}),
        json!({"input":1.5,"output":20,"cached":0,"total":120}),
        json!({"input":"100","output":20,"cached":0,"total":120}),
        json!({"input":100,"output":20,"cached":101,"total":130}),
        json!({"input":100,"output":20,"cached":0,"thoughts":-1,"total":130}),
    ] {
        let mut m = message("m1");
        m["tokens"] = tokens;
        records.push(m);
    }
    records.insert(0, message("m1"));
    let parsed = adapter::parse(&log(records), "session-a").unwrap();
    assert!(parsed.records.is_empty());
    assert_eq!(
        parsed.warnings,
        vec!["Gemini usage metadata is incomplete or invalid."]
    );
    let overflow = legacy(vec![message("m1")]).replace("130", "18446744073709551616");
    let parsed = adapter::parse(&overflow, "session-a").unwrap();
    assert!(parsed.records.is_empty());
    assert_eq!(
        parsed.warnings,
        vec!["Gemini usage metadata is incomplete or invalid."]
    );
}

#[test]
fn user_and_tool_records_do_not_count_but_explicit_zero_does() {
    let mut user = message("u");
    user["type"] = json!("user");
    let mut tool = message("t");
    tool["type"] = json!("tool");
    let mut zero = message("z");
    zero["tokens"] = json!({"input":0,"output":0,"cached":0,"total":0});
    let parsed = adapter::parse(&legacy(vec![user, tool, zero]), "session-a").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].id, "session-a:z");
    assert_eq!(parsed.records[0].total_tokens, 0);
}

#[test]
fn invalid_identity_model_and_timestamps_are_not_fabricated() {
    for (key, value) in [
        ("id", json!("bad\nPRIVATE")),
        ("id", json!("x".repeat(257))),
        ("model", Value::Null),
        ("model", json!("")),
        ("model", json!("bad\u{1b}model")),
        ("model", json!("x".repeat(257))),
        ("timestamp", json!("2026-02-30T00:00:00Z")),
        ("timestamp", json!("2026-11-07")),
        ("timestamp", Value::Null),
    ] {
        let mut m = message("m1");
        m[key] = value;
        let parsed = adapter::parse(&legacy(vec![m]), "session-a").unwrap();
        assert!(parsed.records.is_empty(), "{key}");
        assert_eq!(
            parsed.warnings,
            vec!["Gemini usage metadata is incomplete or invalid."]
        );
    }
}

#[test]
fn unsupported_controls_and_malformed_input_fail_closed_privately() {
    for input in [
        "PRIVATE broken".to_string(),
        "".into(),
        log(vec![message("m1"), json!({"$unknown":"PRIVATE"})]),
        log(vec![
            message("m1"),
            json!({"$patch":{"id":"m1","tokens":{"total":999},"content":"PRIVATE"}}),
        ]),
        log(vec![message("m1"), json!({"$rewindTo":null})]),
        log(vec![message("m1"), json!({"$set":{"messages":null}})]),
    ] {
        assert_eq!(
            adapter::parse(&input, "session-a").unwrap_err(),
            "Gemini session data is invalid or unsupported."
        );
    }
    for hint in ["", "../PRIVATE", "bad\nPRIVATE"] {
        assert_eq!(
            adapter::parse(&legacy(vec![message("m1")]), hint).unwrap_err(),
            "Gemini session identifier is invalid."
        );
    }
}

#[test]
fn top_level_json_array_like_logs_json_is_silently_ignored() {
    let logs = json!([
        {"sessionId": "s1", "messageId": 0, "type": "user", "message": "hello"}
    ])
    .to_string();
    let parsed = adapter::parse(&logs, "logs-hint").unwrap();
    assert!(parsed.records.is_empty());
    assert!(parsed.warnings.is_empty());

    let empty_arr = adapter::parse("[]", "logs-hint").unwrap();
    assert!(empty_arr.records.is_empty());
    assert!(empty_arr.warnings.is_empty());
}

#[test]
fn normalizes_inclusive_vs_exclusive_cached_tokens_and_preserves_reported_total() {
    // Inclusive cached: input(100) includes cached(30); total = 100 + 20 + 7 + 3 = 130
    let inclusive = json!({
        "id": "m_inc",
        "type": "gemini",
        "timestamp": "2026-11-07T12:00:00Z",
        "model": "gemini-2.5-pro",
        "tokens": {"input": 100, "output": 20, "cached": 30, "thoughts": 7, "tool": 3, "total": 130}
    });
    // Exclusive cached: input(10) excludes cached(50); total = 10 + 20 + 50 + 5 + 2 = 87
    let exclusive = json!({
        "id": "m_exc",
        "type": "gemini",
        "timestamp": "2026-11-07T12:01:00Z",
        "model": "gemini-2.5-pro",
        "tokens": {"input": 10, "output": 20, "cached": 50, "thoughts": 5, "tool": 2, "total": 87}
    });
    let parsed = adapter::parse(&legacy(vec![inclusive, exclusive]), "session-norm").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 2);

    let r0 = &parsed.records[0];
    assert_eq!(
        (
            r0.input_tokens,
            r0.output_tokens,
            r0.cached_tokens,
            r0.cache_creation_tokens,
            r0.cache_read_tokens,
            r0.total_tokens
        ),
        (73, 20, 30, 0, 30, 130)
    );
    assert!(r0.input_tokens + r0.output_tokens + r0.cached_tokens <= r0.total_tokens);

    let r1 = &parsed.records[1];
    assert_eq!(
        (
            r1.input_tokens,
            r1.output_tokens,
            r1.cached_tokens,
            r1.cache_creation_tokens,
            r1.cache_read_tokens,
            r1.total_tokens
        ),
        (12, 20, 50, 0, 50, 87)
    );
    assert!(r1.input_tokens + r1.output_tokens + r1.cached_tokens <= r1.total_tokens);
}

#[test]
fn supports_token_key_aliases_and_stats_summary_records() {
    let aliased_msg = json!({
        "id": "m_alias",
        "type": "gemini",
        "timestamp": "2026-11-07T12:00:00Z",
        "model": "gemini-2.5-flash",
        "tokens": {
            "prompt_tokens": 80,
            "candidates_tokens": 15,
            "cached_tokens": 20,
            "reasoning_tokens": 5,
            "tool_tokens": 4,
            "total_tokens": 104
        }
    });
    let parsed_alias = adapter::parse(&legacy(vec![aliased_msg]), "session-alias").unwrap();
    assert!(parsed_alias.warnings.is_empty());
    assert_eq!(parsed_alias.records.len(), 1);
    let ra = &parsed_alias.records[0];
    // 80 + 15 + 5 + 4 = 104 == total => inclusive cached: input = 80 - 20 + 4 = 64
    assert_eq!(
        (ra.input_tokens, ra.output_tokens, ra.cached_tokens, ra.total_tokens),
        (64, 15, 20, 104)
    );

    // stats.models.<model>.tokens summary record
    let stats_models = json!({
        "timestamp": "2026-11-07T12:10:00Z",
        "stats": {
            "models": {
                "gemini-2.5-pro": {
                    "tokens": {
                        "prompt": 200,
                        "candidates": 50,
                        "cached": 80,
                        "thoughts": 10,
                        "tool": 5,
                        "total": 265
                    }
                },
                "gemini-2.5-flash": {
                    "tokens": {
                        "input_tokens": 40,
                        "output_tokens": 10
                    }
                }
            }
        }
    });
    let parsed_models = adapter::parse(&stats_models.to_string(), "session-stats").unwrap();
    assert!(parsed_models.warnings.is_empty());
    assert_eq!(parsed_models.records.len(), 2);
    let pro = parsed_models
        .records
        .iter()
        .find(|r| r.model == "gemini-2.5-pro")
        .unwrap();
    // subtract min(200, 80) => 120 + tool(5) = 125
    assert_eq!(
        (pro.input_tokens, pro.output_tokens, pro.cached_tokens, pro.total_tokens),
        (125, 50, 80, 265)
    );
    let flash = parsed_models
        .records
        .iter()
        .find(|r| r.model == "gemini-2.5-flash")
        .unwrap();
    assert_eq!(
        (flash.input_tokens, flash.output_tokens, flash.cached_tokens, flash.total_tokens),
        (40, 10, 0, 50)
    );

    // result.stats flat summary record
    let result_stats = json!({
        "createdAt": "2026-11-07T12:20:00Z",
        "model": "gemini-2.5-pro",
        "result": {
            "stats": {
                "input": 60,
                "output": 15,
                "cached": 25
            }
        }
    });
    let parsed_flat = adapter::parse(&result_stats.to_string(), "session-flat").unwrap();
    assert!(parsed_flat.warnings.is_empty());
    assert_eq!(parsed_flat.records.len(), 1);
    let rf = &parsed_flat.records[0];
    // subtract min(60, 25) => 35 uncached + 25 cached + 15 output = 75
    assert_eq!(
        (rf.input_tokens, rf.output_tokens, rf.cached_tokens, rf.total_tokens),
        (35, 15, 25, 75)
    );
}
