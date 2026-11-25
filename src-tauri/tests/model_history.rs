use ai_usage_monitor::usage::{aggregate, TokenRecord};
#[test]
fn daily_model_history_preserves_date_model_pairs_and_existing_totals() {
    let now = "2026-11-07T12:00:00Z".parse().unwrap();
    let records = [
        ("a", "2026-11-06T09:00:00Z", "model-a", 10),
        ("b", "2026-11-07T09:00:00Z", "model-b", 30),
        ("c", "2026-11-07T10:00:00Z", "model-a", 20),
    ]
    .into_iter()
    .map(|(id, time, model, n)| TokenRecord {
        id: id.into(),
        timestamp: time.parse().unwrap(),
        model: model.into(),
        input_tokens: n,
        output_tokens: 0,
        cached_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_tokens: n,
    })
    .collect();
    let s = aggregate(records, now, "Fixture").unwrap();
    assert_eq!(s.usage.total_tokens, 60);
    assert_eq!(
        s.history.iter().map(|r| r.total_tokens).collect::<Vec<_>>(),
        vec![10, 50]
    );
    assert_eq!(
        serde_json::to_value(&s.history).unwrap()
            .as_array().unwrap().iter().map(|row| row["recordedResponses"].as_u64()).collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
    assert_eq!(
        s.model_usage
            .iter()
            .map(|r| r.total_tokens)
            .collect::<Vec<_>>(),
        vec![30, 30]
    );
    let rows: Vec<_> = s
        .model_history
        .iter()
        .map(|r| (r.date.as_str(), r.model.as_str(), r.total_tokens))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("2026-11-06", "model-a", 10),
            ("2026-11-07", "model-a", 20),
            ("2026-11-07", "model-b", 30)
        ]
    );
    let json = serde_json::to_value(&s.model_history).unwrap();
    assert_eq!((json[2]["inputTokens"].as_u64(), json[2]["outputTokens"].as_u64()), (Some(30), Some(0)));
}
