use ai_usage_monitor::{
    ContextInfo, Discovery, McpInfo, ModelDayUsage, Observation, Service, StatusReader,
};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::Notify;

struct Found;
impl Discovery for Found {
    fn installed(&self, _: &str) -> bool {
        true
    }
}
fn evidence() -> Observation {
    Observation {
        context: Some(ContextInfo {
            mcps: vec![McpInfo {
                name: "fixture-mcp".into(),
                status: "configured".into(),
            }],
            skills: vec!["fixture-skill".into()],
            note: "Fixture".into(),
        }),
        model_history: vec![ModelDayUsage {
            input_tokens: None,
            output_tokens: None,
            cached_tokens: None,
            cache_creation_tokens: None,
            cache_read_tokens: None,
            date: "2026-11-07".into(),
            model: "fixture".into(),
            total_tokens: 42,
        }],
        ..Default::default()
    }
}
struct Reader;
impl StatusReader for Reader {
    fn read(&self, _: String) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
        Box::pin(async { Ok(evidence()) })
    }
}
#[test]
fn registry_distinguishes_gemini_cli_from_antigravity_without_reading_context() {
    let s = Service::new(Found, Reader).scan_agents();
    let ids: Vec<_> = s.agents.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "codex",
            "claude",
            "antigravity",
            "gemini",
            "opencode",
            "hermes",
            "ollama",
            "lmstudio"
        ]
    );
    assert!(s
        .agents
        .iter()
        .all(|a| a.context.is_none() && a.model_history.is_empty() && !a.connected));
}
#[tokio::test]
async fn context_and_model_history_publish_only_after_permission_and_clear_on_revoke() {
    let s = Service::new(Found, Reader);
    s.scan_agents();
    assert!(s
        .refresh_usage()
        .await
        .agents
        .iter()
        .all(|a| a.context.is_none()));
    s.set_permission("codex", true).unwrap();
    let result = s.refresh_usage().await;
    let a = result.agents.iter().find(|a| a.id == "codex").unwrap();
    assert_eq!(a.context.as_ref().unwrap().skills, vec!["fixture-skill"]);
    assert_eq!(a.model_history[0].total_tokens, 42);
    let result = s.set_permission("codex", false).unwrap();
    let a = result.agents.iter().find(|a| a.id == "codex").unwrap();
    assert!(a.context.is_none() && a.model_history.is_empty() && !a.connected);
}
struct Waiting(Arc<Notify>, Arc<Notify>);
impl StatusReader for Waiting {
    fn read(&self, _: String) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
        let (start, end) = (self.0.clone(), self.1.clone());
        Box::pin(async move {
            start.notify_one();
            end.notified().await;
            Ok(evidence())
        })
    }
}
#[tokio::test]
async fn late_context_cannot_cross_a_revocation_generation() {
    let (start, end) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let s = Arc::new(Service::new(Found, Waiting(start.clone(), end.clone())));
    s.scan_agents();
    s.set_permission("codex", true).unwrap();
    let task = tokio::spawn({
        let s = s.clone();
        async move { s.refresh_usage().await }
    });
    start.notified().await;
    s.set_permission("codex", false).unwrap();
    end.notify_one();
    assert!(task
        .await
        .unwrap()
        .agents
        .iter()
        .all(|a| a.context.is_none() && a.model_history.is_empty()));
}
