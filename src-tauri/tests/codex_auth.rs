#![cfg(unix)]

use ai_usage_monitor::{auth_types::AuthProgress, codex_auth::CodexSession};
use serde_json::Value;
use std::{
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    executable: PathBuf,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "codex-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("home")).unwrap();
        let executable = root.join("fake-codex");
        let script = format!(
            "#!/usr/bin/python3 -I\nMODE = {mode:?}\nROOT = {:?}\n",
            root.to_str().unwrap()
        ) + r#"
import os, sys, json, time, stat
os.environ['HOME'] = ROOT + '/home'
assert sys.argv[1:] == ['app-server']
with open(ROOT + '/process', 'w') as f:
    json.dump({'pid': os.getpid(), 'cwd': os.getcwd(), 'mode': stat.S_IMODE(os.stat('.').st_mode)}, f)
def emit(v):
    print(json.dumps(v), flush=True)
def completed(login='attempt-1', success=True):
    return {'method':'account/login/completed','params':{'loginId':login,'success':success,'error':'PRIVATE-ERROR'}}
for line in sys.stdin:
    q = json.loads(line)
    with open(ROOT + '/requests', 'a') as f:
        f.write(json.dumps(q) + '\n')
    method = q['method']
    assert method in ['initialize','initialized','account/login/start','account/read','account/login/cancel']
    if method == 'initialize':
        assert q['params']['clientInfo']['name'] == 'ai_usage_monitor'
        emit({'id':q['id'],'result':{}})
    elif method == 'initialized':
        pass
    elif method == 'account/login/start':
        assert q['params'] == {'type':'chatgptDeviceCode'}
        if MODE == 'unsupported':
            emit({'id':q['id'],'error':{'message':'PRIVATE-ERROR'}})
            continue
        if MODE == 'malformed':
            print('PRIVATE-ERROR not-json', flush=True)
            continue
        if MODE == 'oversized':
            print('x' * (256 * 1024 + 1), flush=True)
            continue
        if MODE == 'total':
            for _ in range(6): emit({'method':'notice','params':{'text':'x' * 200000}})
            continue
        result = {'type':'chatgptDeviceCode','loginId':'attempt-1','verificationUrl':'https://auth.openai.com/codex/device','userCode':'ABCD-EFGH'}
        if MODE.startswith('url:'): result['verificationUrl'] = MODE[4:]
        if MODE == 'bad-code': result['userCode'] = 'ABC\nPRIVATE-ERROR'
        if MODE == 'long-code': result['userCode'] = 'x' * 129
        if MODE == 'bad-id': result['loginId'] = 'x' * 513
        if MODE in ['early', 'early-failure']:
            emit(completed('wrong'))
            emit(completed(success=MODE != 'early-failure'))
        emit({'id':q['id'],'result':result})
        if MODE in ['success','api-key','failure','partial']:
            if MODE == 'partial':
                raw = json.dumps(completed()) + '\n'
                sys.stdout.write(raw[:25]); sys.stdout.flush()
                time.sleep(0.3)
                sys.stdout.write(raw[25:]); sys.stdout.flush()
            else: emit(completed(success=MODE != 'failure'))
            emit(completed())
        elif MODE == 'wrong':
            emit(completed('not-our-attempt'))
            emit(completed('another-attempt', success='invalid-for-other-client'))
        elif MODE == 'disconnect':
            sys.exit(0)
    elif method == 'account/read':
        assert q['params'] == {'refreshToken':False}
        emit({'id':q['id'],'result':{'account':{'type':'apiKey' if MODE == 'api-key' else 'chatgpt','email':'PRIVATE-EMAIL','accessToken':'PRIVATE-TOKEN'},'requiresOpenaiAuth':True}})
    elif method == 'account/login/cancel':
        assert q['params'] == {'loginId':'attempt-1'}
        if MODE == 'cancel-timeout':
            time.sleep(60)
        if MODE == 'cancel-error': emit({'id':q['id'],'error':{'message':'PRIVATE-ERROR'}})
        else: emit({'id':q['id'],'result':{'status':'canceled'}})
"#;
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self { root, executable }
    }
    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join("requests"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
    fn process(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.root.join("process")).unwrap()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn safe(v: &AuthProgress) {
    let text = serde_json::to_string(v).unwrap();
    for forbidden in [
        "PRIVATE-",
        "accessToken",
        "loginId",
        "account/login",
        "attempt-1",
    ] {
        assert!(!text.contains(forbidden), "unsafe public progress");
    }
    assert!(v.output.is_empty());
    assert_eq!(v.kind, "device_code");
}
async fn terminal(s: &mut CodexSession) -> AuthProgress {
    for _ in 0..60 {
        let v = s.poll().await.unwrap();
        safe(&v);
        if v.state != "waiting" {
            return v;
        }
    }
    panic!("fixture did not complete");
}

#[tokio::test]
async fn authenticates_only_matching_native_completion_and_chatgpt_status() {
    fn is_send<T: Send>() {}
    is_send::<CodexSession>();
    for mode in ["success", "early"] {
        let f = Fixture::new(mode);
        let mut s = CodexSession::start(&f.executable).await.unwrap();
        safe(&s.view());
        let v = terminal(&mut s).await;
        assert_eq!(v.state, "authenticated");
        assert!(v.url.is_none() && v.user_code.is_none());
        let requests = f.requests();
        assert_eq!(
            requests
                .iter()
                .map(|q| q["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "initialize",
                "initialized",
                "account/login/start",
                "account/read"
            ]
        );
        drop(s);
        let process = f.process();
        assert_eq!(process["mode"], 0o700);
        assert!(!PathBuf::from(process["cwd"].as_str().unwrap()).exists());
    }
}

#[tokio::test]
async fn wrong_attempt_does_not_finish_or_read_account_and_cancel_is_not_logout() {
    for mode in ["wrong", "cancel-error", "cancel-timeout", "waiting"] {
        let f = Fixture::new(mode);
        let mut s = CodexSession::start(&f.executable).await.unwrap();
        let v = s.poll().await.unwrap();
        assert_eq!(v.state, "waiting");
        safe(&v);
        assert_eq!(
            v.url.as_deref(),
            Some("https://auth.openai.com/codex/device")
        );
        assert_eq!(v.user_code.as_deref(), Some("ABCD-EFGH"));
        assert!(v.message.contains("browser"));
        let before_cancel = std::time::Instant::now();
        let result = s.cancel().await;
        assert!(before_cancel.elapsed() < Duration::from_secs(2));
        if mode.starts_with("cancel-") {
            assert!(!result.unwrap_err().contains("PRIVATE-"));
        } else {
            result.unwrap();
        }
        let v = s.view();
        safe(&v);
        assert_eq!(v.state, "cancelled");
        assert!(v.url.is_none() && v.user_code.is_none());
        let requests = f.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[3]["method"], "account/login/cancel");
        assert_eq!(
            requests[3]["params"],
            serde_json::json!({"loginId":"attempt-1"})
        );
        let pid = f.process()["pid"].as_u64().unwrap();
        assert!(
            !PathBuf::from(format!("/proc/{pid}")).exists(),
            "cancel must reap child"
        );
    }
}

#[tokio::test]
async fn partial_frames_survive_poll_timeouts_without_leaking_credentials() {
    let f = Fixture::new("partial");
    let mut s = CodexSession::start(&f.executable).await.unwrap();
    for _ in 0..2 {
        let start = std::time::Instant::now();
        assert_eq!(s.poll().await.unwrap().state, "waiting");
        assert!(start.elapsed() < Duration::from_millis(180));
    }
    assert_eq!(terminal(&mut s).await.state, "authenticated");
}

#[tokio::test]
async fn failure_disconnect_and_api_key_status_never_authenticate() {
    for mode in ["failure", "early-failure", "disconnect", "api-key"] {
        let f = Fixture::new(mode);
        let mut s = CodexSession::start(&f.executable).await.unwrap();
        let v = terminal(&mut s).await;
        assert_eq!(v.state, "failed");
        assert!(v.url.is_none() && v.user_code.is_none());
        if mode != "api-key" {
            assert!(!f.requests().iter().any(|q| q["method"] == "account/read"));
        }
    }
}

#[tokio::test]
async fn unsupported_malformed_bounded_and_untrusted_challenges_fail_closed() {
    for mode in [
        "unsupported",
        "malformed",
        "oversized",
        "total",
        "bad-code",
        "long-code",
        "bad-id",
        "url:http://auth.openai.com/codex/device",
        "url:https://evil.example/codex/device",
        "url:https://auth.openai.com:443/codex/device",
        "url:https://user@auth.openai.com/codex/device",
        "url:https://auth.openai.com/other",
        "url:https://auth.openai.com/codex/device?token=PRIVATE-TOKEN",
        "url:https://auth.openai.com/codex/device#fragment",
    ] {
        let f = Fixture::new(mode);
        let error = CodexSession::start(&f.executable)
            .await
            .err()
            .expect("must reject challenge");
        assert!(!error.contains("PRIVATE-") && !error.contains("not-json"));
        assert_eq!(f.requests().len(), 3);
        let process = f.process();
        assert!(!PathBuf::from(format!("/proc/{}", process["pid"])).exists());
        assert!(!PathBuf::from(process["cwd"].as_str().unwrap()).exists());
    }
    assert!(CodexSession::start(std::path::Path::new("relative-codex"))
        .await
        .is_err());
}

#[tokio::test]
async fn drop_kills_owned_child_without_native_logout() {
    let f = Fixture::new("waiting");
    let s = CodexSession::start(&f.executable).await.unwrap();
    let pid = f.process()["pid"].as_u64().unwrap();
    drop(s);
    for _ in 0..40 {
        if !PathBuf::from(format!("/proc/{pid}")).exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("dropped child remains alive");
}
