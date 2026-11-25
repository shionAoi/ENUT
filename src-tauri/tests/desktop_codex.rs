#![cfg(unix)]

use ai_usage_monitor::{codex, codex_auth::CodexSession, discovery::installed_path};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
fn scenario(mode: &str) {
    let root = std::env::temp_dir().join(format!(
        "desktop-codex-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let bin = root.join("home/.local/share/fnm/aliases/default/bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(root.join("desktop")).unwrap();
    fs::create_dir(root.join("tmp")).unwrap();
    executable(&bin.join("codex"), "#!/usr/bin/env node\n");
    executable(
        &bin.join("node"),
        r#"#!/usr/bin/python3 -I
import json,os,sys
root=os.environ['EZTYN_CODEX_TEST_ROOT']
native=root+'/home/.local/share/fnm/aliases/default/bin'
assert sys.argv[1:]==[native+'/codex','app-server']
paths=os.environ['PATH'].split(os.pathsep)
if os.environ['EZTYN_CODEX_TEST_MODE']=='terminal':
    assert paths==[root+'/desktop',native]
else:
    assert paths==[native,root+'/desktop']
for line in sys.stdin:
    q=json.loads(line)
    method=q['method']
    if method=='initialize': result={}
    elif method=='initialized': continue
    elif method=='account/read': result={'account':{'type':'chatgpt'}}
    elif method=='account/rateLimits/read': result={'rateLimits':{'primary':{'usedPercent':25}}}
    elif method=='account/login/start':
        assert q['params']=={'type':'chatgptDeviceCode'}
        result={'type':'chatgptDeviceCode','loginId':'fixture-login','verificationUrl':'https://auth.openai.com/codex/device','userCode':'TEST-ONLY'}
    elif method=='account/login/cancel': result={'status':'canceled'}
    else: raise AssertionError('Unexpected method')
    print(json.dumps({'id':q['id'],'result':result}),flush=True)
"#,
    );
    let path = if mode == "terminal" {
        std::env::join_paths([root.join("desktop"), bin]).unwrap()
    } else {
        root.join("desktop").into_os_string()
    };
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "desktop_codex_child", "--nocapture"])
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", path)
        .env("TMPDIR", root.join("tmp"))
        .env("EZTYN_CODEX_TEST_ROOT", &root)
        .env("EZTYN_CODEX_TEST_MODE", mode)
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn desktop_status_finds_fnm_codex_without_shell_path() {
    scenario("status");
}
#[test]
fn desktop_login_uses_matching_node_without_shell_path() {
    scenario("login");
}
#[test]
fn existing_terminal_path_order_and_both_codex_flows_are_preserved() {
    scenario("terminal");
}

#[tokio::test]
async fn desktop_codex_child() {
    let Some(root) = std::env::var_os("EZTYN_CODEX_TEST_ROOT").map(PathBuf::from) else {
        return;
    };
    let mode = std::env::var("EZTYN_CODEX_TEST_MODE").unwrap();
    let before = std::env::var_os("PATH");
    let expected = root.join("home/.local/share/fnm/aliases/default/bin/codex");
    if mode != "login" {
        assert_eq!(installed_path("codex"), Some(expected.clone()));
        let observation = codex::read().await.unwrap();
        assert_eq!(observation.quotas[0].remaining_percent, Some(75.0));
    }
    if mode != "status" {
        let mut session = CodexSession::start(&expected).await.unwrap();
        assert_eq!(session.view().state, "waiting");
        assert_eq!(session.view().kind, "device_code");
        session.cancel().await.unwrap();
    }
    assert_eq!(
        std::env::var_os("PATH"),
        before,
        "parent PATH must not change"
    );
}
