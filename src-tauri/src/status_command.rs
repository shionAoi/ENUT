//! Fixed, permission-gated status operations. Never accept frontend argv or executable paths.
use serde_json::Value;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, process::Command};

pub const ERROR: &str = "Status command unavailable, unsupported, or exceeded safety limits.";
pub const AUTH_REQUIRED: &str =
    "Antigravity reports authentication is required. Use Login to sign in; saved history is preserved.";
pub const INVALID: &str = "Unsupported status metadata.";
pub const MAX_BYTES: usize = 1024 * 1024;
const MAX_STDERR: usize = 64 * 1024;

pub fn is_antigravity_auth_required(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr)
        .to_ascii_lowercase()
        .contains("authentication required")
}

async fn read_bounded_stderr(
    stderr: Option<tokio::process::ChildStderr>,
) -> Result<Vec<u8>, &'static str> {
    let Some(mut stderr) = stderr else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = stderr.read(&mut buf).await.map_err(|_| ERROR)?;
        if n == 0 {
            break;
        }
        let room = MAX_STDERR.saturating_sub(out.len());
        if room > 0 {
            out.extend_from_slice(&buf[..n.min(room)]);
        }
    }
    Ok(out)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    AntigravityVersion,
    AntigravityUsage,
    HermesUsage,
    OpenCodeStats,
    OpenCodeLimits,
    OpenCodeMcps,
    OpenCodeSkills,
    OpenCodeIntegrations,
    OpenCodeActiveSessions,
    ClaudeAgents,
}
impl Operation {
    pub fn tool(self) -> &'static str {
        match self {
            Self::AntigravityVersion | Self::AntigravityUsage => "antigravity",
            Self::HermesUsage => "hermes",
            Self::ClaudeAgents => "claude",
            _ => "opencode",
        }
    }
    pub fn args(self) -> &'static [&'static str] {
        match self {
            Self::AntigravityVersion => &["--version"],
            Self::AntigravityUsage => &["-p", "/usage", "--output-format", "json"],
            Self::HermesUsage => &["usage", "--json"],
            Self::OpenCodeStats => &["api", "get", "/api/experimental/session/stats"],
            Self::OpenCodeLimits => &["api", "get", "/api/experimental/usage/limits"],
            Self::OpenCodeMcps => &["api", "get", "/api/mcp"],
            Self::OpenCodeSkills => &["api", "get", "/api/skill"],
            Self::OpenCodeIntegrations => &["api", "get", "/api/integration"],
            Self::OpenCodeActiveSessions => &["api", "get", "/api/session/active"],
            Self::ClaudeAgents => &["agents", "--json"],
        }
    }
}
pub type CommandFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>>;
pub trait CommandRunner: Send + Sync {
    fn run(&self, op: Operation) -> CommandFuture<'_>;
}
pub struct ProcessRunner {
    resolve: fn(&str) -> Option<PathBuf>,
}
impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            resolve: crate::discovery::installed_path,
        }
    }
}
impl ProcessRunner {
    /// Trusted backend dependency injection for fixture executables; never expose through IPC.
    pub fn with_resolver(resolve: fn(&str) -> Option<PathBuf>) -> Self {
        Self { resolve }
    }
}
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Neutral(PathBuf);
impl Neutral {
    fn create() -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ERROR)?
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "ai-status-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&p).map_err(|_| ERROR)?;
        Ok(Self(p))
    }
}
impl Drop for Neutral {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}
impl CommandRunner for ProcessRunner {
    fn run(&self, op: Operation) -> CommandFuture<'_> {
        Box::pin(async move {
            let path = (self.resolve)(op.tool())
                .filter(|p| p.is_absolute())
                .ok_or(ERROR)?;
            // Windows batch wrappers require a shell and are intentionally unsupported.
            if path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("cmd") || x.eq_ignore_ascii_case("bat"))
            {
                return Err(ERROR.into());
            }
            let cwd = Neutral::create()?;
            let mut command = Command::new(path);
            command
                .args(op.args())
                .current_dir(&cwd.0)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            if op == Operation::AntigravityUsage {
                // agy mistakes /dev/null for interactive input; use EOF without a terminal.
                command.stdin(Stdio::piped()).stderr(Stdio::piped());
                #[cfg(unix)]
                unsafe {
                    command.pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            let mut child = command.spawn().map_err(|_| ERROR)?;
            drop(child.stdin.take());
            let stdout = child.stdout.take().ok_or(ERROR)?;
            let stderr = child.stderr.take();
            let work = async {
                let read = async {
                    let mut b = Vec::new();
                    stdout
                        .take((MAX_BYTES + 1) as u64)
                        .read_to_end(&mut b)
                        .await
                        .map_err(|_| ERROR)?;
                    if b.len() > MAX_BYTES {
                        return Err(ERROR);
                    }
                    Ok(b)
                };
                let read_err = read_bounded_stderr(stderr);
                let wait = async { child.wait().await.map_err(|_| ERROR) };
                let (bytes, err_bytes, status) = tokio::try_join!(read, read_err, wait)?;
                if !status.success() {
                    if op == Operation::AntigravityUsage && is_antigravity_auth_required(&err_bytes)
                    {
                        return Err(AUTH_REQUIRED);
                    }
                    return Err(ERROR);
                }
                Ok(bytes)
            };
            let result = tokio::time::timeout(Duration::from_secs(20), work).await;
            match result {
                Ok(Ok(b)) => Ok(b),
                Ok(Err(e)) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    Err(e.into())
                }
                Err(_) => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    Err(ERROR.into())
                }
            }
        })
    }
}
pub async fn json(r: &dyn CommandRunner, op: Operation) -> Result<Value, String> {
    let b = r.run(op).await?;
    if b.len() > MAX_BYTES {
        return Err(INVALID.into());
    }
    serde_json::from_slice(&b).map_err(|_| INVALID.into())
}
pub(crate) fn label(v: &Value) -> Result<String, String> {
    v.as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.trim() == *s
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b" _-.():+".contains(&b))
        })
        .map(String::from)
        .ok_or_else(|| INVALID.into())
}
pub(crate) fn rows(v: &Value) -> Result<&Vec<Value>, String> {
    v.as_array()
        .filter(|a| a.len() <= 256)
        .ok_or_else(|| INVALID.into())
}
pub(crate) fn percent(v: &Value) -> Result<f64, String> {
    v.as_f64()
        .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
        .ok_or_else(|| INVALID.into())
}
pub(crate) fn reset(v: &Value) -> Result<Option<String>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let s = v.as_str().filter(|s| s.len() <= 40).ok_or(INVALID)?;
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| Some(d.to_rfc3339()))
        .map_err(|_| INVALID.into())
}
pub(crate) fn epoch(v: &Value) -> Result<String, String> {
    let n = v
        .as_i64()
        .filter(|n| *n >= 0 && *n <= 9_007_199_254_740_991)
        .ok_or(INVALID)?;
    chrono::DateTime::from_timestamp_millis(n)
        .filter(|d| d.format("%Y").to_string().len() == 4)
        .map(|d| d.to_rfc3339())
        .ok_or_else(|| INVALID.into())
}
