use ai_usage_monitor::usage;
#[path = "../src/claude_usage.rs"]
mod adapter;
use serde_json::{json, Value};

fn message() -> Value {
    json!({"type":"message","role":"assistant","id":"msg_example",
        "model":"claude-sonnet-4-5","stop_reason":"end_turn",
        "usage":{"input_tokens":10,"output_tokens":7,
        "cache_read_input_tokens":20,"cache_creation_input_tokens":30}})
}
fn event(event: Value) -> Value {
    json!({"type":"stream_event","timestamp":"2026-11-07T12:00:00+02:00",
        "parent_tool_use_id":null,"event":event})
}
fn stream(msg: Value, output: Value) -> String {
    [
        event(json!({"type":"message_start","message":msg})),
        event(
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},
        "usage":{"output_tokens":output}}),
        ),
        event(json!({"type":"message_stop"})),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n")
}
fn assert_unknown(text: &str) {
    let result = adapter::parse(text, "opaque-session").unwrap();
    assert!(result.records.is_empty());
    assert!(!result.warnings.is_empty());
    assert!(result
        .warnings
        .iter()
        .all(|w| w == "Claude usage is incomplete or unsupported."));
}

#[test]
fn complete_stream_uses_final_output_and_separate_cache_buckets() {
    let result = adapter::parse(&stream(message(), json!(9)), "opaque").unwrap();
    assert!(result.warnings.is_empty());
    assert_eq!(result.records.len(), 1);
    let r = &result.records[0];
    assert_eq!(
        (
            r.input_tokens,
            r.output_tokens,
            r.cached_tokens,
            r.cache_creation_tokens,
            r.cache_read_tokens,
            r.total_tokens
        ),
        (10, 9, 50, 30, 20, 69)
    );
    assert_eq!(r.model, "claude-sonnet-4-5");
    assert_eq!(r.timestamp.to_rfc3339(), "2026-11-07T10:00:00+00:00");
    assert_eq!(r.id, "claude:msg_example");
}

#[test]
fn repeated_message_ids_replace_never_sum_and_survive_copies() {
    let first = stream(message(), json!(4));
    let last = stream(message(), json!(9));
    let result = adapter::parse(&format!("{first}\n{last}"), "copy-a").unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].total_tokens, 69);
    assert_eq!(
        result.records[0].id,
        adapter::parse(&last, "copy-b").unwrap().records[0].id
    );
    assert!(result.warnings.is_empty());
}

#[test]
fn cumulative_deltas_replace_and_final_input_updates_are_used() {
    let text = [
        event(json!({"type":"message_start","message":message()})),
        event(json!({"type":"message_delta","usage":{"output_tokens":3}})),
        event(
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},
        "usage":{"input_tokens":11,"output_tokens":8}}),
        ),
        event(json!({"type":"message_stop"})),
    ]
    .iter()
    .map(Value::to_string)
    .collect::<Vec<_>>()
    .join("\n");
    let result = adapter::parse(&text, "opaque").unwrap();
    assert!(result.warnings.is_empty());
    assert_eq!(result.records[0].total_tokens, 69);
}

#[test]
fn ordinary_assistant_rows_extract_normalized_usage_and_skip_empty_or_result() {
    let assistant = adapter::parse(
        &json!({
            "type": "assistant",
            "timestamp": "2026-11-07T10:00:00Z",
            "requestId": "req_1",
            "sessionId": "sess_1",
            "message": message()
        })
        .to_string(),
        "opaque-session",
    )
    .unwrap();
    assert!(assistant.warnings.is_empty());
    assert_eq!(assistant.records.len(), 1);
    let r = &assistant.records[0];
    assert_eq!(
        (
            r.input_tokens,
            r.output_tokens,
            r.cached_tokens,
            r.cache_creation_tokens,
            r.cache_read_tokens,
            r.total_tokens
        ),
        (10, 7, 50, 30, 20, 67)
    );
    assert_eq!(r.model, "claude-sonnet-4-5");

    let result_row = adapter::parse(
        &json!({"type":"result","usage":message()["usage"]}).to_string(),
        "opaque-session",
    )
    .unwrap();
    assert!(result_row.records.is_empty());

    for usage in [Value::Null, json!({})] {
        let skipped = adapter::parse(
            &json!({"type":"assistant","timestamp":"2026-11-07T10:00:00Z","message":{"id":"msg_x","model":"claude-sonnet-4-5","usage":usage}}).to_string(),
            "opaque-session",
        )
        .unwrap();
        assert!(skipped.records.is_empty());
    }
}

#[test]
fn user_tool_and_content_token_lookalikes_are_ignored() {
    for kind in ["user", "tool", "system"] {
        let result = adapter::parse(
            &json!({"type":kind,"message":message(),"usage":message()["usage"]}).to_string(),
            "opaque",
        )
        .unwrap();
        assert!(result.records.is_empty());
        assert!(result.warnings.is_empty());
    }
    assert_unknown(&stream(
        {
            let mut m = message();
            m["role"] = json!("user");
            m
        },
        json!(9),
    ));
}

#[test]
fn malformed_missing_negative_fractional_and_overflow_counts_stay_unknown() {
    for field in [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ] {
        for value in [Value::Null, json!(-1), json!(1.5), json!("4"), json!(true)] {
            let mut m = message();
            m["usage"][field] = value;
            assert_unknown(&stream(m, json!(9)));
        }
        let mut m = message();
        m["usage"].as_object_mut().unwrap().remove(field);
        assert_unknown(&stream(m, json!(9)));
    }
    for output in [Value::Null, json!(-1), json!(1.5), json!(u64::MAX)] {
        assert_unknown(&stream(message(), output));
    }
    let mut m = message();
    m["usage"]["input_tokens"] = json!(u64::MAX);
    assert_unknown(&stream(m, json!(9)));
}

#[test]
fn validates_models_ids_timestamps_and_stream_completion() {
    for model in ["", "bad\nmodel", "bad/model", &"x".repeat(129)] {
        let mut m = message();
        m["model"] = json!(model);
        assert_unknown(&stream(m, json!(9)));
    }
    let mut m = message();
    m["id"] = Value::Null;
    assert_unknown(&stream(m, json!(9)));
    for time in ["not-a-date", "2026-02-30T00:00:00Z", ""] {
        assert_unknown(&stream(message(), json!(9)).replace("2026-11-07T12:00:00+02:00", time));
    }
    assert_unknown(&event(json!({"type":"message_start","message":message()})).to_string());
    assert_unknown(&event(json!({"type":"message_stop"})).to_string());
    assert_unknown(&stream(message(), json!(9)).replace("message_delta", "unknown_delta"));
}

#[test]
fn failures_are_generic_and_empty_input_stays_absent() {
    assert_eq!(
        adapter::parse("{secret-path-credential", "private-identity").unwrap_err(),
        "Invalid Claude usage JSONL."
    );
    assert_eq!(
        adapter::parse("[]", "opaque").unwrap_err(),
        "Invalid Claude usage JSONL."
    );
    let result = adapter::parse("\n ", "opaque").unwrap();
    assert!(result.records.is_empty());
    assert!(result.warnings.is_empty());
}

#[test]
fn explicit_zero_is_known_but_later_incomplete_duplicate_invalidates_file() {
    let mut m = message();
    for field in [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ] {
        m["usage"][field] = json!(0);
    }
    let complete = stream(m.clone(), json!(0));
    let result = adapter::parse(&complete, "opaque").unwrap();
    assert!(result.warnings.is_empty());
    let r = &result.records[0];
    assert_eq!(
        (
            r.input_tokens,
            r.output_tokens,
            r.cached_tokens,
            r.total_tokens
        ),
        (0, 0, 0, 0)
    );
    let incomplete = event(json!({"type":"message_start","message":m}));
    assert_unknown(&format!("{complete}\n{incomplete}"));
}

#[test]
fn cache_updates_replace_and_interleaved_subagent_stream_is_rejected() {
    let text = stream(message(), json!(9)).replace(
        "\"output_tokens\":9",
        "\"output_tokens\":9,\"input_tokens\":11,\"cache_read_input_tokens\":12,\"cache_creation_input_tokens\":13",
    );
    let result = adapter::parse(&text, "opaque").unwrap();
    assert!(result.warnings.is_empty());
    let r = &result.records[0];
    assert_eq!(
        (
            r.input_tokens,
            r.output_tokens,
            r.cached_tokens,
            r.cache_creation_tokens,
            r.cache_read_tokens,
            r.total_tokens
        ),
        (11, 9, 25, 13, 12, 45)
    );
    assert_unknown(&text.replace(
        "\"parent_tool_use_id\":null",
        "\"parent_tool_use_id\":\"tool_example\"",
    ));
}

#[test]
fn streaming_chunk_dedupe_prefers_higher_total_and_cache_creation_split_tiebreak() {
    let chunk1 = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:01Z",
        "sessionId": "s1",
        "requestId": "req_1",
        "message": {
            "id": "msg_1",
            "model": "claude-sonnet-4-5",
            "usage": {"input_tokens": 100, "output_tokens": 5, "cache_creation_input_tokens": 20, "cache_read_input_tokens": 10}
        }
    });
    let chunk2 = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:02Z",
        "sessionId": "s1",
        "requestId": "req_1",
        "message": {
            "id": "msg_1",
            "model": "claude-sonnet-4-5",
            "usage": {"input_tokens": 100, "output_tokens": 40, "cache_creation_input_tokens": 20, "cache_read_input_tokens": 10}
        }
    });
    let chunk3_with_split = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:03Z",
        "sessionId": "s1",
        "requestId": "req_1",
        "message": {
            "id": "msg_1",
            "model": "claude-sonnet-4-5",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 40,
                "cache_creation_input_tokens": 20,
                "cache_creation": {"ephemeral_5m_input_tokens": 8, "ephemeral_1h_input_tokens": 12},
                "cache_read_input_tokens": 10
            }
        }
    });
    let text = format!("{chunk1}\n{chunk2}\n{chunk3_with_split}");
    let parsed = adapter::parse(&text, "s1").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 1);
    let r = &parsed.records[0];
    assert_eq!(r.output_tokens, 40);
    assert_eq!(r.cache_creation_tokens, 20);
    assert_eq!(r.total_tokens, 170);
    assert_eq!(r.timestamp.to_rfc3339(), "2026-11-07T10:00:03+00:00");
}

#[test]
fn non_sidechain_beats_sidechain_and_btw_replay_is_not_double_counted() {
    let main_row = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:01Z",
        "sessionId": "sess_main",
        "requestId": "req_main",
        "isSidechain": false,
        "message": {
            "id": "msg_shared",
            "model": "claude-sonnet-4-5",
            "usage": {"input_tokens": 50, "output_tokens": 20, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 10}
        }
    });
    let btw_replay = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:02Z",
        "sessionId": "sess_main",
        "requestId": "req_btw_different",
        "isSidechain": true,
        "message": {
            "id": "msg_shared",
            "model": "claude-sonnet-4-5",
            "usage": {"input_tokens": 50, "output_tokens": 999, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 10}
        }
    });
    // Even if sidechain appears first AND has higher tokens, non-sidechain wins and absorbs the replay.
    let files = vec![
        usage::SessionFile {
            hint: "sess_main".into(),
            path_hint: "root0/proj/subagents/agent-1.jsonl".into(),
            text: btw_replay.to_string(),
        },
        usage::SessionFile {
            hint: "sess_main".into(),
            path_hint: "root0/proj/sess_main.jsonl".into(),
            text: main_row.to_string(),
        },
    ];
    let parsed = adapter::parse_files(&files);
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].output_tokens, 20);
    assert_eq!(parsed.records[0].total_tokens, 80);
}

#[test]
fn requestless_blocks_link_via_uuid_parent_chain_and_keep_distinct_turns_separate() {
    let block1 = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:01Z",
        "sessionId": "s1",
        "uuid": "u1",
        "parentUuid": "u0",
        "message": {
            "id": "msg_reqless",
            "model": "claude-sonnet-4-5",
            "content": [{"type": "tool_use", "id": "tu_1"}],
            "usage": {"input_tokens": 100, "output_tokens": 10, "cache_creation_input_tokens": 5, "cache_read_input_tokens": 20}
        }
    });
    let tool_result = json!({
        "type": "user",
        "timestamp": "2026-11-07T10:00:02Z",
        "sessionId": "s1",
        "uuid": "u2",
        "parentUuid": "u1",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "tu_1"}]
        }
    });
    let block2 = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:03Z",
        "sessionId": "s1",
        "uuid": "u3",
        "parentUuid": "u2",
        "message": {
            "id": "msg_reqless",
            "model": "claude-sonnet-4-5",
            "usage": {"input_tokens": 100, "output_tokens": 35, "cache_creation_input_tokens": 5, "cache_read_input_tokens": 20}
        }
    });
    // Separate requestless response reusing msg_id at a later timestamp after a normal user prompt
    let user_prompt = json!({
        "type": "user",
        "timestamp": "2026-11-07T10:01:00Z",
        "sessionId": "s1",
        "uuid": "u4",
        "parentUuid": "u3",
        "message": {"role": "user", "content": [{"type": "text", "text": "next"}]}
    });
    let independent = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:01:05Z",
        "sessionId": "s1",
        "uuid": "u5",
        "parentUuid": "u4",
        "message": {
            "id": "msg_reqless",
            "model": "claude-sonnet-4-5",
            "usage": {"input_tokens": 100, "output_tokens": 12, "cache_creation_input_tokens": 5, "cache_read_input_tokens": 20}
        }
    });
    let text = [block1, tool_result, block2, user_prompt, independent]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let parsed = adapter::parse(&text, "s1").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 2);
    let mut outputs: Vec<u64> = parsed.records.iter().map(|r| r.output_tokens).collect();
    outputs.sort();
    assert_eq!(outputs, vec![12, 35]);
}

#[test]
fn agent_progress_nested_subagent_usage_is_extracted() {
    let progress = json!({
        "type": "progress",
        "sessionId": "sess_parent",
        "data": {
            "type": "agent_progress",
            "message": {
                "timestamp": "2026-11-07T10:05:00Z",
                "requestId": "req_sub_1",
                "isSidechain": true,
                "message": {
                    "id": "msg_sub_1",
                    "model": "claude-haiku-4-5",
                    "usage": {
                        "input_tokens": 40,
                        "output_tokens": 15,
                        "cache_creation_input_tokens": 10,
                        "cache_read_input_tokens": 25
                    }
                }
            }
        }
    });
    let parsed = adapter::parse(&progress.to_string(), "sess_parent").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 1);
    let r = &parsed.records[0];
    assert_eq!(r.model, "claude-haiku-4-5");
    assert_eq!(
        (
            r.input_tokens,
            r.output_tokens,
            r.cached_tokens,
            r.cache_creation_tokens,
            r.cache_read_tokens,
            r.total_tokens
        ),
        (40, 15, 35, 10, 25, 90)
    );
}

#[test]
fn cache_creation_ttl_split_and_advisor_iterations_are_extracted() {
    let row = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:10:00Z",
        "sessionId": "s_adv",
        "requestId": "req_adv",
        "message": {
            "id": "msg_adv",
            "model": "claude-sonnet-4-5",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 50,
                "cache_creation_input_tokens": 9999,
                "cache_creation": {
                    "ephemeral_5m_input_tokens": 100,
                    "ephemeral_1h_input_tokens": 200
                },
                "cache_read_input_tokens": 40,
                "iterations": [
                    {
                        "type": "message",
                        "cache_creation": {"ephemeral_5m_input_tokens": 100, "ephemeral_1h_input_tokens": 200}
                    },
                    {
                        "type": "message",
                        "cache_creation": {"ephemeral_5m_input_tokens": 50, "ephemeral_1h_input_tokens": 25}
                    },
                    {
                        "type": "advisor_message",
                        "model": "claude-opus-4-6",
                        "input_tokens": 30,
                        "output_tokens": 12,
                        "cache_creation_input_tokens": 5,
                        "cache_read_input_tokens": 3
                    }
                ]
            }
        }
    });
    let parsed = adapter::parse(&row.to_string(), "s_adv").unwrap();
    assert!(parsed.warnings.is_empty());
    assert_eq!(parsed.records.len(), 2);
    let main_rec = parsed
        .records
        .iter()
        .find(|r| r.model == "claude-sonnet-4-5")
        .unwrap();
    // Message iterations summed: (100+200) + (50+25) = 375
    assert_eq!(main_rec.cache_creation_tokens, 375);
    assert_eq!(main_rec.cache_read_tokens, 40);
    assert_eq!(main_rec.cached_tokens, 415);
    assert_eq!(main_rec.total_tokens, 10 + 50 + 415);

    let advisor_rec = parsed
        .records
        .iter()
        .find(|r| r.model == "claude-opus-4-6")
        .unwrap();
    assert!(advisor_rec.id.contains("msg_adv:advisor:0"));
    assert_eq!(
        (
            advisor_rec.input_tokens,
            advisor_rec.output_tokens,
            advisor_rec.cache_creation_tokens,
            advisor_rec.cache_read_tokens,
            advisor_rec.total_tokens
        ),
        (30, 12, 5, 3, 50)
    );
}

#[test]
fn api_error_synthetic_model_and_overflow_counts_are_skipped() {
    let api_err = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:00Z",
        "isApiErrorMessage": true,
        "message": message()
    });
    let mut synth_msg = message();
    synth_msg["model"] = json!("<synthetic>");
    let synthetic = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:00Z",
        "message": synth_msg
    });
    let mut overflow_msg = message();
    overflow_msg["usage"]["input_tokens"] = json!(u64::MAX);
    let overflow = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:00Z",
        "message": overflow_msg
    });
    let text = format!("{api_err}\n{synthetic}\n{overflow}");
    let parsed = adapter::parse(&text, "s1").unwrap();
    assert!(parsed.records.is_empty());
}

#[test]
fn parse_files_dedupes_across_files_and_skips_corrupt_lines_with_generic_warning() {
    let row = json!({
        "type": "assistant",
        "timestamp": "2026-11-07T10:00:00Z",
        "sessionId": "s1",
        "requestId": "req_cross",
        "message": message()
    });
    let files = vec![
        usage::SessionFile {
            hint: "s1".into(),
            path_hint: "root0/proj/a.jsonl".into(),
            text: format!("{{corrupt-secret-path\n{row}"),
        },
        usage::SessionFile {
            hint: "s2".into(),
            path_hint: "root1/proj/b.jsonl".into(),
            text: row.to_string(),
        },
    ];
    let parsed = adapter::parse_files(&files);
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].total_tokens, 67);
    assert_eq!(
        parsed.warnings,
        vec!["Claude usage is incomplete or unsupported."]
    );
}
