use ai_usage_monitor::opencode_status;
use serde_json::json;
fn stats() -> serde_json::Value {
    json!({"data":{"range":{"from":1794000000000u64,"to":1794086400000u64},"tokens":{"input":10,"output":20,"reasoning":3,"cache":{"read":4,"write":5}},"cost":0.25,"models":[{"model":{"id":"test","providerID":"vendor"},"tokens":{"input":10,"output":20,"reasoning":3,"cache":{"read":4,"write":5}}}],"activity":[{"date":"2026-11-07","steps":99}]}})
}
#[test]
fn stats_total_matches_official_total_and_activity_is_not_tokens() {
    let o = opencode_status::parse_stats(&stats()).unwrap();
    let u = o.usage.unwrap();
    assert_eq!(
        (
            u.input_tokens,
            u.output_tokens,
            u.cached_tokens,
            u.total_tokens
        ),
        (10, 23, 9, 42)
    );
    assert_eq!(o.model_usage[0].total_tokens, 42);
    assert!(o.history.is_empty() && o.model_history.is_empty());
    let mut v = stats();
    v["data"]["tokens"]["input"] = json!(9007199254740992u64);
    assert!(opencode_status::parse_stats(&v).is_err());
}

#[tokio::test]
async fn native_period_usage_is_saved_without_its_cost_or_artificial_day_rows() {
    use ai_usage_monitor::{Discovery, Observation, Service, StatusReader};
    struct Found;
    impl Discovery for Found {
        fn installed(&self, id: &str) -> bool {
            id == "opencode"
        }
    }
    struct Reader;
    impl StatusReader for Reader {
        fn read(
            &self,
            _: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
        {
            Box::pin(async { opencode_status::parse_stats(&stats()) })
        }
    }
    let path =
        std::env::temp_dir().join(format!("opencode-reported-period-{}", std::process::id()));
    {
        let service = Service::with_storage(Found, Reader, &path);
        service.scan_agents();
        service.set_permission("opencode", true).unwrap();
        let agent = service.refresh_usage().await.agents.remove(0);
        assert_eq!(agent.reported_period_usage.unwrap().total_tokens, 42);
    }
    let service = Service::with_storage(Found, Reader, &path);
    let agent = service.scan_agents().agents.remove(0);
    assert!(agent.reported_period_usage.is_none() && agent.usage_history.is_empty());
    let saved = agent.saved_period_usage.unwrap();
    assert_eq!(saved.total_tokens, 42);
    assert!(saved.spend_usd.is_none());
}
#[test]
fn context_allows_only_names_and_statuses() {
    let c = opencode_status::parse_context(
        Some(&json!({"data":[{"name":"safe","status":{"status":"connected","error":"SECRET"}}]})),
        Some(&json!({"data":[{"name":"skill","path":"PRIVATE","content":"SECRET"}]})),
    )
    .unwrap();
    assert_eq!(c.mcps[0].status, "connected");
    assert_eq!(c.skills, vec!["skill"]);
    let s = serde_json::to_string(&c).unwrap();
    assert!(!s.contains("SECRET") && !s.contains("PRIVATE"));
    assert!(opencode_status::parse_context(
        Some(&json!({"data":[{"name":"safe","status":{"status":"invented"}}]})),
        None
    )
    .is_err());
}
#[test]
fn limits_null_is_unknown_and_console_budgets_are_not_provider_quotas() {
    assert!(opencode_status::parse_limits(&json!({"data":null}))
        .unwrap()
        .is_empty());
    let q=opencode_status::parse_limits(&json!({"data":{"budgets":[{"id":"rule","limit":10,"spent":2.5,"resetsAt":1794086400000u64,"per":"set"}]}})).unwrap();
    assert_eq!(q[0].remaining_percent, Some(75.0));
    assert!(q[0].label.contains("Console"));
}

#[tokio::test]
async fn unsupported_endpoints_preserve_other_status_without_private_errors() {
    use ai_usage_monitor::status_command::{CommandFuture, CommandRunner, Operation};
    struct Fake;
    impl CommandRunner for Fake {
        fn run(&self, op: Operation) -> CommandFuture<'_> {
            Box::pin(async move {
                match op {
                    Operation::OpenCodeStats => Ok(stats().to_string().into_bytes()),
                    Operation::OpenCodeSkills => Ok(
                        br#"{"data":[{"name":"available","content":"SECRET","path":"PRIVATE"}]}"#
                            .to_vec(),
                    ),
                    _ => Err("SECRET".into()),
                }
            })
        }
    }
    let o = opencode_status::read_with(&Fake).await.unwrap();
    assert_eq!(o.usage.unwrap().total_tokens, 42);
    assert!(o.quotas.is_empty());
    assert!(o.active_sessions.is_none());
    let c = o.context.unwrap();
    assert_eq!(c.skills, vec!["available"]);
    assert!(c.note.contains("unavailable"));
    assert!(!c.note.contains("SECRET"));
}

#[test]
fn go_percent_is_used_and_invalid_limits_fail_closed() {
    let meter = json!({"percent":37.0,"limited":false,"resetsAt":1794086400000u64});
    let mut v =
        json!({"data":{"go":{"rolling":meter,"weekly":meter,"monthly":meter},"budgets":[]}});
    let q = opencode_status::parse_limits(&v).unwrap();
    assert_eq!(q.len(), 3);
    assert_eq!(q[0].remaining_percent, Some(63.0));
    v["data"]["go"]["rolling"]["percent"] = json!(101);
    assert!(opencode_status::parse_limits(&v).is_err());
}
