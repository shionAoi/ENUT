use ai_usage_monitor::claude_inputs::parse_inputs;

#[test]
fn ordinary_claude_rows_report_only_reliable_input_and_cache_not_placeholder_output() {
    let text = r#"{"type":"assistant","timestamp":"2026-11-07T10:00:00Z","message":{"role":"assistant","id":"msg-fixture","model":"claude-fixture","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":20,"cache_creation_input_tokens":30}}}"#;
    let rows = parse_inputs(&format!("{text}\n{text}")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].input_tokens, 60);
    assert_eq!(rows[0].cached_tokens, 20);
}

#[test]
fn absent_cache_or_invalid_fields_are_unknown_and_never_echo_private_text() {
    let bad = r#"{"type":"assistant","timestamp":"invalid private timestamp","message":{"role":"assistant","id":"msg-one","model":"fixture","usage":{"input_tokens":5}}}"#;
    let error = parse_inputs(bad).err().unwrap();
    assert!(!error.contains("private timestamp"));
    assert!(
        parse_inputs(r#"{"type":"user","message":{"usage":{"input_tokens":999}}}"#)
            .unwrap()
            .is_empty()
    );
}
