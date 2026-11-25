#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use ai_usage_monitor::{Discovery, Observation, Service, StatusReader};
use chrono::{Days, Utc};
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicI64, Ordering::Relaxed},
};

static UTC_SHIFT: AtomicI64 = AtomicI64::new(0);
// Only this test binary's realtime clock is shifted; timers stay monotonic.
#[unsafe(no_mangle)]
unsafe extern "C" fn clock_gettime(
    clock: libc::clockid_t,
    out: *mut libc::timespec,
) -> libc::c_int {
    let rc = unsafe { libc::syscall(libc::SYS_clock_gettime, clock, out) } as libc::c_int;
    if rc == 0 && clock == libc::CLOCK_REALTIME {
        unsafe { (*out).tv_sec += UTC_SHIFT.load(Relaxed) };
    }
    rc
}

struct Found;
impl Discovery for Found {
    fn installed(&self, id: &str) -> bool {
        id == "claude"
    }
}
struct Offline;
impl StatusReader for Offline {
    fn read(
        &self,
        _: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Observation, String>> + Send>>
    {
        Box::pin(async { Err("Synthetic source unavailable.".into()) })
    }
}

#[tokio::test]
async fn expired_rows_are_removed_during_an_offline_refresh_after_utc_rollover() {
    let today = Utc::now().date_naive();
    let last = (today - Days::new(89)).to_string();
    let root =
        std::env::temp_dir().join(format!("usage-retention-rollover-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let file = root.join("monitor-state.json");
    fs::write(&file, json!({
        "version":1,"tools":{"claude":{"allowed":true,"usage":null,"models":[],"partial":null,"updated_at":null,
        "history":[{"date":last,"inputTokens":21,"outputTokens":5,"cachedTokens":0,"totalTokens":26}],
        "model_history":[{"date":last,"model":"legacy-model","totalTokens":26}]}}
    }).to_string()).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let service = Service::with_storage(Found, Offline, &root);
    assert_eq!(service.scan_agents().agents[0].usage_history.len(), 1);
    UTC_SHIFT.store(86400, Relaxed);
    assert_eq!(Utc::now().date_naive(), today + Days::new(1));
    let agent = service.refresh_usage().await.agents.remove(0);
    assert_eq!(agent.message, "Synthetic source unavailable.");
    let db = rusqlite::Connection::open(root.join("monitor-usage.sqlite3")).unwrap();
    let counts: (i64, i64) = db
        .query_row(
            "SELECT (SELECT count(*) FROM usage_daily), (SELECT count(*) FROM usage_model_daily)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(agent.usage_history.is_empty() && agent.model_history.is_empty());
    assert_eq!(counts, (0, 0));
}
