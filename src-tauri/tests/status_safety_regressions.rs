use ai_usage_monitor::status_command::{CommandFuture, CommandRunner, Operation};
use ai_usage_monitor::{
    antigravity, opencode_status, Discovery, Observation, Service, StatusReader,
};
use serde_json::json;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
struct Found;
impl Discovery for Found {
    fn installed(&self, id: &str) -> bool {
        id == "antigravity"
    }
}
struct Wire {
    started: Notify,
    release: Notify,
    calls: Mutex<Vec<Operation>>,
}
impl CommandRunner for Wire {
    fn run(&self, op: Operation) -> CommandFuture<'_> {
        self.calls.lock().unwrap().push(op);
        Box::pin(async move {
            if op == Operation::AntigravityVersion {
                self.started.notify_one();
                self.release.notified().await;
                Ok(b"1.1.11\n".to_vec())
            } else {
                Ok(br#"{"command":{"data":{"groups":[{"name":"Pro","buckets":[{"id":"one","name":"Model","window":"5h","remaining_fraction":0.5,"reset_time":"2026-11-10T12:00:00Z"}]}]}}}"#.to_vec())
            }
        })
    }
}
struct Reader(Arc<Wire>);
impl StatusReader for Reader {
    fn read(&self, _: String) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
        let w = self.0.clone();
        Box::pin(async move { antigravity::read_with(w.as_ref()).await })
    }
}
#[tokio::test]
async fn revoke_during_version_never_starts_authenticated_usage() {
    let w = Arc::new(Wire {
        started: Notify::new(),
        release: Notify::new(),
        calls: Mutex::new(vec![]),
    });
    let s = Arc::new(Service::new(Found, Reader(w.clone())));
    s.scan_agents();
    s.set_permission("antigravity", true).unwrap();
    let task = tokio::spawn({
        let s = s.clone();
        async move { s.refresh_usage().await }
    });
    w.started.notified().await;
    s.set_permission("antigravity", false).unwrap();
    w.release.notify_one();
    assert!(task
        .await
        .unwrap()
        .agents
        .iter()
        .all(|a| !a.connected && a.context.is_none() && a.quotas.is_empty()));
    assert_eq!(
        *w.calls.lock().unwrap(),
        vec![Operation::AntigravityVersion]
    );
}
#[test]
fn namespaced_model_ids_preserve_supported_usage() {
    let t = json!({"input":2,"output":3,"reasoning":0,"cache":{"read":0,"write":0}});
    let v = json!({"data":{"range":{"from":1794000000000u64,"to":1794086400000u64},"tokens":t,"cost":0.01,"models":[{"model":{"id":"anthropic/claude-sonnet-4.5","providerID":"openrouter"},"tokens":t}]}});
    let o = opencode_status::parse_stats(&v).unwrap();
    assert_eq!(o.usage.unwrap().total_tokens, 5);
    assert_eq!(
        o.model_usage[0].model,
        "openrouter:anthropic/claude-sonnet-4.5"
    );
    assert_eq!(o.model_usage[0].total_tokens, 5);
    assert!(o.model_history.is_empty());
}
