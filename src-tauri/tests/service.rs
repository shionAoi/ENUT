use ai_usage_monitor::{Discovery, Endpoint, Observation, Service, StatusReader};
use serde_json::json;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::Notify;

#[tokio::test]
async fn rescan_removes_installations_that_no_longer_exist() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Changing(Arc<AtomicBool>);
    impl Discovery for Changing {
        fn installed(&self, id: &str) -> bool {
            id == "ollama" && self.0.load(Ordering::SeqCst)
        }
    }
    let present = Arc::new(AtomicBool::new(true));
    let service = Service::new(Changing(present.clone()), Fixture);
    assert_eq!(service.scan_agents().agents.len(), 1);
    service.set_permission("ollama", true).unwrap();
    present.store(false, Ordering::SeqCst);
    assert!(service.scan_agents().agents.is_empty());
    assert!(service.refresh_usage().await.agents.is_empty());
}

struct Found;
impl Discovery for Found {
    fn installed(&self, id: &str) -> bool {
        matches!(id, "codex" | "ollama" | "lmstudio")
    }
}
struct Fixture;
impl StatusReader for Fixture {
    fn read(
        &self,
        id: String,
    ) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
        Box::pin(async move {
            match id.as_str() {
                "ollama" => Observation::ollama(
                    json!({"models":[{"name":"tiny","size":42},{"name":"tiny","size":42}]}),
                    Some(json!({"models":[{"name":"tiny"}]})),
                ),
                "lmstudio" => Err("Local inventory unavailable; server may be stopped.".into()),
                _ => Err("Supported quota access is unavailable.".into()),
            }
        })
    }
}

#[tokio::test]
async fn discovery_is_installation_only_and_metrics_stay_unknown_without_consent() {
    let service = Service::new(Found, Fixture);
    let snapshot = service.scan_agents();
    assert_eq!(snapshot.agents.len(), 3);
    for agent in snapshot.agents {
        assert!(agent.installed);
        assert!(!agent.connected);
        assert!(agent.usage.is_none());
        assert!(agent.quotas.is_empty());
        assert!(agent.models.is_empty());
        assert!(agent.running.is_none());
    }
    assert!(service
        .refresh_usage()
        .await
        .agents
        .iter()
        .all(|a| !a.connected));
    assert!(service.set_permission("not-an-agent", true).is_err());
}

#[tokio::test]
async fn approved_inventory_is_real_deduplicated_and_errors_are_isolated() {
    let service = Service::new(Found, Fixture);
    service.scan_agents();
    service.set_permission("ollama", true).unwrap();
    service.set_permission("lmstudio", true).unwrap();
    let snapshot = service.refresh_usage().await;
    let ollama = snapshot.agents.iter().find(|a| a.id == "ollama").unwrap();
    assert_eq!(ollama.status, "ready");
    assert_eq!(ollama.running, Some(true));
    assert_eq!(ollama.models.len(), 1);
    assert_eq!(ollama.models[0].loaded, Some(true));
    assert!(ollama.usage.is_none());
    assert!(ollama.quotas.is_empty());
    assert_eq!(
        snapshot
            .agents
            .iter()
            .find(|a| a.id == "lmstudio")
            .unwrap()
            .status,
        "unavailable"
    );
    let cleared = service.set_permission("ollama", false).unwrap();
    let a = cleared.agents.iter().find(|a| a.id == "ollama").unwrap();
    assert!(!a.connected);
    assert!(a.models.is_empty());
    assert!(a.updated_at.is_none());
    let wire = serde_json::to_value(cleared).unwrap();
    assert!(wire.get("scannedAt").is_some());
    assert!(wire["agents"][0].get("updatedAt").is_some());
}

struct Waiting {
    started: Arc<Notify>,
    finish: Arc<Notify>,
}
impl StatusReader for Waiting {
    fn read(&self, _: String) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
        let started = self.started.clone();
        let finish = self.finish.clone();
        Box::pin(async move {
            started.notify_one();
            finish.notified().await;
            Observation::ollama(json!({"models":[{"name":"private"}]}), None)
        })
    }
}
#[tokio::test]
async fn revoke_and_regrant_during_refresh_cannot_publish_old_data() {
    let started = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let service = Arc::new(Service::new(
        Found,
        Waiting {
            started: started.clone(),
            finish: finish.clone(),
        },
    ));
    service.scan_agents();
    service.set_permission("ollama", true).unwrap();
    let task = tokio::spawn({
        let service = service.clone();
        async move { service.refresh_usage().await }
    });
    started.notified().await;
    service.set_permission("ollama", false).unwrap();
    service.set_permission("ollama", true).unwrap();
    finish.notify_one();
    let snapshot = task.await.unwrap();
    let a = snapshot.agents.iter().find(|a| a.id == "ollama").unwrap();
    assert!(a.models.is_empty());
    assert!(a.updated_at.is_none());
}

#[test]
fn endpoint_allowlist_rejects_generation_redirect_targets_and_remote_hosts() {
    for endpoint in Endpoint::all() {
        assert!(Endpoint::validate("GET", endpoint.url()).is_ok());
    }
    for url in [
        "http://127.0.0.1:11434/api/generate",
        "http://example.com/api/tags",
        "http://localhost:11434/api/tags",
        "http://127.0.0.1:11434/api/tags?x=1",
        "http://127.0.0.1:1234/v1/chat/completions",
    ] {
        assert!(Endpoint::validate("GET", url).is_err());
    }
    assert!(Endpoint::validate("POST", Endpoint::OllamaTags.url()).is_err());
}

#[test]
fn inventories_reject_malformed_data_and_do_not_invent_loaded_state() {
    assert!(Observation::ollama(json!({}), None).is_err());
    assert!(Observation::ollama(json!({"models":[{"name":""}]}), None).is_err());
    let o = Observation::ollama(json!({"models":[{"name":"a","size":-2}]}), None).unwrap();
    assert_eq!(o.models[0].loaded, None);
    assert_eq!(o.models[0].size_bytes, None);
    let o = Observation::lmstudio(json!({"data":[{"id":"a","state":"loaded"},{"id":"b","state":"not-loaded"},{"id":"c","state":"future"}]})).unwrap();
    assert_eq!(
        o.models.iter().map(|m| m.loaded).collect::<Vec<_>>(),
        vec![Some(true), Some(false), None]
    );
}

#[test]
fn detection_candidates_are_narrow_and_platform_specific() {
    use ai_usage_monitor::installation_candidates;
    use std::path::{Path, PathBuf};
    let paths = vec![PathBuf::from("/approved/bin")];
    let linux = installation_candidates("claude", false, &paths, Some(Path::new("/user")), None);
    assert!(linux.contains(&PathBuf::from("/approved/bin/claude")));
    assert!(linux.contains(&PathBuf::from("/user/.local/bin/claude")));
    assert!(!linux
        .iter()
        .any(|p| p.to_string_lossy().contains("credentials")));
    let windows = installation_candidates("ollama", true, &[], None, Some(Path::new("/local")));
    assert_eq!(
        windows,
        vec![PathBuf::from("/local/Programs/Ollama/ollama.exe")]
    );
    assert!(installation_candidates("unknown", false, &paths, None, None).is_empty());
}

#[tokio::test]
async fn unsupported_cloud_accounting_never_fabricates_usage_quota_or_spend() {
    // Use fixture status only: tests must never access the developer's Codex sign-in.
    let service = Service::new(Found, Fixture);
    service.scan_agents();
    service.set_permission("codex", true).unwrap();
    let snapshot = service.refresh_usage().await;
    let codex = snapshot.agents.iter().find(|a| a.id == "codex").unwrap();
    assert_eq!(codex.status, "unavailable");
    assert!(!codex.connected);
    assert!(codex.usage.is_none());
    assert!(codex.quotas.is_empty());
    assert!(codex.updated_at.is_none());
    assert!(codex
        .message
        .contains("Supported quota access is unavailable"));
}

#[tokio::test]
async fn codex_quota_observations_publish_without_inventing_running_state_or_tokens() {
    struct QuotaFixture;
    impl StatusReader for QuotaFixture {
        fn read(
            &self,
            _: String,
        ) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
            Box::pin(async {
                Ok(Observation {
                    models: vec![],
                    quotas: ai_usage_monitor::codex::parse_rate_limits(
                        &json!({"rateLimits":{"primary":{"usedPercent":20}}}),
                    )?,
                    ..Default::default()
                })
            })
        }
    }
    let service = Service::new(Found, QuotaFixture);
    service.scan_agents();
    service.set_permission("codex", true).unwrap();
    let snapshot = service.refresh_usage().await;
    let codex = snapshot.agents.iter().find(|a| a.id == "codex").unwrap();
    assert!(codex.connected);
    assert_eq!(codex.quotas[0].remaining_percent, Some(80.0));
    assert!(codex.running.is_none());
    assert!(codex.usage.is_none());
}

#[test]
fn local_inventory_cannot_be_mistaken_for_account_metrics() {
    let observation = Observation::ollama(
        json!({
            "models": [], "remainingPercent": 150,
            "resetsAt": "invalid timestamp", "spendUsd": -42,
            "usage": {"scope":"account", "totalTokens": 999}
        }),
        None,
    )
    .unwrap();
    // Only documented model inventory is accepted; quota/cost fields are not a source.
    assert!(observation.models.is_empty());
}

#[tokio::test]
async fn overlapping_refreshes_are_serialized_and_cached() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Count(Arc<AtomicUsize>);
    impl StatusReader for Count {
        fn read(
            &self,
            _: String,
        ) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Observation::ollama(json!({"models":[]}), None) })
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let service = Service::new(Found, Count(count.clone()));
    service.scan_agents();
    service.set_permission("ollama", true).unwrap();
    let (a, b) = tokio::join!(service.refresh_usage(), service.refresh_usage());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        serde_json::to_value(a).unwrap(),
        serde_json::to_value(b).unwrap()
    );
}
