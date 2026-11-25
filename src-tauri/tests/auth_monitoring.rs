use ai_usage_monitor::{Discovery, Observation, Service, StatusReader};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
struct Found;
impl Discovery for Found {
    fn installed(&self, id: &str) -> bool {
        id == "codex"
    }
}
struct Reader(Arc<AtomicUsize>);
impl StatusReader for Reader {
    fn read(&self, _: String) -> Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Observation::default()) })
    }
}
#[tokio::test]
async fn login_pauses_status_and_never_silently_regrants_monitoring() {
    let calls = Arc::new(AtomicUsize::new(0));
    let s = Service::new(Found, Reader(calls.clone()));
    s.scan_agents();
    s.set_permission("codex", true).unwrap();
    s.begin_auth("codex").unwrap();
    assert!(s.set_permission("codex", true).is_err());
    s.refresh_usage().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    s.end_auth("codex");
    s.refresh_usage().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    s.set_permission("codex", true).unwrap();
    s.refresh_usage().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(s.begin_auth("unknown").is_err());
}

#[test]
fn dropping_a_sign_in_reservation_does_not_clear_its_replacement() {
    let s = Service::new(Found, Reader(Arc::new(AtomicUsize::new(0))));
    s.scan_agents();
    s.set_permission("codex", true).unwrap();
    let superseded = s.prepare_auth("codex").unwrap();
    let current = s.prepare_auth("codex").unwrap();
    drop(superseded);
    assert_eq!(
        s.set_permission("codex", true).unwrap_err(),
        "Finish or cancel native sign-in before granting monitoring access."
    );
    drop(current);
    assert!(!s.with_snapshot(|snapshot| snapshot.agents[0].monitoring_allowed));
    assert!(s.set_permission("codex", true).unwrap().agents[0].monitoring_allowed);
}
