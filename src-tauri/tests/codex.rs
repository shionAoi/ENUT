use ai_usage_monitor::codex::{parse_rate_limits, status_requests};
use serde_json::json;

#[test]
fn documented_status_messages_never_start_inference() {
    let messages = status_requests();
    let methods: Vec<_> = messages
        .iter()
        .map(|m| m["method"].as_str().unwrap())
        .collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "account/read",
            "account/rateLimits/read"
        ]
    );
    assert_eq!(messages[2]["params"]["refreshToken"], false);
}

#[test]
fn rate_windows_preserve_actual_reset_and_prefer_multibucket_results() {
    let quotas = parse_rate_limits(&json!({
        "rateLimits": {"primary":{"usedPercent":99}},
        "rateLimitsByLimitId": {"codex": {
            "limitName":"Codex", "primary":{"usedPercent":38,"windowDurationMins":300,"resetsAt":1794070800},
            "secondary":{"usedPercent":12,"resetsAt":null}
        }}
    })).unwrap();
    assert_eq!(quotas.len(), 2);
    assert_eq!(quotas[0].remaining_percent, Some(62.0));
    assert_eq!(quotas[0].resets_at.as_deref(), Some("2026-11-07T17:00:00Z"));
    assert_eq!(quotas[1].remaining_percent, Some(88.0));
    assert!(quotas[1].resets_at.is_none());
}

#[test]
fn unknown_or_invalid_quota_values_are_not_invented() {
    assert!(parse_rate_limits(&json!({})).is_err());
    assert!(parse_rate_limits(&json!({"rateLimits":{"primary":{"usedPercent":101}}})).is_err());
    let quotas = parse_rate_limits(&json!({"rateLimits":{"primary":{"resetsAt":-1}}})).unwrap();
    assert_eq!(quotas[0].remaining_percent, None);
    assert_eq!(quotas[0].resets_at, None);
}

#[tokio::test]
async fn protocol_correlates_responses_and_ignores_notifications() {
    let input = concat!(
        "{\"id\":0,\"result\":{}}\n",
        "{\"method\":\"notice\",\"params\":{}}\n",
        "{\"id\":1,\"result\":{\"account\":{\"type\":\"chatgpt\"}}}\n",
        "{\"id\":2,\"result\":{\"rateLimits\":{\"primary\":{\"usedPercent\":25}}}}\n"
    );
    let mut output = vec![];
    let observation = ai_usage_monitor::codex::exchange(input.as_bytes(), &mut output)
        .await
        .unwrap();
    assert_eq!(observation.quotas[0].remaining_percent, Some(75.0));
    assert_eq!(observation.authentication.as_deref(), Some("available"));
    let sent: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(sent, status_requests());
}

#[tokio::test]
async fn api_key_mode_never_requests_subscription_status() {
    let input =
        "{\"id\":0,\"result\":{}}\n{\"id\":1,\"result\":{\"account\":{\"type\":\"apiKey\"}}}\n";
    let mut output = vec![];
    let observation = ai_usage_monitor::codex::exchange(input.as_bytes(), &mut output)
        .await
        .unwrap();
    assert_eq!(observation.authentication.as_deref(), Some("available"));
    assert!(observation.quotas.is_empty());
    assert!(observation
        .message
        .as_deref()
        .unwrap()
        .contains("API-key authentication"));
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("account/rateLimits/read"));
}

#[tokio::test]
async fn only_explicit_signed_out_evidence_requires_login_without_quota_read() {
    let input = "{\"id\":0,\"result\":{}}\n{\"id\":1,\"result\":{\"account\":null,\"requiresOpenaiAuth\":true}}\n";
    let mut output = vec![];
    let observation = ai_usage_monitor::codex::exchange(input.as_bytes(), &mut output)
        .await
        .unwrap();
    assert_eq!(observation.authentication.as_deref(), Some("required"));
    assert!(observation.quotas.is_empty());
    assert!(observation
        .message
        .as_deref()
        .unwrap()
        .contains("device code on the provider website"));
    assert!(!String::from_utf8(output)
        .unwrap()
        .contains("account/rateLimits/read"));
    for result in [
        json!({}),
        json!({"account":null}),
        json!({"account":null,"requiresOpenaiAuth":false}),
        json!({"account":null,"requiresOpenaiAuth":"true"}),
        json!({"account":{"type":"unknown"},"requiresOpenaiAuth":true}),
    ] {
        let input = format!(
            "{{\"id\":0,\"result\":{{}}}}\n{}\n",
            json!({"id":1,"result":result})
        );
        let mut output = vec![];
        assert!(
            ai_usage_monitor::codex::exchange(input.as_bytes(), &mut output)
                .await
                .is_err()
        );
        assert!(!String::from_utf8(output)
            .unwrap()
            .contains("account/rateLimits/read"));
    }
}

#[tokio::test]
async fn oversized_or_error_output_is_rejected_without_echoing_sensitive_content() {
    let error = "{\"id\":0,\"error\":{\"message\":\"sensitive credential\"}}\n";
    let message = ai_usage_monitor::codex::exchange(error.as_bytes(), vec![])
        .await
        .err()
        .unwrap();
    assert!(!message.contains("sensitive credential"));
    let oversized = vec![b'x'; 300_000];
    assert!(
        ai_usage_monitor::codex::exchange(oversized.as_slice(), vec![])
            .await
            .is_err()
    );
}
