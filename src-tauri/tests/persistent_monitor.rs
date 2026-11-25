use ai_usage_monitor::{
    usage::{aggregate, TokenRecord},
    Discovery, Observation, Service, StatusReader,
};
use chrono::{Duration, Utc};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Found;
impl Discovery for Found {
    fn installed(&self, _: &str) -> bool {
        true
    }
}
struct Reader(bool);
impl StatusReader for Reader {
    fn read(
        &self,
        _: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
    {
        let online = self.0;
        Box::pin(async move {
            if !online {
                return Err("Offline".into());
            }
            let at = Utc::now();
            let summary = aggregate(
                vec![TokenRecord {
                    id: "one".into(),
                    timestamp: at - Duration::days(1),
                    model: "test-model".into(),
                    input_tokens: 10,
                    output_tokens: 5,
                    cached_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    total_tokens: 15,
                }],
                at,
                "Synthetic records",
            )?;
            Ok(Observation {
                usage: Some(summary.usage),
                history: summary.history,
                model_usage: summary.model_usage,
                model_history: summary.model_history,
                context: Some(ai_usage_monitor::ContextInfo {
                    skills: vec!["private-skill".into()],
                    ..Default::default()
                }),
                authentication: Some("available".into()),
                ..Default::default()
            })
        })
    }
}
fn root() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "persistent-monitor-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
fn database(path: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path.join("monitor-usage.sqlite3")).unwrap()
}
fn on_disk(path: &std::path::Path, id: &str) -> bool {
    database(path)
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_tools WHERE tool=?1)",
            [id],
            |r| r.get(0),
        )
        .unwrap()
}
fn legacy_seed() -> serde_json::Value {
    serde_json::json!({"version":1,"tools":{"claude":{
        "allowed":true,"usage":null,"models":[],"model_history":[],"partial":null,"updated_at":null,
        "history":[{"date":Utc::now().date_naive().to_string(),"inputTokens":10,"outputTokens":5,"cachedTokens":0,"totalTokens":15}]
    }}})
}
fn write_legacy(path: &std::path::Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}
#[test]
fn expired_input_only_history_is_cleared_on_restart_without_revoking_the_tool() {
    let path = root();
    let file = path.join("monitor-state.json");
    let day = |age| {
        format!(
            "{}T00:00:00Z",
            Utc::now().date_naive() - Duration::days(age)
        )
    };
    std::fs::write(&file, serde_json::json!({
        "version":1,"tools":{"claude":{"allowed":true,"usage":null,"history":[],"models":[],"model_history":[],
        "partial":{"inputTokens":507,"cachedTokens":12,"source":"Claude ordinary transcripts","note":"Partial input only",
        "asOf":day(100),"periodStart":day(129),"periodEnd":day(100)},"updated_at":day(100)}}
    }).to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let service = Service::with_storage(Found, Reader(false), &path);
    let snapshot = service.scan_agents();
    let agent = snapshot.agents.iter().find(|a| a.id == "claude").unwrap();
    assert!(agent.monitoring_allowed);
    assert!(agent.partial_usage.is_none() && !agent.history_cached);
    let partial: Option<String> = database(&path)
        .query_row(
            "SELECT partial_json FROM usage_tools WHERE tool='claude'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(partial.is_none());
    assert!(!file.try_exists().unwrap());
}
#[test]
fn legacy_history_retains_the_ninetieth_utc_day_but_not_older_rows() {
    let path = root();
    let state = path.join("monitor-state.json");
    let mut json = legacy_seed();
    let today = Utc::now().date_naive();
    let tool = &mut json["tools"]["claude"];
    let mut legacy = tool["history"][0].clone();
    legacy.as_object_mut().unwrap().remove("recordedResponses");
    let mut older = legacy.clone();
    legacy["date"] = serde_json::json!((today - Duration::days(89)).to_string());
    older["date"] = serde_json::json!((today - Duration::days(90)).to_string());
    tool["history"] = serde_json::json!([legacy, older]);
    tool["model_history"] = serde_json::json!([]);
    write_legacy(&state, &serde_json::to_vec(&json).unwrap());
    let service = Service::with_storage(Found, Reader(false), &path);
    let snapshot = service.scan_agents();
    let agent = snapshot.agents.iter().find(|a| a.id == "claude").unwrap();
    assert!(agent.monitoring_allowed && agent.history_cached);
    assert_eq!(agent.usage_history.len(), 1);
    assert_eq!(
        agent.usage_history[0].date,
        (today - Duration::days(89)).to_string()
    );
    assert!(serde_json::to_value(&agent.usage_history[0]).unwrap()["recordedResponses"].is_null());
    let date: String = database(&path)
        .query_row(
            "SELECT date FROM usage_daily WHERE tool='claude'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(date, (today - Duration::days(89)).to_string());
}
#[tokio::test]
async fn gemini_cli_history_requires_its_own_permission_and_is_removed_by_revoke() {
    let path = root();
    let service = Service::with_storage(Found, Reader(true), &path);
    let snapshot = service.scan_agents();
    assert!(snapshot
        .agents
        .iter()
        .any(|a| a.id == "gemini" && a.name == "Gemini CLI"));
    service.set_permission("antigravity", true).unwrap();
    let snapshot = service.refresh_usage().await;
    let gemini = snapshot.agents.iter().find(|a| a.id == "gemini").unwrap();
    assert!(!gemini.monitoring_allowed && gemini.usage_history.is_empty());
    service.set_permission("gemini", true).unwrap();
    let snapshot = service.refresh_usage().await;
    let gemini = snapshot.agents.iter().find(|a| a.id == "gemini").unwrap();
    assert!(gemini.monitoring_allowed && !gemini.usage_history.is_empty());
    let snapshot = service.set_permission("gemini", false).unwrap();
    let gemini = snapshot.agents.iter().find(|a| a.id == "gemini").unwrap();
    assert!(!gemini.monitoring_allowed && gemini.usage_history.is_empty());
    assert!(snapshot
        .agents
        .iter()
        .any(|a| a.id == "antigravity" && a.monitoring_allowed));
    assert!(!on_disk(&path, "gemini"));
}
#[tokio::test]
async fn history_and_approval_survive_restart_without_live_evidence_or_double_counting() {
    let path = root();
    let service = Service::with_storage(Found, Reader(true), &path);
    service.scan_agents();
    service.set_permission("claude", true).unwrap();
    let first = service.refresh_usage().await;
    let original = first.agents.iter().find(|a| a.id == "claude").unwrap();
    let timestamp = original.updated_at.clone();
    drop(service);
    let service = Service::with_storage(Found, Reader(false), &path);
    let scan = service.scan_agents();
    let restored = scan.agents.iter().find(|a| a.id == "claude").unwrap();
    assert!(restored.monitoring_allowed && restored.history_cached);
    assert!(!restored.connected);
    assert_eq!(restored.usage.as_ref().unwrap().total_tokens, 15);
    assert_eq!(restored.updated_at, timestamp);
    assert!(
        restored.context.is_none()
            && restored.authentication.is_none()
            && restored.active_sessions.is_none()
    );
    assert!(restored.quotas.is_empty() && restored.models.is_empty());
    let offline = service.refresh_usage().await;
    assert_eq!(
        offline
            .agents
            .iter()
            .find(|a| a.id == "claude")
            .unwrap()
            .model_history[0]
            .total_tokens,
        15
    );
    drop(service);
    let service = Service::with_storage(Found, Reader(true), &path);
    service.scan_agents();
    let fresh = service.refresh_usage().await;
    assert_eq!(
        fresh
            .agents
            .iter()
            .find(|a| a.id == "claude")
            .unwrap()
            .usage_history[0]
            .total_tokens,
        15
    );
    let store = database(&path);
    let file: String = store.query_row("SELECT COALESCE(period_json,'') || COALESCE(partial_json,'') || models_json FROM usage_tools WHERE tool='claude'", [], |r| r.get(0)).unwrap();
    assert!(
        !file.contains("private-skill")
            && !file.contains("authentication")
            && !file.contains("activeSessions")
    );
}
#[tokio::test]
async fn revoke_deletes_only_selected_history_and_approval_after_restart() {
    let path = root();
    let service = Service::with_storage(Found, Reader(true), &path);
    service.scan_agents();
    service.set_permission("claude", true).unwrap();
    service.set_permission("opencode", true).unwrap();
    service.refresh_usage().await;
    service.set_permission("claude", false).unwrap();
    drop(service);
    let service = Service::with_storage(Found, Reader(false), &path);
    let scan = service.scan_agents();
    let revoked = scan.agents.iter().find(|a| a.id == "claude").unwrap();
    assert!(
        !revoked.monitoring_allowed && revoked.usage.is_none() && revoked.model_history.is_empty()
    );
    assert!(
        scan.agents
            .iter()
            .find(|a| a.id == "opencode")
            .unwrap()
            .monitoring_allowed
    );
    assert!(!on_disk(&path, "claude"));
}
#[tokio::test]
async fn authentication_pause_preserves_history_but_never_silently_regrants() {
    let path = root();
    let service = Service::with_storage(Found, Reader(true), &path);
    service.scan_agents();
    service.set_permission("claude", true).unwrap();
    service.refresh_usage().await;
    service.begin_auth("claude").unwrap();
    service.end_auth("claude");
    drop(service);
    let service = Service::with_storage(Found, Reader(false), &path);
    assert!(
        !service
            .scan_agents()
            .agents
            .iter()
            .find(|a| a.id == "claude")
            .unwrap()
            .monitoring_allowed
    );
    let allowed = service.set_permission("claude", true).unwrap();
    assert_eq!(
        allowed
            .agents
            .iter()
            .find(|a| a.id == "claude")
            .unwrap()
            .usage
            .as_ref()
            .unwrap()
            .total_tokens,
        15
    );
}
#[test]
fn corrupted_storage_fails_closed_without_overwriting_it() {
    let path = root();
    let bytes = b"{not valid json";
    std::fs::write(path.join("monitor-state.json"), bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path.join("monitor-state.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let service = Service::with_storage(Found, Reader(false), &path);
    assert!(service
        .scan_agents()
        .warnings
        .iter()
        .any(|w| w.contains("storage")));
    assert!(service.set_permission("claude", true).is_err());
    assert!(service.set_permission("claude", false).is_err());
    assert_eq!(
        std::fs::read(path.join("monitor-state.json")).unwrap(),
        bytes
    );
}
#[test]
fn every_tool_approval_is_remembered_independently_of_status_success() {
    let path = root();
    let service = Service::with_storage(Found, Reader(false), &path);
    let tools = service.scan_agents().agents;
    assert_eq!(tools.len(), 8);
    for agent in &tools {
        service.set_permission(&agent.id, true).unwrap();
    }
    drop(service);
    let service = Service::with_storage(Found, Reader(false), &path);
    assert!(service
        .scan_agents()
        .agents
        .iter()
        .all(|a| a.monitoring_allowed && !a.connected));
}
#[tokio::test]
async fn failed_revoke_stops_access_and_reports_unconfirmed_disk_deletion() {
    let path = root();
    let service = Service::with_storage(Found, Reader(true), &path);
    service.scan_agents();
    service.set_permission("claude", true).unwrap();
    service.refresh_usage().await;
    let blocked = database(&path);
    blocked.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(service.set_permission("claude", false).is_err());
    let snapshot = service.refresh_usage().await;
    let agent = snapshot.agents.iter().find(|a| a.id == "claude").unwrap();
    assert!(!agent.monitoring_allowed && !agent.connected && agent.usage.is_none());
    assert!(agent.message.contains("storage") && !agent.message.contains("deleted"));
    assert!(snapshot.warnings.iter().any(|w| w.contains("storage")));
    assert!(on_disk(&path, "claude"));
}
#[test]
fn second_instance_cannot_overwrite_another_instances_permissions() {
    let path = root();
    let first = Service::with_storage(Found, Reader(false), &path);
    first.scan_agents();
    first.set_permission("claude", true).unwrap();
    let files = [
        path.join("monitor-usage.sqlite3"),
        path.join("monitor-usage.sqlite3-wal"),
    ];
    let before = files.each_ref().map(|p| std::fs::read(p).unwrap());
    let second = Service::with_storage(Found, Reader(false), &path);
    assert!(second.set_permission("claude", false).is_err());
    assert_eq!(files.each_ref().map(|p| std::fs::read(p).unwrap()), before);
}
#[cfg(unix)]
#[test]
fn linked_state_or_directory_never_reads_or_modifies_the_target() {
    use std::os::unix::fs::symlink;
    let path = root();
    let outside = root();
    let target = outside.join("secret");
    std::fs::write(&target, b"do not touch").unwrap();
    symlink(&target, path.join("monitor-state.json")).unwrap();
    let service = Service::with_storage(Found, Reader(false), &path);
    assert!(service.set_permission("claude", true).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"do not touch");
    let linked = outside.join("linked");
    symlink(&path, &linked).unwrap();
    let service = Service::with_storage(Found, Reader(false), &linked);
    assert!(service.set_permission("claude", true).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"do not touch");
}
#[tokio::test]
async fn revoked_inflight_observation_cannot_recreate_saved_data() {
    struct Waiting(std::sync::Arc<tokio::sync::Notify>);
    impl StatusReader for Waiting {
        fn read(
            &self,
            _: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
        {
            let started = self.0.clone();
            Box::pin(async move {
                started.notify_one();
                std::future::pending().await
            })
        }
    }
    let path = root();
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let service = std::sync::Arc::new(Service::with_storage(
        Found,
        Waiting(started.clone()),
        &path,
    ));
    service.scan_agents();
    service.set_permission("claude", true).unwrap();
    let copy = service.clone();
    let task = tokio::spawn(async move { copy.refresh_usage().await });
    started.notified().await;
    service.set_permission("claude", false).unwrap();
    let done = task.await.unwrap();
    assert!(
        !done
            .agents
            .iter()
            .find(|a| a.id == "claude")
            .unwrap()
            .monitoring_allowed
    );
    assert!(!on_disk(&path, "claude"));
}
struct Bucket {
    days_ago: i64,
    tokens: u64,
    dated: bool,
}
#[tokio::test]
async fn model_only_daily_history_survives_publication_and_restart_without_inventing_totals() {
    struct ModelsOnly;
    impl StatusReader for ModelsOnly {
        fn read(
            &self,
            _: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
        {
            Box::pin(async {
                Ok(Observation {
                    model_history: vec![ai_usage_monitor::ModelDayUsage {
                        input_tokens: None,
                        output_tokens: None,
                        cached_tokens: None,
                        cache_creation_tokens: None,
                        cache_read_tokens: None,
                        date: Utc::now().date_naive().to_string(),
                        model: "only-model".into(),
                        total_tokens: 42,
                    }],
                    ..Default::default()
                })
            })
        }
    }
    let path = root();
    let service = Service::with_storage(Found, ModelsOnly, &path);
    service.scan_agents();
    service.set_permission("claude", true).unwrap();
    let observed = service.refresh_usage().await;
    let agent = observed.agents.iter().find(|a| a.id == "claude").unwrap();
    assert_eq!(agent.model_history.len(), 1);
    assert!(agent.usage.is_none() && agent.usage_history.is_empty());
    drop(service);
    let restored = Service::with_storage(Found, Reader(false), &path).scan_agents();
    let agent = restored.agents.iter().find(|a| a.id == "claude").unwrap();
    assert_eq!(agent.model_history[0].total_tokens, 42);
    assert!(agent.history_cached && agent.usage.is_none() && agent.usage_history.is_empty());
}
impl StatusReader for Bucket {
    fn read(
        &self,
        _: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
    {
        let (days_ago, tokens, dated) = (self.days_ago, self.tokens, self.dated);
        Box::pin(async move {
            let at = Utc::now();
            let summary = aggregate(
                vec![TokenRecord {
                    id: "one".into(),
                    timestamp: at - Duration::days(days_ago),
                    model: "test-model".into(),
                    input_tokens: tokens,
                    output_tokens: 0,
                    cached_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    total_tokens: tokens,
                }],
                at,
                "Synthetic records",
            )?;
            Ok(Observation {
                usage: Some(summary.usage),
                model_usage: summary.model_usage,
                history: if dated { summary.history } else { vec![] },
                model_history: if dated { summary.model_history } else { vec![] },
                ..Default::default()
            })
        })
    }
}
#[tokio::test]
async fn two_dates_survive_source_loss_and_same_day_repoll_replaces_instead_of_adding() {
    let path = root();
    for (days_ago, tokens) in [(1, 15), (0, 7), (0, 9), (0, 9)] {
        let service = Service::with_storage(
            Found,
            Bucket {
                days_ago,
                tokens,
                dated: true,
            },
            &path,
        );
        service.scan_agents();
        service.set_permission("claude", true).unwrap();
        service.refresh_usage().await;
    }
    let service = Service::with_storage(Found, Reader(false), &path);
    let scan = service.scan_agents();
    let a = scan.agents.iter().find(|a| a.id == "claude").unwrap();
    assert_eq!(a.usage_history.len(), 2);
    assert_eq!(a.model_history.len(), 2);
    assert_eq!(a.usage.as_ref().unwrap().total_tokens, 24);
    assert_eq!(a.model_usage[0].total_tokens, 24);
    assert_eq!(a.usage_history[0].total_tokens, 15);
    assert_eq!(a.usage_history[1].total_tokens, 9);
}
#[tokio::test]
async fn undated_source_period_is_saved_without_inventing_daily_history() {
    let path = root();
    let service = Service::with_storage(
        Found,
        Bucket {
            days_ago: 1,
            tokens: 20,
            dated: false,
        },
        &path,
    );
    service.scan_agents();
    service.set_permission("opencode", true).unwrap();
    let fresh = service.refresh_usage().await;
    let opencode = fresh.agents.iter().find(|a| a.id == "opencode").unwrap();
    assert_eq!(
        serde_json::to_value(opencode).unwrap()["reportedPeriodUsage"]["totalTokens"],
        20
    );
    let original = fresh
        .agents
        .iter()
        .find(|a| a.id == "opencode")
        .unwrap()
        .usage
        .clone()
        .unwrap();
    drop(service);
    struct ContextOnly;
    impl StatusReader for ContextOnly {
        fn read(
            &self,
            _: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
        {
            Box::pin(async {
                Ok(Observation {
                    context: Some(Default::default()),
                    ..Default::default()
                })
            })
        }
    }
    let service = Service::with_storage(Found, ContextOnly, &path);
    let restored = service.scan_agents();
    let a = restored.agents.iter().find(|a| a.id == "opencode").unwrap();
    assert!(a.model_history.is_empty() && a.usage_history.is_empty());
    assert_eq!(a.model_usage[0].total_tokens, 20);
    assert_eq!(
        a.usage.as_ref().unwrap().period_start,
        original.period_start
    );
    assert_eq!(a.usage.as_ref().unwrap().as_of, original.as_of);
    assert!(serde_json::to_value(a).unwrap()["reportedPeriodUsage"].is_null());
    assert_eq!(a.saved_period_usage.as_ref().unwrap().total_tokens, 20);
    let context_only = service.refresh_usage().await;
    let a = context_only
        .agents
        .iter()
        .find(|a| a.id == "opencode")
        .unwrap();
    assert!(a.connected && a.context.is_some() && a.usage.is_some());
    assert!(serde_json::to_value(a).unwrap()["reportedPeriodUsage"].is_null());
    assert_eq!(a.saved_period_usage.as_ref().unwrap().total_tokens, 20);
}
#[tokio::test]
async fn oversized_unknown_version_and_inconsistent_counts_are_rejected_without_writes() {
    let original = legacy_seed();
    for case in ["oversized", "version", "counts", "tool"] {
        let path = root();
        let mut value = original.clone();
        match case {
            "version" => value["version"] = serde_json::json!(99),
            "counts" => {
                value["tools"]["claude"]["history"][0]["totalTokens"] =
                    serde_json::json!(9007199254740992u64)
            }
            "tool" => {
                value["tools"]["unknown"] = value["tools"]["claude"].clone();
            }
            _ => (),
        }
        let bytes = if case == "oversized" {
            vec![b' '; 4 * 1024 * 1024 + 1]
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        let target = path.join("monitor-state.json");
        std::fs::write(&target, &bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let service = Service::with_storage(Found, Reader(false), &path);
        assert!(service.set_permission("claude", true).is_err(), "{case}");
        assert_eq!(std::fs::read(&target).unwrap(), bytes, "{case}");
    }
}
