//! Backend-only native LOGIN transport; never expose paths, argv, or keystrokes over IPC.
use crate::auth_types::AuthProgress;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::{Zeroize, Zeroizing};

const ERROR: &str = "Native sign-in unavailable or exceeded safety limits.";
const MAX_OUTPUT: usize = 64 * 1024;
const MAX_STREAM: usize = 1024 * 1024;
const MAX_CODE: usize = 1024;
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A transient login attempt, owned by the backend permission/expiry manager.
pub struct NativeSession {
    child: Option<Box<dyn Child + Send + Sync>>,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    reader: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    output: Arc<Mutex<Output>>,
    progress: AuthProgress,
    _cwd: Neutral,
    #[cfg(test)]
    before_publish: Option<Box<dyn FnOnce() + Send>>,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn login_args(args: &[String]) -> bool {
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["acp", "--login"] | ["auth", "login"] => true,
        ["auth", "login", provider, "--method", method] => {
            identifier(provider) && identifier(method)
        }
        ["auth", "add", provider, "--type", "oauth"] => identifier(provider),
        _ => false,
    }
}
fn executable(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .to_str()
            .is_none_or(|s| s.chars().any(char::is_control))
    {
        return false;
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
    {
        return false;
    }
    let Ok(m) = path.metadata() else {
        return false;
    };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    #[cfg(windows)]
    {
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            return false;
        }
    }
    true
}

impl NativeSession {
    /// Only the backend may supply its discovered executable and validated fixed login argv.
    /// This function does not establish consent or validate provider ownership of the binary.
    pub fn start(path: &Path, args: &[String]) -> Result<Self, String> {
        if !login_args(args) || !executable(path) {
            return Err(ERROR.into());
        }
        let cwd = Neutral::create()?;
        // API reference: https://docs.rs/portable-pty/0.9.0/portable_pty/
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| ERROR)?;
        // Unix duplicates share O_NONBLOCK: the reader can observe cancellation without
        // retaining a blocking master fd forever if a descendant holds the slave open.
        #[cfg(unix)]
        {
            let fd = pair.master.as_raw_fd().ok_or(ERROR)?;
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(ERROR.into());
            }
        }
        let mut reader = pair.master.try_clone_reader().map_err(|_| ERROR)?;
        let writer = pair.master.take_writer().map_err(|_| ERROR)?;
        let mut command = CommandBuilder::new(path);
        command.args(args);
        command.cwd(&cwd.0);
        command.env("TERM", "dumb");
        let child = pair.slave.spawn_command(command).map_err(|_| ERROR)?;
        drop(pair.slave);
        let mut session = Self {
            child: Some(child),
            master: Some(pair.master),
            writer: Some(writer),
            reader: None,
            stop: Arc::new(AtomicBool::new(false)),
            output: Arc::new(Mutex::new(Output::default())),
            progress: AuthProgress::waiting("native"),
            _cwd: cwd,
            #[cfg(test)]
            before_publish: None,
        };
        let stop = session.stop.clone();
        let output = session.output.clone();
        let mut killer = session.child.as_ref().ok_or(ERROR)?.clone_killer();
        session.reader = Some(
            thread::Builder::new()
                .name("native-login-output".into())
                .spawn(move || {
                    let mut bytes = Zeroizing::new([0u8; 2048]);
                    while !stop.load(Ordering::Acquire) {
                        match reader.read(&mut bytes[..]) {
                            Ok(0) => break,
                            Ok(n) => {
                                let Ok(mut out) = output.lock() else {
                                    let _ = killer.kill();
                                    break;
                                };
                                if stop.load(Ordering::Acquire) {
                                    break;
                                }
                                out.ingest(&bytes[..n]);
                                bytes.zeroize();
                                if out.failed {
                                    let _ = killer.kill();
                                    break;
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(5))
                            }
                            Err(_) => {
                                if !stop.load(Ordering::Acquire) {
                                    if let Ok(mut out) = output.lock() {
                                        out.failed = true;
                                    }
                                    let _ = killer.kill();
                                }
                                break;
                            }
                        }
                    }
                })
                .map_err(|_| ERROR)?,
        );
        Ok(session)
    }

    pub fn view(&self) -> AuthProgress {
        // An immutable startup view cannot reap a child. Only poll/finish may
        // commit a terminal state; failed output must remain hidden meanwhile.
        self.snapshot().unwrap_or_else(|()| {
            let mut result = self.progress.clone();
            result.output.clear();
            result
        })
    }

    fn snapshot(&self) -> Result<AuthProgress, ()> {
        let mut result = self.progress.clone();
        if result.state == "waiting" {
            match self.output.lock() {
                Ok(out) if !out.failed => {
                    result.output = String::from_utf8_lossy(&out.visible)
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
                        .collect();
                    if result.output.len() > MAX_OUTPUT {
                        let mut end = MAX_OUTPUT;
                        while !result.output.is_char_boundary(end) {
                            end -= 1;
                        }
                        result.output.truncate(end);
                    }
                }
                _ => return Err(()),
            }
        }
        Ok(result)
    }

    /// Never waits for input or completion; output is already bounded by the reader.
    pub fn poll(&mut self) -> Result<AuthProgress, String> {
        if self.progress.state != "waiting" {
            return Ok(self.view());
        }
        if self.output.lock().map(|o| o.failed).unwrap_or(true) {
            self.finish("failed", ERROR)?;
            return Ok(self.view());
        }
        match self.child.as_mut().ok_or(ERROR)?.try_wait() {
            Ok(Some(status)) => {
                self.child.take(); // try_wait has reaped the process.
                self.finish(
                    if status.success() {
                        "finished"
                    } else {
                        "failed"
                    },
                    if status.success() {
                        "Native sign-in process finished. Check status separately."
                    } else {
                        "Native sign-in process failed."
                    },
                )?;
            }
            Ok(None) => (),
            Err(_) => {
                self.finish("failed", ERROR)?;
                return Err(ERROR.into());
            }
        }
        #[cfg(test)]
        if let Some(hook) = self.before_publish.take() {
            hook();
        }
        match self.snapshot() {
            Ok(view) => Ok(view),
            Err(()) => {
                self.finish("failed", ERROR)?;
                Ok(self.view())
            }
        }
    }

    /// One ASCII code or callback URL line, never a command, option, environment value or file.
    pub fn submit_code(&mut self, code: &str) -> Result<(), String> {
        if self.progress.state != "waiting"
            || code.is_empty()
            || code.len() > MAX_CODE
            || !code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._~:/?&=%+#@-".contains(&b))
        {
            return Err(ERROR.into());
        }
        if let Some(status) = self
            .child
            .as_mut()
            .ok_or(ERROR)?
            .try_wait()
            .map_err(|_| ERROR)?
        {
            self.child.take();
            self.finish(
                if status.success() {
                    "finished"
                } else {
                    "failed"
                },
                "Native sign-in process ended. Check status separately.",
            )?;
            return Err(ERROR.into());
        }
        {
            let mut out = self.output.lock().map_err(|_| ERROR)?;
            if out.failed || !out.secrets.is_empty() {
                return Err(ERROR.into());
            }
            // Discard prior transient instructions before registering a possible echo.
            out.visible.zeroize();
            out.visible.clear();
            out.pending.zeroize();
            out.pending.clear();
            out.secrets.push(Zeroizing::new(code.to_owned()));
        }
        let writer = self.writer.as_mut().ok_or(ERROR)?;
        // No joined allocation containing the submitted code. A single line only.
        if writer
            .write_all(code.as_bytes())
            .and_then(|_| writer.write_all(b"\n"))
            .and_then(|_| writer.flush())
            .is_err()
        {
            self.finish("failed", ERROR)?;
            return Err(ERROR.into());
        }
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), String> {
        let state = if self.progress.state == "waiting" {
            "cancelled"
        } else {
            self.progress.state.as_str()
        };
        let state = state.to_owned();
        let message = if state == "cancelled" {
            "Native sign-in cancelled.".into()
        } else {
            self.progress.message.clone()
        };
        self.finish(&state, &message)
    }

    fn finish(&mut self, state: &str, message: &str) -> Result<(), String> {
        self.progress.state = state.into();
        self.progress.message = message.into();
        self.stop.store(true, Ordering::Release);
        let mut failed = false;
        if let Some(mut child) = self.child.take() {
            // Kill before closing the writer: its Drop may emit EOF, which must not
            // accidentally complete an otherwise cancelled login.
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                    _ => {
                        // No unbounded wait in UI cancellation or Drop. A final reaper
                        // owns the child if the OS has not completed termination yet.
                        failed = true;
                        let _ = thread::Builder::new()
                            .name("native-login-reap".into())
                            .spawn(move || {
                                let _ = child.kill();
                                let _ = child.wait();
                            });
                        break;
                    }
                }
            }
        }
        self.writer.take();
        self.master.take();
        if let Some(reader) = self.reader.take() {
            // Never block on a reader retaining a platform-specific terminal handle.
            if reader.is_finished() {
                let _ = reader.join();
            }
        }
        if let Ok(mut out) = self.output.lock() {
            out.clear();
        }
        if failed {
            Err(ERROR.into())
        } else {
            Ok(())
        }
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

#[cfg(all(test, unix))]
mod failure_publication_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(Neutral);
    impl Fixture {
        fn start() -> (Self, NativeSession) {
            let root = Neutral::create().unwrap();
            let path = root.0.join("synthetic-login");
            std::fs::write(
                &path,
                "#!/usr/bin/python3 -I\nimport sys\nsys.stdin.readline()\n",
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            let session = NativeSession::start(&path, &["acp".into(), "--login".into()]).unwrap();
            (Self(root), session)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0 .0.join("synthetic-login"));
        }
    }

    #[test]
    fn immutable_view_suppresses_failed_output_until_poll_finalizes() {
        let (_fixture, mut session) = Fixture::start();
        {
            let mut output = session.output.lock().unwrap();
            output.visible.extend_from_slice(b"synthetic instructions");
            output.failed = true;
        }
        let view = session.view();
        assert_eq!(view.state, "waiting");
        assert!(view.output.is_empty());
        let view = session.poll().unwrap();
        assert_eq!(view.state, "failed");
        assert_eq!(view.message, ERROR);
        assert!(view.output.is_empty());
        assert!(session.child.is_none());
    }

    #[test]
    fn reader_failure_during_poll_is_reaped_before_terminal_publication() {
        let (_fixture, mut session) = Fixture::start();
        let id = session.child.as_ref().unwrap().process_id().unwrap();
        let mut killer = session.child.as_ref().unwrap().clone_killer();
        let output = session.output.clone();
        let (release, released) = std::sync::mpsc::channel();
        let (done, completed) = std::sync::mpsc::channel();
        let reader = thread::spawn(move || {
            released.recv().unwrap();
            let mut output = output.lock().unwrap();
            output.total = MAX_STREAM;
            output.ingest(b"x");
            assert!(output.failed);
            killer.kill().unwrap();
            drop(output);
            done.send(()).unwrap();
        });
        session.before_publish = Some(Box::new(move || {
            release.send(()).unwrap();
            completed.recv().unwrap();
        }));
        let view = session.poll().unwrap();
        reader.join().unwrap();
        assert_eq!(view.state, "failed");
        assert_eq!(view.message, ERROR);
        assert!(view.output.is_empty());
        assert_eq!(session.progress.state, "failed");
        assert!(session.child.is_none());
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(id as i32, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}

#[derive(Default)]
enum Escape {
    #[default]
    Text,
    Esc,
    Csi,
    String,
    StringEsc,
    Intermediate,
}
struct Output {
    visible: Zeroizing<Vec<u8>>,
    pending: Zeroizing<Vec<u8>>,
    secrets: Vec<Zeroizing<String>>,
    escape: Escape,
    total: usize,
    failed: bool,
}
impl Default for Output {
    fn default() -> Self {
        Self {
            // Reserve once: secret prefixes must not survive Vec reallocations.
            pending: Zeroizing::new(Vec::with_capacity(MAX_CODE)),
            visible: Zeroizing::new(Vec::with_capacity(MAX_OUTPUT + 4096)),
            secrets: Vec::new(),
            escape: Escape::default(),
            total: 0,
            failed: false,
        }
    }
}
impl Output {
    fn clear(&mut self) {
        self.visible.zeroize();
        self.visible.clear();
        self.pending.zeroize();
        self.pending.clear();
        self.secrets.clear();
    }
    fn ingest(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len());
        if self.total > MAX_STREAM {
            self.failed = true;
            self.clear();
            return;
        }
        for &b in bytes {
            match self.escape {
                Escape::Text => match b {
                    0x1b => self.escape = Escape::Esc,
                    b'\n' | b'\t' | 0x20..=0x7e | 0x80..=0xff => self.redact(b),
                    _ => (),
                },
                Escape::Esc => {
                    self.escape = match b {
                        b'[' => Escape::Csi,
                        b']' | b'P' | b'X' | b'^' | b'_' => Escape::String,
                        0x20..=0x2f => Escape::Intermediate,
                        0x1b => Escape::Esc,
                        _ => Escape::Text,
                    }
                }
                Escape::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.escape = Escape::Text;
                    }
                }
                Escape::String => match b {
                    0x07 => self.escape = Escape::Text,
                    0x1b => self.escape = Escape::StringEsc,
                    _ => (),
                },
                Escape::StringEsc => {
                    self.escape = if b == b'\\' || b == 0x07 {
                        Escape::Text
                    } else if b == 0x1b {
                        Escape::StringEsc
                    } else {
                        Escape::String
                    }
                }
                Escape::Intermediate => {
                    if (0x30..=0x7e).contains(&b) {
                        self.escape = Escape::Text;
                    }
                }
            }
        }
        if self.visible.len() > MAX_OUTPUT {
            let excess = self.visible.len() - MAX_OUTPUT;
            self.visible[..excess].zeroize();
            self.visible.drain(..excess);
        }
    }
    fn redact(&mut self, b: u8) {
        self.pending.push(b);
        loop {
            if self
                .secrets
                .iter()
                .any(|s| s.as_bytes() == self.pending.as_slice())
            {
                self.pending.zeroize();
                self.pending.clear();
                self.visible.extend_from_slice(b"[redacted]");
                break;
            }
            if self
                .secrets
                .iter()
                .any(|s| s.as_bytes().starts_with(&self.pending))
            {
                break;
            }
            if self.pending.is_empty() {
                break;
            }
            self.visible.push(self.pending[0]);
            self.pending.rotate_left(1);
            if let Some(last) = self.pending.last_mut() {
                last.zeroize();
            }
            self.pending.pop();
        }
    }
}

/// Separate owned auth cwd; no repository instructions/plugins are discovered here.
struct Neutral(PathBuf);
impl Neutral {
    fn create() -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ERROR)?
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ai-native-login-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        if !path.is_absolute() {
            return Err(ERROR.into());
        }
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).map_err(|_| ERROR)?;
        Ok(Self(path))
    }
}
impl Drop for Neutral {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}
