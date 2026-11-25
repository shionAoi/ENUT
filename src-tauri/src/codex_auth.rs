//! Authentication-only Codex app-server transport. No browser, credential or inference API.
//! Protocol: https://github.com/openai/codex/tree/rust-v0.165.0/codex-rs/app-server-protocol/schema/json/v2
use crate::{auth_types::AuthProgress, codex::status_requests};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    time::{timeout, Instant},
};
use zeroize::Zeroize;

const ERROR: &str =
    "Codex sign-in unavailable, unsupported, or exceeded safety limits. No model request was made.";
const MAX_LINE: usize = 256 * 1024;
const MAX_TOTAL: usize = 1024 * 1024;
const START_ID: u64 = 10;
const READ_ID: u64 = 11;
const CANCEL_ID: u64 = 12;
static NEXT: AtomicU64 = AtomicU64::new(0);

// Session-owned, not a project directory or a credential store. Only remove this empty leaf.
struct WorkingDirectory(PathBuf);
impl WorkingDirectory {
    fn create() -> Result<Self, String> {
        let root = std::env::temp_dir().canonicalize().map_err(|_| ERROR)?;
        let cwd = std::env::current_dir()
            .map_err(|_| ERROR)?
            .canonicalize()
            .map_err(|_| ERROR)?;
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or(ERROR)?;
        if root.starts_with(&cwd) || root.starts_with(source) {
            return Err(ERROR.into());
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ERROR)?
            .as_nanos();
        let path = root.join(format!(
            "ai-codex-auth-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
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
impl Drop for WorkingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// One backend-owned native login attempt. The manager owns consent, expiry and serialization.
/// No Debug/Serialize: protocol/account data must never become UI output or logs.
pub struct CodexSession {
    child: Child,
    input: ChildStdin,
    output: Option<BufReader<ChildStdout>>,
    _directory: WorkingDirectory,
    frame: Vec<u8>,
    total: usize,
    outbound: Vec<u8>,
    written: usize,
    progress: AuthProgress,
    login_id: Option<String>,
    early: Vec<(String, Option<bool>)>,
    completion_seen: bool,
    account_deadline: Option<Instant>,
}

impl CodexSession {
    /// `path` is resolved by the trusted backend, never supplied by IPC.
    pub async fn start(path: &Path) -> Result<Self, String> {
        if !path.is_absolute()
            || (cfg!(windows)
                && !path
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        {
            return Err(ERROR.into());
        }
        let directory = WorkingDirectory::create()?;
        let mut child = crate::codex::command(path)
            .arg("app-server")
            .current_dir(&directory.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| ERROR)?;
        let input = child.stdin.take().ok_or(ERROR)?;
        let output = BufReader::new(child.stdout.take().ok_or(ERROR)?);
        let mut session = Self {
            child,
            input,
            output: Some(output),
            _directory: directory,
            frame: vec![],
            total: 0,
            outbound: vec![],
            written: 0,
            progress: AuthProgress::waiting("device_code"),
            login_id: None,
            early: vec![],
            completion_seen: false,
            account_deadline: None,
        };
        let result = timeout(Duration::from_secs(8), session.initialize()).await;
        if !matches!(result, Ok(Ok(()))) {
            session.clear_transient();
            let _ = session.reap().await;
            return Err(ERROR.into());
        }
        Ok(session)
    }

    pub fn view(&self) -> AuthProgress {
        self.progress.clone()
    }

    pub async fn poll(&mut self) -> Result<AuthProgress, String> {
        // All reads/writes keep their partial state in self before the next await.
        // Dropping this timeout cannot discard part of a JSON frame or request.
        let _ = timeout(Duration::from_millis(50), async {
            if self.progress.state == "waiting" && self.advance().await.is_err() {
                self.finish(
                    "failed",
                    "Codex sign-in failed. Try again with a supported native CLI.",
                );
            }
            if self.progress.state != "waiting" {
                let _ = self.reap().await;
            }
        })
        .await;
        Ok(self.view())
    }

    pub async fn cancel(&mut self) -> Result<(), String> {
        if self.progress.state != "waiting" {
            self.reap().await?;
            return Ok(());
        }
        // Attempt native cancellation before killing, even if a status reply is outstanding.
        let result = timeout(Duration::from_millis(500), async {
            self.flush_pending().await?;
            if let Some(id) = self.login_id.clone() {
                self.queue(
                    json!({"id":CANCEL_ID,"method":"account/login/cancel","params":{"loginId":id}}),
                )?;
                self.flush_pending().await?;
                let result = self.response(CANCEL_ID).await?;
                if !matches!(result["status"].as_str(), Some("canceled" | "notFound")) {
                    return Err(ERROR.into());
                }
            }
            Ok::<(), String>(())
        })
        .await;
        self.finish(
            "cancelled",
            "Codex sign-in cancelled. Existing native sign-in was not logged out.",
        );
        let reaped = self.reap().await;
        if matches!(result, Ok(Ok(()))) {
            reaped
        } else {
            Err(ERROR.into())
        }
    }

    async fn initialize(&mut self) -> Result<(), String> {
        let requests = status_requests();
        self.queue(requests[0].clone())?;
        self.flush_pending().await?;
        self.response(0).await?;
        self.queue(requests[1].clone())?;
        self.flush_pending().await?;
        self.queue(json!({"id":START_ID,"method":"account/login/start","params":{"type":"chatgptDeviceCode"}}))?;
        self.flush_pending().await?;
        let result = self.response(START_ID).await?;
        if result["type"].as_str() != Some("chatgptDeviceCode") {
            return Err(ERROR.into());
        }
        self.login_id = Some(clean(&result["loginId"], 512)?.to_owned());
        let code = clean(&result["userCode"], 128)?.to_owned();
        let url = verification_url(result["verificationUrl"].as_str().ok_or(ERROR)?)?;
        self.progress.url = Some(url);
        self.progress.user_code = Some(code);
        self.progress.message = "Open the provider site in your browser and enter this device code there, not in this app. Continue only if you started this sign-in.".into();
        Ok(())
    }

    async fn advance(&mut self) -> Result<(), String> {
        if !self.completion_seen {
            if let Some(success) = self
                .early
                .iter()
                .find(|(id, _)| Some(id) == self.login_id.as_ref())
                .map(|(_, success)| *success)
            {
                self.completed(success.ok_or(ERROR)?)?;
            }
            self.early.clear();
        }
        if self.progress.state != "waiting" {
            return Ok(());
        }
        loop {
            self.flush_pending().await?;
            let value = if let Some(deadline) = self.account_deadline {
                tokio::time::timeout_at(deadline, self.next())
                    .await
                    .map_err(|_| ERROR)??
            } else {
                self.next().await?
            };
            if value["method"] == "account/login/completed" {
                if let Some((id, success)) = completion(&value)? {
                    if Some(&id) == self.login_id.as_ref() && !self.completion_seen {
                        self.completed(success.ok_or(ERROR)?)?;
                    }
                }
            } else if self.account_deadline.is_some() && value["id"].as_u64() == Some(READ_ID) {
                if value.get("error").is_some()
                    || value["result"]["account"]["type"].as_str() != Some("chatgpt")
                {
                    return Err(ERROR.into());
                }
                self.finish(
                    "authenticated",
                    "Codex native sign-in completed. Monitoring permission is separate.",
                );
                return Ok(());
            }
            if self.progress.state != "waiting" {
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    }

    fn completed(&mut self, success: bool) -> Result<(), String> {
        self.completion_seen = true;
        if !success {
            self.finish(
                "failed",
                "Codex sign-in failed. Try again with a supported native CLI.",
            );
        } else {
            self.account_deadline = Some(Instant::now() + Duration::from_secs(8));
            self.queue(
                json!({"id":READ_ID,"method":"account/read","params":{"refreshToken":false}}),
            )?;
        }
        Ok(())
    }

    fn queue(&mut self, value: Value) -> Result<(), String> {
        if !self.outbound.is_empty() {
            return Err(ERROR.into());
        }
        self.outbound = serde_json::to_vec(&value).map_err(|_| ERROR)?;
        self.outbound.push(b'\n');
        self.written = 0;
        Ok(())
    }
    async fn flush_pending(&mut self) -> Result<(), String> {
        while self.written < self.outbound.len() {
            let count = self
                .input
                .write(&self.outbound[self.written..])
                .await
                .map_err(|_| ERROR)?;
            if count == 0 {
                return Err(ERROR.into());
            }
            self.written += count;
        }
        self.outbound.zeroize();
        self.outbound.clear();
        self.written = 0;
        Ok(())
    }

    async fn response(&mut self, expected: u64) -> Result<Value, String> {
        loop {
            let value = self.next().await?;
            if value["method"] == "account/login/completed" {
                if let Some(event) = completion(&value)? {
                    if !self.early.iter().any(|(id, _)| id == &event.0) {
                        if self.early.len() >= 64 {
                            return Err(ERROR.into());
                        }
                        self.early.push(event);
                    }
                }
            } else if value["id"].as_u64() == Some(expected) {
                if value.get("error").is_some() {
                    return Err(ERROR.into());
                }
                return value.get("result").cloned().ok_or_else(|| ERROR.into());
            }
            tokio::task::yield_now().await;
        }
    }

    async fn next(&mut self) -> Result<Value, String> {
        loop {
            let output = self.output.as_mut().ok_or(ERROR)?;
            let bytes = output.fill_buf().await.map_err(|_| ERROR)?;
            if bytes.is_empty() {
                return Err(ERROR.into());
            }
            let n = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| i + 1);
            if self.frame.len() + n > MAX_LINE || self.total + n > MAX_TOTAL {
                return Err(ERROR.into());
            }
            let done = bytes[n - 1] == b'\n';
            self.frame.extend_from_slice(&bytes[..n]);
            self.total += n;
            output.consume(n);
            if done {
                let result =
                    serde_json::from_slice::<Value>(&self.frame).map_err(|_| ERROR.to_string());
                self.frame.zeroize();
                self.frame.clear();
                return result.and_then(|v| {
                    if v.is_object() {
                        Ok(v)
                    } else {
                        Err(ERROR.into())
                    }
                });
            }
            // A continuously readable malicious child must not monopolize a poll.
            tokio::task::yield_now().await;
        }
    }

    fn clear_transient(&mut self) {
        self.output = None;
        self.progress.url = None;
        if let Some(mut code) = self.progress.user_code.take() {
            code.zeroize();
        }
        self.progress.output.zeroize();
        self.progress.output.clear();
        if let Some(mut id) = self.login_id.take() {
            id.zeroize();
        }
        self.frame.zeroize();
        self.frame.clear();
        self.outbound.zeroize();
        self.outbound.clear();
        for (id, _) in &mut self.early {
            id.zeroize();
        }
        self.early.clear();
        self.account_deadline = None;
    }
    fn finish(&mut self, state: &str, message: &str) {
        self.clear_transient();
        self.progress.state = state.into();
        self.progress.message = message.into();
        let _ = self.child.start_kill();
    }
    async fn reap(&mut self) -> Result<(), String> {
        let _ = self.child.start_kill();
        self.child.wait().await.map_err(|_| ERROR)?;
        Ok(())
    }
}
impl Drop for CodexSession {
    fn drop(&mut self) {
        self.clear_transient();
        let _ = self.child.start_kill();
        let _ = self.child.try_wait();
        // Child's kill_on_drop also covers cancellation outside a running poll.
        // directory is removed by its own empty-directory-only Drop.
    }
}

fn clean(value: &Value, max: usize) -> Result<&str, String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or_else(|| ERROR.into())
}
fn completion(value: &Value) -> Result<Option<(String, Option<bool>)>, String> {
    let params = &value["params"];
    let Some(id) = params["loginId"].as_str() else {
        return Ok(None);
    };
    if id.is_empty() || id.len() > 512 || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ERROR.into());
    }
    // Validate success only after correlating the ID. An unrelated notification
    // must not fail this attempt, including one from another protocol version.
    Ok(Some((id.into(), params["success"].as_bool())))
}
fn verification_url(raw: &str) -> Result<String, String> {
    if raw.len() > 2048 || raw.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(ERROR.into());
    }
    let url = reqwest::Url::parse(raw).map_err(|_| ERROR)?;
    // Url normalizes away explicit :443; check the original authority as well.
    let authority = raw
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""));
    if url.scheme() != "https"
        || !authority.is_some_and(|s| s.eq_ignore_ascii_case("auth.openai.com"))
        || url.host_str() != Some("auth.openai.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/codex/device"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ERROR.into());
    }
    Ok(url.to_string())
}
