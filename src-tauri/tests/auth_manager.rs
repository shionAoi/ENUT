#[cfg(unix)]
#[tokio::test]
async fn native_attempts_are_owned_cancelled_and_never_confused_with_verified_login() {
    use ai_usage_monitor::auth::AuthManager;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::OnceLock, time::Duration};
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    fn resolve(_: &str) -> Option<PathBuf> {
        BIN.get().cloned()
    }
    let root = std::env::temp_dir().join(format!("auth-manager-fixture-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let bin = root.join("fixture");
    fs::write(&bin,"#!/bin/sh\nprintf 'Authorization code: '\nread code\nif [ \"$code\" = fixture-code ]; then exit 0; else exit 1; fi\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    BIN.set(bin).unwrap();
    let manager = AuthManager::with_resolver(resolve);
    let choices = manager.methods("claude").await.unwrap();
    assert!(manager.start("claude", "forged").await.is_err());
    let cancelled = manager.start("claude", &choices[0].id).await.unwrap();
    manager.cancel(Some(&cancelled.attempt_id)).await;
    assert!(manager.active_tool().await.is_none());
    assert!(manager.poll(&cancelled.attempt_id).await.is_err());
    let view = manager.start("claude", &choices[0].id).await.unwrap();
    for _ in 0..100 {
        let p = manager.poll(&view.attempt_id).await.unwrap();
        if p.progress.state == "waiting" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // A stale cancel cannot cancel a replacement attempt.
    manager.cancel(Some(&cancelled.attempt_id)).await;
    assert_eq!(
        manager.active_id().await.as_deref(),
        Some(view.attempt_id.as_str())
    );
    manager
        .submit(&view.attempt_id, "fixture-code")
        .await
        .unwrap();
    let mut finished = false;
    for _ in 0..100 {
        let p = manager.poll(&view.attempt_id).await.unwrap();
        if p.progress.state == "finished" {
            assert!(p.progress.output.is_empty() && p.progress.user_code.is_none());
            finished = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        finished,
        "native command completion should be reported, not inferred authentication"
    );
    manager.cancel(Some(&view.attempt_id)).await;
    println!("Retained auth fixture: {}", root.display());
}
