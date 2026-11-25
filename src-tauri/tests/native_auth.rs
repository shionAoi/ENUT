//! Synthetic login processes only. No installed provider or real home is accessed.
#![cfg(unix)]
use ai_usage_monitor::{auth_types::AuthProgress, native_auth::NativeSession};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

static LOCK: Mutex<()> = Mutex::new(());
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    path: PathBuf,
    home: Option<std::ffi::OsString>,
    _lock: MutexGuard<'static, ()>,
}
impl Fixture {
    fn new(body: &str) -> Self {
        let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "native-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = std::env::var_os("HOME");
        std::env::set_var("HOME", &root);
        let path = root.join("fake-login");
        fs::write(&path, format!("#!/usr/bin/python3 -I\nimport os, sys, time\nassert 'native-fixture-' in os.environ['HOME']\nassert os.isatty(0) and os.isatty(1)\nassert os.getcwd() != os.environ['HOME']\nassert os.stat(os.getcwd()).st_mode & 0o777 == 0o700\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            path,
            home,
            _lock: lock,
        }
    }
    fn start(&self) -> NativeSession {
        NativeSession::start(&self.path, &args(&["acp", "--login"])).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        match &self.home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        // Fixtures intentionally retained for the parent to inspect and remove.
    }
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
fn until(s: &mut NativeSession, done: impl Fn(&AuthProgress) -> bool) -> AuthProgress {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let v = s.poll().unwrap();
        assert_eq!(v.kind, "native");
        assert!(v.url.is_none() && v.user_code.is_none());
        if done(&v) {
            return v;
        }
        assert!(Instant::now() < deadline, "synthetic login timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn pid(s: &mut NativeSession) -> u32 {
    let v = until(s, |v| v.output.contains("READY\n"));
    v.output.lines().next().unwrap().parse().unwrap()
}
fn reaped(id: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(id as i32, &mut status, libc::WNOHANG) };
        if result == -1 {
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
            assert_eq!(unsafe { libc::kill(id as i32, 0) }, -1);
            return;
        }
        assert_eq!(result, 0, "transport left a zombie child");
        assert!(Instant::now() < deadline, "transport left a live child");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn fixed_login_allowlist_rejects_shell_chat_options_and_untrusted_identifiers() {
    let f = Fixture::new("print('READY', flush=True)\nsys.stdin.readline()");
    for a in [
        vec![],
        vec!["acp"],
        vec!["-c", "echo"],
        vec!["auth", "login", "--token", "secret"],
        vec!["acp", "--login", "--no-browser"],
        vec!["auth", "login", "../x", "--method", "oauth"],
        vec!["auth", "login", "--bad", "--method", "oauth"],
        vec!["auth", "login", "x*y", "--method", "oauth"],
        vec!["auth", "login", "x\ny", "--method", "oauth"],
        vec!["auth", "add", "x", "--type", "api-key"],
    ] {
        assert!(NativeSession::start(&f.path, &args(&a)).is_err());
    }
    for path in [
        PathBuf::from("relative"),
        f.path.with_extension("CMD"),
        f.path.with_extension("bat"),
    ] {
        assert!(NativeSession::start(&path, &args(&["auth", "login"])).is_err());
    }
    fs::set_permissions(&f.path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(NativeSession::start(&f.path, &args(&["auth", "login"])).is_err());
}

#[test]
fn all_four_fixed_login_forms_keep_native_tty_and_nonblocking_waiting_state() {
    fn assert_send<T: Send>() {}
    assert_send::<NativeSession>();
    let f = Fixture::new("print('READY', flush=True)\nsys.stdin.readline()");
    for a in [
        vec!["acp", "--login"],
        vec!["auth", "login"],
        vec!["auth", "login", "provider-name", "--method", "oauth_2"],
        vec!["auth", "add", "provider", "--type", "oauth"],
    ] {
        let mut s = NativeSession::start(&f.path, &args(&a)).unwrap();
        let v = until(&mut s, |v| v.output.contains("READY"));
        assert_eq!(v.state, "waiting");
        let now = Instant::now();
        s.poll().unwrap();
        assert!(now.elapsed() < Duration::from_millis(100));
        s.cancel().unwrap();
    }
}

#[test]
fn code_is_single_bounded_line_and_echo_is_redacted_even_across_chunks() {
    let f = Fixture::new("print('Paste code:', flush=True)\ncode = sys.stdin.readline().strip()\nassert code == 'FAKE-CODE_123'\nassert all(code not in value for value in sys.argv + list(os.environ.values()))\nassert os.listdir(os.getcwd()) == []\nprint('ACCEPTED', flush=True)\nfor part in [code[:5], code[5:]]:\n    sys.stdout.write(part); sys.stdout.flush(); time.sleep(0.12)\nprint(' done', flush=True)\ntime.sleep(0.2)");
    let mut s = f.start();
    until(&mut s, |v| v.output.contains("Paste code:"));
    let before = s.view().output;
    for bad in [
        "",
        "two words",
        "x\ny",
        "x\ry",
        "x\u{1b}y",
        "x{y}",
        "x[y]",
        "é",
        &"X".repeat(1025),
    ] {
        assert!(s.submit_code(bad).is_err());
        assert_eq!(s.view().output, before);
        assert_eq!(s.view().state, "waiting");
    }
    s.submit_code("FAKE-CODE_123").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_redaction = false;
    loop {
        let v = s.poll().unwrap();
        assert!(!v.output.contains("FAKE"), "partial echo escaped redaction");
        assert!(!v.output.contains("CODE_123"));
        saw_redaction |= v.output.contains("[redacted]");
        if v.state != "waiting" {
            assert_eq!(v.state, "finished");
            assert!(v.output.is_empty());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(saw_redaction);
    assert!(s.submit_code("LATE").is_err());
}

#[test]
fn escape_sequences_and_controls_are_removed_across_reads() {
    let f = Fixture::new("for part in [b'hello\\x1b[', b'31m red\\x1b[0m\\x1b]52;c;', b'PRIVATE\\x07\\x1bP', b'DCS-PRIVATE\\x1b', b'\\\\ done\\x00\\x08\\r\\n']:\n    os.write(1, part); time.sleep(0.04)\nsys.stdin.readline()");
    let mut s = f.start();
    let v = until(&mut s, |v| v.output.contains("done"));
    assert_eq!(v.output, "hello red done\n");
    s.cancel().unwrap();
}

#[test]
fn zero_exit_is_finished_not_authenticated_and_nonzero_is_failed() {
    let f = Fixture::new("print('Authenticated successfully!', flush=True)\nsys.stdin.readline()\nsys.exit(0 if sys.argv[1] == 'acp' else 7)");
    for (a, state) in [
        (vec!["acp", "--login"], "finished"),
        (vec!["auth", "login"], "failed"),
    ] {
        let mut s = NativeSession::start(&f.path, &args(&a)).unwrap();
        assert_eq!(
            until(&mut s, |v| v.output.contains("Authenticated")).state,
            "waiting"
        );
        s.submit_code("SYNTHETIC").unwrap();
        let v = until(&mut s, |v| v.state != "waiting");
        assert_eq!(v.state, state);
        assert!(!v.message.to_lowercase().contains("authenticated"));
        assert!(v.output.is_empty());
        s.cancel().unwrap();
    }
}

#[test]
fn cancellation_and_drop_kill_and_reap_the_actual_direct_child() {
    let f =
        Fixture::new("print(os.getpid(), flush=True)\nprint('READY', flush=True)\ntime.sleep(60)");
    let mut s = f.start();
    let id = pid(&mut s);
    let now = Instant::now();
    s.cancel().unwrap();
    assert!(now.elapsed() < Duration::from_secs(2));
    assert_eq!(s.view().state, "cancelled");
    assert!(s.view().output.is_empty());
    s.cancel().unwrap();
    reaped(id);
    let mut s = f.start();
    let id = pid(&mut s);
    drop(s);
    reaped(id);
}

#[test]
fn bounded_output_and_total_stream_limit_stop_a_flooding_child() {
    let f = Fixture::new("print(os.getpid(), flush=True)\nprint('READY', flush=True)\nsys.stdin.readline()\nos.write(1, b'x' * (1024 * 1024 + 1))\ntime.sleep(60)");
    let mut s = f.start();
    let id = pid(&mut s);
    s.submit_code("GO").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let v = s.poll().unwrap();
        assert!(v.output.len() <= 64 * 1024);
        if v.state != "waiting" {
            assert_eq!(v.state, "failed");
            assert!(v.output.is_empty());
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    reaped(id);
}

#[test]
fn minimum_and_maximum_code_lengths_and_url_characters_are_supported_once() {
    let f = Fixture::new("print('READY', flush=True)\ncode = sys.stdin.readline().strip()\nprint('ACCEPTED-' + str(len(code)), flush=True)\ntime.sleep(0.2)");
    for code in [
        "Z".into(),
        "X".repeat(1024),
        "https://example.invalid/callback?code=ABC_12&state=x%2By#z@~.-".into(),
    ] {
        let mut s = f.start();
        until(&mut s, |v| v.output.contains("READY"));
        s.submit_code(&code).unwrap();
        let expected = format!("ACCEPTED-{}", code.len());
        until(&mut s, |v| v.output.contains(&expected));
        let before = s.view().output;
        assert!(s.submit_code("SECOND").is_err());
        assert_eq!(s.view().output, before);
        s.cancel().unwrap();
    }
}

#[test]
fn invalid_utf8_output_is_still_bounded_plain_text() {
    let f = Fixture::new(
        "os.write(1, b'\\xff' * 65536)\nprint('READY', flush=True)\nsys.stdin.readline()",
    );
    let mut s = f.start();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let v = s.poll().unwrap();
        assert!(v.output.len() <= 64 * 1024);
        assert!(!v
            .output
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t'));
        if v.output.len() > 60 * 1024 {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    s.cancel().unwrap();
}
