use ai_usage_monitor::{ActiveSession, Discovery, Observation, Service, SessionActivity};
struct D;
impl Discovery for D {
    fn installed(&self, id: &str) -> bool {
        id == "opencode"
    }
}
struct R;
impl ai_usage_monitor::StatusReader for R {
    fn read(
        &self,
        _: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
    {
        Box::pin(async {
            Ok(Observation {
                authentication: Some("available".into()),
                active_sessions: Some(SessionActivity {
                    sessions: vec![ActiveSession {
                        id: "fixture".into(),
                        state: "running".into(),
                    }],
                    source: "Native fixture".into(),
                    scope: "One service".into(),
                    as_of: "2026-11-11T12:00:00Z".into(),
                }),
                ..Default::default()
            })
        })
    }
}
#[tokio::test]
async fn active_evidence_is_permission_gated_and_cleared_on_revoke() {
    let service = Service::new(D, R);
    service.scan_agents();
    assert!(service.refresh_usage().await.agents[0]
        .active_sessions
        .is_none());
    service.set_permission("opencode", true).unwrap();
    let granted = service.refresh_usage().await;
    assert_eq!(
        granted.agents[0]
            .active_sessions
            .as_ref()
            .unwrap()
            .sessions
            .len(),
        1
    );
    assert_eq!(
        granted.agents[0].authentication.as_deref(),
        Some("available")
    );
    let revoked = service.set_permission("opencode", false).unwrap();
    assert!(revoked.agents[0].active_sessions.is_none());
    assert!(revoked.agents[0].authentication.is_none());
}

#[tokio::test]
async fn delayed_active_session_response_cannot_restore_revoked_evidence() {
    use std::sync::Arc;
    use tokio::sync::Notify;
    struct Delayed {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }
    impl ai_usage_monitor::StatusReader for Delayed {
        fn read(
            &self,
            id: String,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
        {
            let started = self.started.clone();
            let release = self.release.clone();
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                R.read(id).await
            })
        }
    }
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let service = Service::new(
        D,
        Delayed {
            started: started.clone(),
            release: release.clone(),
        },
    );
    service.scan_agents();
    service.set_permission("opencode", true).unwrap();
    let refresh = service.refresh_usage();
    let revoke = async {
        started.notified().await;
        service.set_permission("opencode", false).unwrap();
        release.notify_one();
    };
    let (snapshot, ()) = tokio::join!(refresh, revoke);
    assert!(snapshot.agents[0].active_sessions.is_none());
    assert!(snapshot.agents[0].authentication.is_none());
    assert!(!snapshot.agents[0].connected);
}
