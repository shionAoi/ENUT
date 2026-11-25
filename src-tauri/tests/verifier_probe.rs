#![cfg(unix)]
use ai_usage_monitor::{
    codex,
    codex_auth::CodexSession,
    discovery::{installation_candidates, installed_path},
};
use std::{fs, path::PathBuf, process::Command};
#[tokio::test]
async fn isolated_worker() {
    let Some(root) = std::env::var_os("VERIFY_ROOT").map(PathBuf::from) else {
        return;
    };
    let mode = std::env::var("VERIFY_MODE").unwrap();
    let path = std::env::var_os("PATH");
    let home = std::env::var_os("HOME");
    let expected = if mode == "priority" {
        root.join("explicit/codex")
    } else {
        root.join("home/.local/share/fnm/aliases/default/bin/codex")
    };
    if mode == "noexec" || mode == "broken" {
        assert_ne!(installed_path("codex"), Some(expected));
        return;
    }
    assert_eq!(installed_path("codex"), Some(expected.clone()));
    if mode == "missingnode" {
        assert_eq!(codex::read().await.err().unwrap(), "Codex quota unavailable. Check that the installed Codex version supports app-server and is signed in with ChatGPT. No model request was made.");
        assert_eq!(CodexSession::start(&expected).await.err().unwrap(), "Codex sign-in unavailable, unsupported, or exceeded safety limits. No model request was made.");
    } else {
        assert_eq!(
            codex::read().await.unwrap().quotas[0].remaining_percent,
            Some(58.0)
        );
        let mut session = CodexSession::start(&expected).await.unwrap();
        assert_eq!(session.view().state, "waiting");
        session.cancel().await.unwrap();
    }
    assert_eq!(std::env::var_os("PATH"), path);
    assert_eq!(std::env::var_os("HOME"), home);
}
#[test]
fn sibling_and_windows_candidates() {
    let home = PathBuf::from("/synthetic-home");
    let explicit = PathBuf::from("/explicit");
    for id in [
        "codex",
        "claude",
        "gemini",
        "antigravity",
        "opencode",
        "hermes",
        "ollama",
        "lmstudio",
    ] {
        for windows in [true, false] {
            let got = installation_candidates(id, windows, &[explicit.clone()], Some(&home), None);
            assert!(got[0].starts_with(&explicit));
            assert_eq!(
                got.iter().any(|p| p.to_string_lossy().contains("/fnm/")),
                id == "codex" && !windows
            );
        }
    }
    assert!(installation_candidates("unknown", false, &[], Some(&home), None).is_empty());
}
#[test]
fn external_scenarios() {
    if std::env::var_os("VERIFY_ROOT").is_some() {
        return;
    }
    let root = std::env::temp_dir().join(format!("independent-codex-{}", std::process::id()));
    let setup = Command::new("/usr/bin/python3")
        .args([
            "-I",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/desktop_runtime.py"
            ),
        ])
        .env("VERIFY_FIXTURES", &root)
        .status()
        .unwrap();
    assert!(setup.success());
    for mode in [
        "desktop",
        "terminal",
        "priority",
        "noexec",
        "broken",
        "missingnode",
    ] {
        let base = root.join(mode);
        let bin = base.join("home/.local/share/fnm/aliases/default/bin");
        let path = match mode {
            "terminal" => std::env::join_paths([base.join("explicit"), bin]).unwrap(),
            "missingnode" => base.join("empty").into_os_string(),
            _ => base.join("explicit").into_os_string(),
        };
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "isolated_worker", "--nocapture"])
            .env_clear()
            .env("HOME", base.join("home"))
            .env("TMPDIR", root.join("tmp"))
            .env("PATH", path)
            .env("CODEX_HOME", base.join("auth"))
            .env("VERIFY_ROOT", &base)
            .env("VERIFY_MODE", mode)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        println!("{mode}: exit={} verified", out.status.code().unwrap());
    }
    assert!(
        fs::metadata(root.join("desktop/home/.local/share/fnm/aliases/default"))
            .unwrap()
            .is_dir()
    );
    let check=Command::new("/usr/bin/python3").args(["-I","-c",r#"import pathlib,hashlib,json,sys
r=pathlib.Path(sys.argv[1]); before=json.loads((r/'before.json').read_text())
after={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in r.rglob('*') if p.is_file() and not p.is_symlink() and p.name not in ['rpc.log','before.json']}
assert before==after
assert not list(r.glob('*/profile-executed'))
expected=['initialize','initialized','account/read','account/rateLimits/read','initialize','initialized','account/login/start','account/login/cancel']
for mode in ['desktop','terminal','priority']: assert (r/mode/'rpc.log').read_text().splitlines()==expected
for mode in ['noexec','broken','missingnode']: assert not (r/mode/'rpc.log').exists()
print('SHA256 and exact RPC invariants passed')"#]).arg(&root).status().unwrap();
    assert!(check.success());
    println!("Retained fixture: {}", root.display());
}
