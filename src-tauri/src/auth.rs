//! Native-owned authentication. No tokens, passwords or generic terminal commands.
use crate::{auth_types::AuthProgress, codex_auth::CodexSession, native_auth::NativeSession};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{watch, Mutex};
const ERROR: &str = "Native sign-in unavailable. Check the installed tool version and try again.";
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthChoice {
    pub id: String,
    pub provider: String,
    pub method: String,
    pub label: String,
    pub kind: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthView {
    pub attempt_id: String,
    pub tool_id: String,
    #[serde(flatten)]
    pub progress: AuthProgress,
}
fn choice(provider: &str, method: &str, label: &str, kind: &str) -> AuthChoice {
    AuthChoice {
        id: format!("{}:{provider}{method}", provider.len()),
        provider: provider.into(),
        method: method.into(),
        label: label.into(),
        kind: kind.into(),
    }
}
pub fn choices_for(tool: &str) -> Result<Vec<AuthChoice>, String> {
    Ok(match tool {
        "codex" => vec![choice(
            "chatgpt",
            "device",
            "ChatGPT device code",
            "device_code",
        )],
        "antigravity" => vec![choice(
            "google",
            "native",
            "Google · native browser/code login",
            "native",
        )],
        "claude" => vec![choice(
            "anthropic",
            "native",
            "Claude · native browser/code login",
            "native",
        )],
        "hermes" => ["openai-codex", "anthropic", "nous", "openrouter"]
            .into_iter()
            .map(|p| choice(p, "oauth", "Native OAuth", "native"))
            .collect(),
        "opencode" | "ollama" | "lmstudio" => vec![],
        _ => return Err("Unknown sign-in integration.".into()),
    })
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}
pub fn parse_integrations(v: &Value) -> Result<Vec<AuthChoice>, String> {
    let rows = v["data"]
        .as_array()
        .filter(|a| a.len() <= 128)
        .ok_or(ERROR)?;
    let mut out = vec![];
    for row in rows {
        let Some(p) = row["id"].as_str().filter(|s| identifier(s)) else {
            continue;
        };
        let Some(methods) = row["methods"].as_array().filter(|m| m.len() <= 64) else {
            continue;
        };
        for m in methods {
            if m["type"] != "oauth"
                || m.get("form")
                    .is_some_and(|f| !f.is_null() && f.as_array().is_none_or(|a| !a.is_empty()))
            {
                continue;
            }
            let Some(id) = m["id"].as_str().filter(|s| identifier(s)) else {
                continue;
            };
            let label = m["label"]
                .as_str()
                .filter(|s| s.len() <= 128 && !s.chars().any(char::is_control))
                .unwrap_or("Native OAuth");
            if out.len() >= 256 {
                return Err(ERROR.into());
            }
            out.push(choice(p, id, label, "native"));
        }
    }
    Ok(out)
}
pub fn login_args(tool: &str, c: &AuthChoice) -> Result<Vec<String>, String> {
    let args: Vec<&str> = match tool {
        "antigravity" if c.provider == "google" && c.method == "native" => vec!["acp", "--login"],
        "claude" if c.provider == "anthropic" && c.method == "native" => vec!["auth", "login"],
        "hermes"
            if ["openai-codex", "anthropic", "nous", "openrouter"]
                .contains(&c.provider.as_str())
                && c.method == "oauth" =>
        {
            vec!["auth", "add", &c.provider, "--type", "oauth"]
        }
        "opencode" if identifier(&c.provider) && identifier(&c.method) => {
            vec!["auth", "login", &c.provider, "--method", &c.method]
        }
        _ => return Err(ERROR.into()),
    };
    Ok(args.into_iter().map(String::from).collect())
}
pub fn valid_browser_url(s: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(s) else {
        return false;
    };
    u.scheme() == "https"
        && u.host_str() == Some("auth.openai.com")
        && u.port().is_none()
        && u.username().is_empty()
        && u.password().is_none()
        && u.path() == "/codex/device"
        && u.fragment().is_none()
}
enum Driver {
    Codex(CodexSession),
    Native(NativeSession),
}
impl Driver {
    async fn poll(&mut self) -> Result<AuthProgress, String> {
        match self {
            Self::Codex(s) => s.poll().await,
            Self::Native(s) => s.poll(),
        }
    }
    async fn cancel(&mut self) {
        match self {
            Self::Codex(s) => {
                let _ = s.cancel().await;
            }
            Self::Native(s) => {
                let _ = s.cancel();
            }
        }
    }
}
struct Attempt {
    view: AuthView,
    driver: Option<Driver>,
    changed: watch::Sender<()>,
    deadline: Instant,
    startup: Option<tokio::task::JoinHandle<()>>,
}
#[derive(Default)]
struct Inner {
    choices: HashMap<String, Vec<AuthChoice>>,
    attempt: Option<Attempt>,
}
#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<Mutex<Inner>>,
    start_gate: Arc<Mutex<()>>,
    resolve: fn(&str) -> Option<std::path::PathBuf>,
}
impl Default for AuthManager {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            start_gate: Arc::new(Mutex::new(())),
            resolve: crate::discovery::installed_path,
        }
    }
}
static NEXT: AtomicU64 = AtomicU64::new(1);
impl AuthManager {
    pub fn with_resolver(resolve: fn(&str) -> Option<std::path::PathBuf>) -> Self {
        Self {
            resolve,
            ..Self::default()
        }
    }
    pub async fn methods(&self, tool: &str) -> Result<Vec<AuthChoice>, String> {
        let c = if tool == "opencode" {
            use crate::status_command::{json, Operation, ProcessRunner};
            parse_integrations(
                &json(
                    &ProcessRunner::with_resolver(self.resolve),
                    Operation::OpenCodeIntegrations,
                )
                .await?,
            )?
        } else {
            choices_for(tool)?
        };
        self.inner
            .lock()
            .await
            .choices
            .insert(tool.into(), c.clone());
        Ok(c)
    }
    pub async fn active_tool(&self) -> Option<String> {
        self.inner
            .lock()
            .await
            .attempt
            .as_ref()
            .map(|a| a.view.tool_id.clone())
    }
    pub async fn active_id(&self) -> Option<String> {
        self.inner
            .lock()
            .await
            .attempt
            .as_ref()
            .map(|a| a.view.attempt_id.clone())
    }
    pub async fn start(&self, tool: &str, choice_id: &str) -> Result<AuthView, String> {
        let _start = self.start_gate.lock().await;
        if matches!(tool, "ollama" | "lmstudio") {
            return Err("Local runtime login is not required.".into());
        }
        let c = self
            .inner
            .lock()
            .await
            .choices
            .get(tool)
            .and_then(|rows| rows.iter().find(|c| c.id == choice_id))
            .cloned()
            .ok_or("Choose a current native sign-in method first.")?;
        self.cancel(None).await;
        let token = NEXT.fetch_add(1, Ordering::Relaxed).to_string();
        let mut progress = AuthProgress::waiting(&c.kind);
        progress.state = "preparing".into();
        progress.message = "Starting the selected native sign-in method…".into();
        let view = AuthView {
            attempt_id: token.clone(),
            tool_id: tool.into(),
            progress,
        };
        let (changed, mut cancelled) = watch::channel(());
        let mut expiry_cancel = changed.subscribe();
        self.inner.lock().await.attempt = Some(Attempt {
            view: view.clone(),
            driver: None,
            changed,
            deadline: Instant::now() + Duration::from_secs(900),
            startup: None,
        });
        let expiry_manager = self.clone();
        let expiry_id = token.clone();
        tokio::spawn(async move {
            tokio::select! {biased;_=expiry_cancel.changed()=>{},_=tokio::time::sleep(Duration::from_secs(900))=>{let _=expiry_manager.poll(&expiry_id).await;}}
        });
        let manager = self.clone();
        let tool = tool.to_string();
        let handle = tokio::spawn(async move {
            let build = async {
                let path = (manager.resolve)(&tool).ok_or(ERROR)?;
                if tool == "codex" {
                    return CodexSession::start(&path).await.map(Driver::Codex);
                }
                if tool == "antigravity" {
                    use crate::status_command::{CommandRunner, Operation, ProcessRunner};
                    let bytes = ProcessRunner::with_resolver(manager.resolve)
                        .run(Operation::AntigravityVersion)
                        .await?;
                    let s = std::str::from_utf8(&bytes).map_err(|_| ERROR)?.trim();
                    let parts = s
                        .split('.')
                        .map(str::parse::<u32>)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| ERROR)?;
                    if parts.len() != 3 || parts.as_slice() < [1, 5, 0].as_slice() {
                        return Err(
                            "Antigravity native sign-in requires a recognized stable CLI >=1.5.0."
                                .into(),
                        );
                    }
                }
                NativeSession::start(&path, &login_args(&tool, &c)?).map(Driver::Native)
            };
            let result = tokio::select! {biased; _=cancelled.changed()=>return, r=tokio::time::timeout(Duration::from_secs(30),build)=>r.unwrap_or_else(|_|Err(ERROR.into()))};
            let mut inner = manager.inner.lock().await;
            if let Some(a) = inner.attempt.as_mut().filter(|a| {
                a.view.attempt_id == token
                    && a.view.progress.state == "preparing"
                    && Instant::now() < a.deadline
            }) {
                match result {
                    Ok(driver) => {
                        a.view.progress = match &driver {
                            Driver::Codex(s) => s.view(),
                            Driver::Native(s) => s.view(),
                        };
                        a.driver = Some(driver);
                    }
                    Err(_) => {
                        a.view.progress = AuthProgress::waiting(&c.kind);
                        a.view.progress.state = "failed".into();
                        a.view.progress.message = ERROR.into();
                    }
                }
            }
        });
        if let Some(a) = self
            .inner
            .lock()
            .await
            .attempt
            .as_mut()
            .filter(|a| a.view.attempt_id == view.attempt_id)
        {
            a.startup = Some(handle);
        } else {
            handle.abort();
            let _ = handle.await;
        }
        Ok(view)
    }
    pub async fn poll(&self, id: &str) -> Result<AuthView, String> {
        let mut inner = self.inner.lock().await;
        let a = inner
            .attempt
            .as_mut()
            .filter(|a| a.view.attempt_id == id)
            .ok_or("Sign-in attempt is no longer active.")?;
        if Instant::now() >= a.deadline {
            a.changed.send_modify(|_| {});
            if let Some(d) = a.driver.as_mut() {
                d.cancel().await;
            }
            a.driver = None;
            a.view.progress = AuthProgress::waiting("native");
            a.view.progress.state = "expired".into();
            a.view.progress.message = "Sign-in expired. Start a new attempt.".into();
        } else if let Some(d) = a.driver.as_mut() {
            a.view.progress = d.poll().await.unwrap_or_else(|_| {
                let mut p = AuthProgress::waiting("native");
                p.state = "failed".into();
                p.message = ERROR.into();
                p
            });
        }
        if [
            "authenticated",
            "finished",
            "failed",
            "cancelled",
            "expired",
        ]
        .contains(&a.view.progress.state.as_str())
        {
            if let Some(mut d) = a.driver.take() {
                d.cancel().await;
            }
            a.view.progress.url = None;
            a.view.progress.user_code = None;
            a.view.progress.output.clear();
        }
        Ok(a.view.clone())
    }
    pub async fn submit(&self, id: &str, code: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().await;
        let a = inner
            .attempt
            .as_mut()
            .filter(|a| {
                a.view.attempt_id == id
                    && a.view.progress.state == "waiting"
                    && Instant::now() < a.deadline
            })
            .ok_or("Sign-in is not waiting for input.")?;
        match a.driver.as_mut() {
            Some(Driver::Native(s)) => s.submit_code(code),
            _ => Err("Enter device codes on the provider website, not in this app.".into()),
        }
    }
    pub async fn cancel(&self, id: Option<&str>) {
        let a = {
            let mut i = self.inner.lock().await;
            if id.is_some_and(|id| i.attempt.as_ref().is_none_or(|a| a.view.attempt_id != id)) {
                return;
            }
            i.attempt.take()
        };
        if let Some(mut a) = a {
            a.changed.send_modify(|_| {});
            if let Some(handle) = a.startup.take() {
                handle.abort();
                let _ = handle.await;
            }
            if let Some(d) = a.driver.as_mut() {
                d.cancel().await;
            }
        }
    }
    pub async fn browser_url(&self, id: &str) -> Result<String, String> {
        let i = self.inner.lock().await;
        let a = i
            .attempt
            .as_ref()
            .filter(|a| {
                a.view.attempt_id == id
                    && a.view.progress.state == "waiting"
                    && Instant::now() < a.deadline
            })
            .ok_or(ERROR)?;
        a.view
            .progress
            .url
            .clone()
            .filter(|u| valid_browser_url(u))
            .ok_or_else(|| ERROR.into())
    }
}
