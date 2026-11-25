use ai_usage_monitor::billing::{parse_costs, report_url};
use serde_json::json;

#[test]
fn reporting_urls_are_fixed_and_never_accept_generation_or_custom_hosts() {
    assert_eq!(
        report_url("openai").unwrap(),
        "https://api.openai.com/v1/organization/costs"
    );
    assert_eq!(
        report_url("anthropic").unwrap(),
        "https://api.anthropic.com/v1/organizations/cost_report"
    );
    assert!(report_url("http://127.0.0.1").is_err());
    assert!(report_url("messages").is_err());
}

#[test]
fn provider_units_and_empty_reports_are_not_confused() {
    assert_eq!(parse_costs("openai", &json!({"data":[{"results":[{"amount":{"currency":"usd","value":12.5}}]}],"has_more":false})).unwrap(), 12.5);
    assert_eq!(
        parse_costs(
            "anthropic",
            &json!({"data":[{"results":[{"currency":"USD","amount":"1250.5"}]}],"has_more":false})
        )
        .unwrap(),
        12.505
    );
    assert_eq!(
        parse_costs("openai", &json!({"data":[],"has_more":false})).unwrap(),
        0.0
    );
}

#[test]
fn partial_malformed_or_foreign_currency_reports_are_unavailable() {
    for response in [
        json!({"data":[],"has_more":true}),
        json!({"data":[{"results":[{"amount":{"currency":"eur","value":12}}]}],"has_more":false}),
        json!({"data":[{"results":[{}]}],"has_more":false}),
        json!({"error":"private error with key"}),
    ] {
        assert!(parse_costs("openai", &response).is_err());
    }
}

#[tokio::test]
async fn explicit_reporting_permission_and_revocation_gate_inflight_results() {
    use ai_usage_monitor::billing::{BillingReport, BillingStore, ReportFuture, ReportReader};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;
    struct Fixture {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        calls: Arc<AtomicUsize>,
    }
    impl ReportReader for Fixture {
        fn read(&self, provider: String, _: zeroize::Zeroizing<String>) -> ReportFuture {
            let entered = self.entered.clone();
            let release = self.release.clone();
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(BillingReport {
                    provider,
                    spend_usd: Some(10.0),
                    period_start: String::new(),
                    period_end: String::new(),
                    as_of: None,
                    message: String::new(),
                })
            })
        }
    }
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let store = Arc::new(BillingStore::new(Fixture {
        entered: entered.clone(),
        release: release.clone(),
        calls: calls.clone(),
    }));
    store.refresh().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    store
        .connect("openai", "fixture-key-not-a-secret".into())
        .unwrap();
    let running = {
        let store = store.clone();
        tokio::spawn(async move { store.refresh().await })
    };
    entered.notified().await;
    store.disconnect("openai").unwrap();
    release.notify_one();
    running.await.unwrap();
    assert!(store.reports().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
