use ai_usage_monitor::hermes;
use serde_json::json;
#[test]
fn official_windows_are_used_percent_not_remaining_and_private_fields_are_dropped() {
    let o = hermes::parse(&json!({"provider":"PRIVATE", "details":["SECRET"],"windows":[{"label":"5h","used_percent":37.0,"resets_at":"2026-11-08T00:00:00Z"},{"label":"weekly","used_percent":12.5,"resets_at":null},{"label":"credits","used_percent":null,"resets_at":null}],"unavailable_reason":null})).unwrap();
    assert_eq!(
        o.quotas
            .iter()
            .map(|q| q.remaining_percent)
            .collect::<Vec<_>>(),
        vec![Some(63.0), Some(87.5), None]
    );
    assert!(o.usage.is_none() && o.history.is_empty());
    assert!(!serde_json::to_string(&o.quotas)
        .unwrap()
        .contains("PRIVATE"));
}
#[test]
fn unsupported_private_errors_and_invalid_values_fail_closed() {
    for v in [
        json!({}),
        json!({"windows":[],"unavailable_reason":"SECRET"}),
        json!({"windows":[{"label":"5h","used_percent":101,"resets_at":null}]}),
        json!({"windows":[{"label":"5h","used_percent":1,"resets_at":"bad"}]}),
    ] {
        assert!(!hermes::parse(&v).err().unwrap().contains("SECRET"));
    }
}
