use ai_usage_monitor::{
    antigravity,
    status_command::{CommandFuture, CommandRunner, Operation},
};
use serde_json::json;
use std::sync::Mutex;
struct Fake {
    version: &'static str,
    calls: Mutex<Vec<Operation>>,
}
impl CommandRunner for Fake {
    fn run(&self, op: Operation) -> CommandFuture<'_> {
        self.calls.lock().unwrap().push(op);
        Box::pin(async move {
            Ok(if op == Operation::AntigravityVersion {
                self.version.as_bytes().to_vec()
            } else {
                fixture().to_string().into_bytes()
            })
        })
    }
}
fn fixture() -> serde_json::Value {
    json!({"usage":{"total_tokens":999},"response":"PRIVATE","command":{"data":{"groups":[{"name":"Pro","buckets":[{"id":"fast","name":"Fast","window":"weekly","remaining_fraction":0.375,"reset_time":"2026-11-10T00:00:00Z"}]}]}}})
}
#[tokio::test]
async fn version_gate_never_issues_usage_for_unknown_old_or_unstable_versions() {
    for version in [
        "1.1.10",
        "1.5.3-preview",
        "1.5.3+custom",
        "garbage",
        "01.5.3",
        "2.0.0\n1.5.3",
    ] {
        let f = Fake {
            version,
            calls: Mutex::new(vec![]),
        };
        assert!(antigravity::read_with(&f).await.is_err());
        assert_eq!(
            *f.calls.lock().unwrap(),
            vec![Operation::AntigravityVersion]
        );
    }
    for version in ["1.1.11", "1.5.3\n"] {
        let f = Fake {
            version,
            calls: Mutex::new(vec![]),
        };
        let o = antigravity::read_with(&f).await.unwrap();
        assert_eq!(o.quotas[0].remaining_percent, Some(37.5));
        assert!(o.usage.is_none() && o.history.is_empty() && o.model_history.is_empty());
        assert_eq!(
            *f.calls.lock().unwrap(),
            vec![Operation::AntigravityVersion, Operation::AntigravityUsage]
        );
    }
}
#[test]
fn observed_groups_only_and_strict_validation() {
    let o = antigravity::parse(&fixture()).unwrap();
    let q = &o.quotas[0];
    assert_eq!(q.label, "Pro · Fast · weekly");
    assert_eq!(q.resets_at.as_deref(), Some("2026-11-10T00:00:00+00:00"));
    for (key, value) in [
        ("remaining_fraction", json!(-0.1)),
        ("remaining_fraction", json!(1.1)),
        ("reset_time", json!("tomorrow")),
        ("id", json!("/private/key")),
        ("window", json!("bad\nlabel")),
    ] {
        let mut v = fixture();
        v["command"]["data"]["groups"][0]["buckets"][0][key] = value;
        assert!(antigravity::parse(&v).is_err(), "{key}");
    }
    assert!(antigravity::parse(&json!({"groups":[]})).is_err());
}

#[tokio::test]
async fn locked_keyring_skips_cli_invocation_to_avoid_timeout_fallback() {
    let f = Fake {
        version: "1.5.17",
        calls: Mutex::new(vec![]),
    };
    let err = antigravity::read_with_keyring(&f, antigravity::KeyringState::Locked)
        .await
        .err()
        .unwrap();
    assert_eq!(err, antigravity::KEYRING_LOCKED);
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn parse_keyring_locked_handles_busctl_and_gdbus_outputs() {
    assert_eq!(antigravity::parse_keyring_locked(b"b true\n"), Some(true));
    assert_eq!(antigravity::parse_keyring_locked(b"b false\n"), Some(false));
    assert_eq!(antigravity::parse_keyring_locked(b"(<true>,)\n"), Some(true));
    assert_eq!(
        antigravity::parse_keyring_locked(b"(<false>,)\n"),
        Some(false)
    );
    assert_eq!(antigravity::parse_keyring_locked(b"unexpected"), None);
}

#[tokio::test]
async fn explicit_auth_required_surfaces_login_without_quotas() {
    struct AuthRequired;
    impl CommandRunner for AuthRequired {
        fn run(&self, op: Operation) -> CommandFuture<'_> {
            Box::pin(async move {
                match op {
                    Operation::AntigravityVersion => Ok(b"1.5.17\n".to_vec()),
                    Operation::AntigravityUsage => {
                        Err(ai_usage_monitor::status_command::AUTH_REQUIRED.into())
                    }
                    _ => unreachable!(),
                }
            })
        }
    }
    let obs = antigravity::read_with(&AuthRequired).await.unwrap();
    assert_eq!(obs.authentication.as_deref(), Some("required"));
    assert!(obs.quotas.is_empty());
    assert_eq!(
        obs.message.as_deref(),
        Some(ai_usage_monitor::status_command::AUTH_REQUIRED)
    );
}
