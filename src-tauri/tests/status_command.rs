use ai_usage_monitor::status_command::Operation;
#[test]
fn operation_allowlist_has_only_status_and_no_user_arguments() {
    assert_eq!(
        Operation::AntigravityUsage.args(),
        &["-p", "/usage", "--output-format", "json"]
    );
    assert_eq!(Operation::AntigravityVersion.args(), &["--version"]);
    assert_eq!(Operation::HermesUsage.args(), &["usage", "--json"]);
    assert_eq!(
        Operation::OpenCodeActiveSessions.args(),
        &["api", "get", "/api/session/active"]
    );
    assert_eq!(Operation::ClaudeAgents.args(), &["agents", "--json"]);
    assert_eq!(Operation::ClaudeAgents.tool(), "claude");
    for o in [
        Operation::OpenCodeStats,
        Operation::OpenCodeLimits,
        Operation::OpenCodeMcps,
        Operation::OpenCodeSkills,
    ] {
        assert_eq!(&o.args()[..2], &["api", "get"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn synthetic_process_has_null_stdin_neutral_cwd_redacted_errors_and_bounds() {
    use ai_usage_monitor::status_command::{CommandRunner, ProcessRunner, ERROR};
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::OnceLock};
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    fn resolve(_: &str) -> Option<PathBuf> {
        BIN.get().cloned()
    }
    let dir = std::env::temp_dir().join(format!("status-fixture-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let bin = dir.join("fixture");
    BIN.set(bin.clone()).unwrap();
    let write = |body: &str| {
        fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    };
    let r = ProcessRunner::with_resolver(resolve);
    write("if read value; then exit 1; fi\nprintf '%s\\n' \"$PWD\"\nprintf '%s\\n' \"$@\"\nprintf SECRET >&2");
    let b = r.run(Operation::HermesUsage).await.unwrap();
    let s = String::from_utf8(b).unwrap();
    let lines = s.lines().collect::<Vec<_>>();
    assert!(lines[0].starts_with(std::env::temp_dir().to_str().unwrap()));
    assert!(lines[0].contains("ai-status-"));
    assert_eq!(&lines[1..], &["usage", "--json"]);
    assert!(!std::path::Path::new(lines[0]).exists());
    // agy treats any character-device stdin (including /dev/null) as interactive.
    write("if [ -c /dev/stdin ]; then printf BROWSER_LOGIN; exit 2; fi\nif read value; then exit 3; fi\nprintf '%s\\n' \"$@\"");
    assert_eq!(
        r.run(Operation::AntigravityUsage).await.unwrap(),
        b"-p\n/usage\n--output-format\njson\n"
    );
    write("test \"$(ps -o sid= -p $$ | tr -d ' ')\" = \"$$\" || exit 1\nprintf detached");
    assert_eq!(
        r.run(Operation::AntigravityUsage).await.unwrap(),
        b"detached"
    );
    // Other status commands retain their existing null-input policy.
    write("test -c /dev/stdin || exit 1\nprintf unchanged");
    assert_eq!(r.run(Operation::HermesUsage).await.unwrap(), b"unchanged");
    write("printf SECRET; printf PRIVATE >&2; exit 2");
    assert_eq!(r.run(Operation::HermesUsage).await.unwrap_err(), ERROR);
    write(&format!("while :; do printf '{}'; done", "x".repeat(1024)));
    assert_eq!(r.run(Operation::HermesUsage).await.unwrap_err(), ERROR);
    let pidfile = dir.join("pid");
    write(&format!(
        "echo $$ > '{}'; exec /bin/sleep 60",
        pidfile.display()
    ));
    assert_eq!(r.run(Operation::HermesUsage).await.unwrap_err(), ERROR);
    let pid: i32 = fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "timed-out child must be reaped"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn antigravity_auth_required_stderr_is_detected_without_leaking_output_or_deadlocking() {
    use ai_usage_monitor::status_command::{CommandRunner, ProcessRunner, AUTH_REQUIRED, ERROR};
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::OnceLock};
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    fn resolve(_: &str) -> Option<PathBuf> {
        BIN.get().cloned()
    }
    let dir = std::env::temp_dir().join(format!("status-agy-auth-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let bin = dir.join("agy");
    BIN.set(bin.clone()).unwrap();
    let write = |body: &str| {
        fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    };
    let r = ProcessRunner::with_resolver(resolve);

    write("printf '{\"secret\":\"TOKEN\"}\\n'; printf 'Error: authentication required. Run agy to log in.\\n' >&2; exit 1");
    assert_eq!(
        r.run(Operation::AntigravityUsage).await.unwrap_err(),
        AUTH_REQUIRED
    );

    write("printf 'PRIVATE_ERR\\n' >&2; exit 2");
    assert_eq!(r.run(Operation::AntigravityUsage).await.unwrap_err(), ERROR);

    write("i=0; while [ $i -lt 128 ]; do printf '%01024d' 0 >&2; i=$((i+1)); done; printf ok");
    assert_eq!(r.run(Operation::AntigravityUsage).await.unwrap(), b"ok");

    fs::remove_dir_all(dir).unwrap();
}
