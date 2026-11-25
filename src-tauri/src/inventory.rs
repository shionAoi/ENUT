use crate::Model;
use serde_json::Value;
use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_MODELS: usize = 2048;
const UNAVAILABLE: &str =
    "Local inventory unavailable; server may be stopped, protected, or incompatible.";

#[derive(Clone, Copy, Debug)]
pub enum Endpoint {
    OllamaTags,
    OllamaPs,
    LmModels,
}
impl Endpoint {
    pub fn all() -> [Self; 3] {
        [Self::OllamaTags, Self::OllamaPs, Self::LmModels]
    }
    pub fn url(self) -> &'static str {
        match self {
            Self::OllamaTags => "http://127.0.0.1:11434/api/tags",
            Self::OllamaPs => "http://127.0.0.1:11434/api/ps",
            Self::LmModels => "http://127.0.0.1:1234/api/v0/models",
        }
    }
    pub fn validate(method: &str, url: &str) -> Result<Self, String> {
        Self::all()
            .into_iter()
            .find(|e| method == "GET" && e.url() == url)
            .ok_or_else(|| "Operation is not an approved status request.".into())
    }
}

#[derive(Default)]
pub struct Observation {
    pub models: Vec<Model>,
    pub quotas: Vec<crate::Quota>,
    pub usage: Option<crate::Usage>,
    pub history: Vec<crate::DailyUsage>,
    pub model_usage: Vec<crate::ModelUsage>,
    pub message: Option<String>,
    pub partial_usage: Option<crate::PartialUsage>,
    pub model_history: Vec<crate::ModelDayUsage>,
    pub context: Option<crate::ContextInfo>,
    pub authentication: Option<String>,
    pub active_sessions: Option<crate::SessionActivity>,
    pub codex_records: Vec<crate::usage::TokenRecord>,
}
impl Observation {
    // Sources: https://github.com/ollama/ollama/blob/main/docs/api.md
    // https://lmstudio.ai/docs/developer/rest/endpoints#get-apiv0models
    pub fn ollama(tags: Value, ps: Option<Value>) -> Result<Self, String> {
        let loaded = ps
            .as_ref()
            .map(|v| parse_models(v, "models", "name", false))
            .transpose()?;
        let mut models = parse_models(&tags, "models", "name", false)?;
        for model in &mut models {
            model.loaded = loaded
                .as_ref()
                .map(|list| list.iter().any(|m| m.id == model.id));
        }
        // Loaded models may outlive removal from the on-disk inventory.
        if let Some(loaded) = loaded {
            for mut model in loaded {
                if !models.iter().any(|m| m.id == model.id) {
                    model.loaded = Some(true);
                    models.push(model);
                }
            }
        }
        Ok(Self {
            models,
            ..Default::default()
        })
    }
    pub fn lmstudio(value: Value) -> Result<Self, String> {
        Ok(Self {
            models: parse_models(&value, "data", "id", true)?,
            ..Default::default()
        })
    }
}

fn parse_models(value: &Value, key: &str, id_key: &str, lm: bool) -> Result<Vec<Model>, String> {
    let rows = value[key]
        .as_array()
        .filter(|v| v.len() <= MAX_MODELS)
        .ok_or("Unsupported or oversized model inventory.")?;
    let mut seen = HashSet::new();
    let mut result = vec![];
    for row in rows {
        let id = row[id_key]
            .as_str()
            .filter(|id| {
                !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
            })
            .ok_or("Invalid model identifier.")?;
        if !seen.insert(id) {
            continue;
        }
        result.push(Model {
            id: id.into(),
            name: id.into(),
            size_bytes: if lm { None } else { row["size"].as_u64() },
            loaded: if lm {
                match row["state"].as_str() {
                    Some("loaded") => Some(true),
                    Some("not-loaded") => Some(false),
                    _ => None,
                }
            } else {
                None
            },
        });
    }
    Ok(result)
}

pub type StatusFuture = Pin<Box<dyn Future<Output = Result<Observation, String>> + Send>>;
pub trait StatusReader: Send + Sync {
    fn read(&self, id: String) -> StatusFuture;

    /// Keep the service's per-tool status lock until the blocking scan actually exits.
    fn scan_blocking<T: Send + 'static>(
        work: impl FnOnce(&dyn Fn() -> bool) -> Result<T, String> + Send + 'static,
        unavailable: &'static str,
    ) -> impl Future<Output = Result<T, String>> + Send
    where
        Self: Sized,
    {
        scan_blocking(work, unavailable)
    }
}
pub struct LocalStatus;
pub(crate) struct StatusScan {
    pub(crate) _guard: tokio::sync::OwnedMutexGuard<()>,
    pub(crate) invalidated: Arc<AtomicBool>,
}
tokio::task_local! {
    pub(crate) static STATUS_SCAN: Arc<StatusScan>;
}

struct CancelScan(Arc<AtomicBool>);
impl Drop for CancelScan {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn scan_blocking<T: Send + 'static>(
    work: impl FnOnce(&dyn Fn() -> bool) -> Result<T, String> + Send + 'static,
    unavailable: &'static str,
) -> Result<T, String> {
    let scan = CancelScan(Default::default());
    let dropped = scan.0.clone();
    // The blocking closure, not its awaiter, owns this clone of the lock.
    let status = STATUS_SCAN.try_with(Arc::clone).ok();
    tokio::task::spawn_blocking(move || {
        let cancelled = || {
            dropped.load(Ordering::Acquire)
                || status
                    .as_ref()
                    .is_some_and(|s| s.invalidated.load(Ordering::Acquire))
        };
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        work(&cancelled)
    })
    .await
    .unwrap_or_else(|_| Err(unavailable.into()))
}
impl LocalStatus {
    async fn get(client: &reqwest::Client, endpoint: Endpoint) -> Result<Value, String> {
        let mut response = client
            .get(endpoint.url())
            .send()
            .await
            .map_err(|_| UNAVAILABLE.to_string())?;
        if !response.status().is_success() {
            return Err(UNAVAILABLE.into());
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE as u64)
        {
            return Err("Inventory exceeds size limit.".into());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| UNAVAILABLE.to_string())?
        {
            if body.len() + chunk.len() > MAX_RESPONSE {
                return Err("Inventory exceeds size limit.".into());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| "Unsupported inventory response.".into())
    }
}
impl StatusReader for LocalStatus {
    fn read(&self, id: String) -> StatusFuture {
        Box::pin(async move {
            let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(std::path::PathBuf::from);
            if id == "codex" {
                let base = crate::codex::read().await;
                let with_usage = attach_codex_usage(base, home.clone()).await;
                return attach_local_context(with_usage, "codex").await;
            }
            if id == "antigravity" {
                let base = crate::antigravity::read().await;
                let with_usage = attach_db_usage(base, home.clone(), "antigravity").await;
                return attach_local_context(with_usage, "antigravity").await;
            }
            if id == "opencode" {
                let base = crate::opencode_status::read().await;
                return attach_db_usage(base, home, "opencode").await;
            }
            if id == "hermes" {
                let base = crate::hermes::read().await;
                let with_usage = attach_db_usage(base, home.clone(), "hermes").await;
                return attach_local_context(with_usage, "hermes").await;
            }
            if matches!(id.as_str(), "claude" | "gemini") {
                let claude = id == "claude";
                let result = if let Some(home) = home {
                    let extra = match id.as_str() {
                        "claude" => crate::usage::extra_roots_from_env("CLAUDE_CONFIG_DIR", &home),
                        "gemini" => crate::usage::extra_roots_from_env("GEMINI_DATA_DIR", &home),
                        _ => vec![],
                    };
                    Self::scan_blocking(
                        move |cancelled| {
                            local_usage_observation_with_roots(
                                &home,
                                &extra,
                                &id,
                                chrono::Utc::now(),
                                cancelled,
                            )
                        },
                        "Local usage reader unavailable.",
                    )
                    .await
                } else {
                    Err("Local usage home unavailable.".into())
                };
                if !claude {
                    return result;
                }
                let result =
                    attach_session_activity(result, crate::session_activity::read_claude().await);
                return attach_local_context(result, "claude").await;
            }
            if id != "ollama" && id != "lmstudio" {
                return Err(match id.as_str() {
                    "codex" => "Codex quota unavailable: trusted app-server integration is not enabled. No sign-in or logs are read.",
                    "claude" => "Claude usage unavailable: version-aware local logs and separately authorized API billing are not enabled. Subscription tokens are never collected.",
                    _ => "Gemini usage unavailable: version-aware local logs are not enabled; internal quota endpoints are not used.",
                }.into());
            }
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .connect_timeout(Duration::from_millis(500))
                .build()
                .map_err(|_| "Status client unavailable.")?;
            if id == "ollama" {
                let tags = Self::get(&client, Endpoint::OllamaTags).await?;
                let ps = Self::get(&client, Endpoint::OllamaPs).await.ok();
                Observation::ollama(tags, ps)
            } else {
                Observation::lmstudio(Self::get(&client, Endpoint::LmModels).await?)
            }
        })
    }
}

fn attach_session_activity(
    observation: Result<Observation, String>,
    activity: Result<crate::SessionActivity, String>,
) -> Result<Observation, String> {
    match (observation, activity) {
        (Ok(mut o), Ok(a)) => {
            o.active_sessions = Some(a);
            Ok(o)
        }
        (Err(_), Ok(a)) => Ok(Observation {
            active_sessions: Some(a),
            message: Some(
                "Native active-session metadata only; recorded token consumption is unavailable."
                    .into(),
            ),
            ..Default::default()
        }),
        (o, Err(_)) => o,
    }
}

#[cfg(test)]
mod session_evidence_tests {
    use super::*;
    #[test]
    fn claude_session_metadata_and_recorded_tokens_are_independent() {
        let activity = crate::SessionActivity {
            sessions: vec![],
            source: "Synthetic".into(),
            scope: "Native fixture".into(),
            as_of: "2026-11-11T12:00:00Z".into(),
        };
        let o =
            attach_session_activity(Err("No saved token records".into()), Ok(activity)).unwrap();
        assert!(o.active_sessions.unwrap().sessions.is_empty());
        assert!(o.usage.is_none());
        let o = attach_session_activity(
            Ok(Observation {
                message: Some("Recorded input only".into()),
                ..Default::default()
            }),
            Err("Unsupported CLI".into()),
        )
        .unwrap();
        assert!(o.active_sessions.is_none());
        assert_eq!(o.message.as_deref(), Some("Recorded input only"));
        assert!(
            attach_session_activity(Err("No tokens".into()), Err("No session API".into())).is_err()
        );
    }

    #[test]
    fn merge_local_usage_preserves_antigravity_auth_required_and_keyring_locked_notes() {
        let summary = || crate::usage::UsageSummary {
            usage: crate::Usage {
                input_tokens: 1,
                output_tokens: 2,
                cached_tokens: 0,
                cache_creation_tokens: None,
                cache_read_tokens: None,
                total_tokens: 3,
                spend_usd: None,
                source: "Local".into(),
                scope: "device".into(),
                as_of: "2026-11-25T10:00:00Z".into(),
                period_start: None,
                period_end: None,
            },
            history: vec![],
            model_usage: vec![],
            model_history: vec![],
            warnings: vec![],
        };

        let auth_obs = merge_local_usage(
            Ok(Observation {
                authentication: Some("required".into()),
                message: Some(crate::status_command::AUTH_REQUIRED.into()),
                ..Default::default()
            }),
            Ok(summary()),
        )
        .unwrap();
        assert_eq!(auth_obs.authentication.as_deref(), Some("required"));
        assert!(auth_obs.quotas.is_empty());
        assert_eq!(
            auth_obs.message.as_deref(),
            Some(crate::status_command::AUTH_REQUIRED)
        );
        assert_eq!(auth_obs.usage.unwrap().total_tokens, 3);

        let locked_obs = merge_local_usage(
            Err(crate::antigravity::KEYRING_LOCKED.into()),
            Ok(summary()),
        )
        .unwrap();
        assert!(locked_obs
            .message
            .as_deref()
            .is_some_and(|m| m.starts_with(crate::antigravity::KEYRING_LOCKED)));
        assert_eq!(locked_obs.usage.unwrap().total_tokens, 3);
    }
}

async fn attach_local_context(
    observation: Result<Observation, String>,
    id: &'static str,
) -> Result<Observation, String> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(std::path::PathBuf::from);
    let Some(home) = home else { return observation };
    let context = LocalStatus::scan_blocking(
        move |cancelled| Ok(crate::context::read_local_until(&home, id, cancelled)),
        "Context discovery unavailable.",
    )
    .await?;
    match (observation, context) {
        (Ok(mut o), Ok(context)) => {
            o.context = Some(context);
            Ok(o)
        }
        (Ok(mut o), Err(_)) => {
            o.context = Some(crate::ContextInfo {
                note: "Context metadata unavailable; usage reporting is independent.".into(),
                ..Default::default()
            });
            Ok(o)
        }
        (Err(message), Ok(context)) if !context.mcps.is_empty() || !context.skills.is_empty() => {
            Ok(Observation {
                context: Some(context),
                message: Some(message),
                ..Default::default()
            })
        }
        (Err(message), _) => Err(message),
    }
}

async fn attach_codex_usage(
    observation: Result<Observation, String>,
    home: Option<std::path::PathBuf>,
) -> Result<Observation, String> {
    let Some(home) = home else {
        return observation;
    };
    let local = LocalStatus::scan_blocking(
        move |cancelled| {
            let extra = crate::usage::extra_roots_from_env("CODEX_HOME", &home);
            let files =
                crate::usage::read_session_files_with_roots(&home, &extra, "codex", cancelled)?;
            crate::usage::codex_records(&files, chrono::Utc::now())
        },
        "Codex rollout usage reader unavailable.",
    )
    .await;
    merge_codex_usage(observation, local)
}

fn merge_codex_usage(
    observation: Result<Observation, String>,
    local: Result<(crate::usage::UsageSummary, Vec<crate::usage::TokenRecord>), String>,
) -> Result<Observation, String> {
    let Ok((summary, records)) = local else {
        return observation;
    };
    let summary_models: Vec<crate::Model> = summary
        .model_usage
        .iter()
        .map(|m| crate::Model {
            id: m.model.clone(),
            name: m.model.clone(),
            size_bytes: None,
            loaded: None,
        })
        .collect();
    match observation {
        Ok(mut o) => {
            if o.models.is_empty() {
                o.models = summary_models;
            }
            o.usage = Some(summary.usage);
            o.history = summary.history;
            o.model_usage = summary.model_usage;
            o.model_history = summary.model_history;
            o.codex_records = records;
            Ok(o)
        }
        Err(_) => {
            let message = if summary.warnings.is_empty() {
                "Reported Codex rollout tokens (ccusage method).".into()
            } else {
                summary.warnings.join(" ")
            };
            Ok(Observation {
                models: summary_models,
                usage: Some(summary.usage),
                history: summary.history,
                model_usage: summary.model_usage,
                model_history: summary.model_history,
                message: Some(message),
                authentication: Some("notRequired".into()),
                codex_records: records,
                ..Default::default()
            })
        }
    }
}

async fn attach_db_usage(
    observation: Result<Observation, String>,
    home: Option<std::path::PathBuf>,
    id: &'static str,
) -> Result<Observation, String> {
    let Some(home) = home else {
        return observation;
    };
    let local = LocalStatus::scan_blocking(
        move |cancelled| {
            let now = chrono::Utc::now();
            let roots = match id {
                "opencode" => {
                    let mut roots = crate::usage::extra_roots_from_env("OPENCODE_DATA_DIR", &home);
                    if roots.is_empty() {
                        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
                            let xdg_path = std::path::PathBuf::from(xdg.trim());
                            if xdg_path.is_absolute() {
                                roots.push(xdg_path.join("opencode"));
                            }
                        }
                    }
                    roots
                }
                "antigravity" => crate::usage::extra_roots_from_env("ANTIGRAVITY_DATA_DIR", &home),
                "hermes" => crate::usage::extra_roots_from_env("HERMES_HOME", &home),
                _ => return Err("Unsupported local database usage integration.".into()),
            };
            read_db_usage(&home, &roots, id, now, cancelled)
        },
        "Local database usage reader unavailable.",
    )
    .await;
    merge_local_usage(observation, local)
}

fn read_db_usage(
    home: &std::path::Path,
    roots: &[std::path::PathBuf],
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
    cancelled: &dyn Fn() -> bool,
) -> Result<crate::usage::UsageSummary, String> {
    match id {
        "opencode" if roots.is_empty() => crate::opencode_usage::load_until(home, now, cancelled),
        "antigravity" if roots.is_empty() => {
            crate::antigravity_usage::load_until(home, now, cancelled)
        }
        "hermes" if roots.is_empty() => crate::hermes_usage::load_until(home, now, cancelled),
        "opencode" => crate::opencode_usage::load_from_roots(roots, now, cancelled),
        "antigravity" => crate::antigravity_usage::load_from_roots(roots, now, cancelled),
        "hermes" => crate::hermes_usage::load_from_roots(roots, now, cancelled),
        _ => Err("Unsupported local database usage integration.".into()),
    }
}

fn merge_local_usage(
    observation: Result<Observation, String>,
    local: Result<crate::usage::UsageSummary, String>,
) -> Result<Observation, String> {
    let Ok(summary) = local else {
        return observation;
    };
    let summary_models: Vec<crate::Model> = summary
        .model_usage
        .iter()
        .map(|m| crate::Model {
            id: m.model.clone(),
            name: m.model.clone(),
            size_bytes: None,
            loaded: None,
        })
        .collect();
    match observation {
        Ok(mut o) => {
            if o.models.is_empty() {
                o.models = summary_models;
            }
            o.usage = Some(summary.usage);
            o.history = summary.history;
            o.model_usage = summary.model_usage;
            o.model_history = summary.model_history;
            if o.message.as_deref().is_some_and(|m| {
                m.contains("not usage history")
                    || m.contains("history unavailable")
                    || m.contains("activity counts steps, not tokens")
            }) {
                o.message = Some(format!(
                    "Account quotas and local token history over the last {} UTC days (ccusage method).",
                    crate::usage::RETAINED_USAGE_DAYS
                ));
            }
            Ok(o)
        }
        Err(base_err) => {
            let mut message = if summary.warnings.is_empty() {
                format!(
                    "Observed local token usage over the last {} UTC days (ccusage method).",
                    crate::usage::RETAINED_USAGE_DAYS
                )
            } else {
                summary.warnings.join(" ")
            };
            if base_err == crate::antigravity::KEYRING_LOCKED {
                message = format!("{base_err} {message}");
            }
            Ok(Observation {
                models: summary_models,
                usage: Some(summary.usage),
                history: summary.history,
                model_usage: summary.model_usage,
                model_history: summary.model_history,
                message: Some(message),
                ..Default::default()
            })
        }
    }
}

/// The injected home makes filesystem-boundary regression tests independent of real user data.
pub fn local_usage_observation(
    home: &std::path::Path,
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Observation, String> {
    local_usage_observation_with_roots(home, &[], id, now, &|| false)
}

fn local_usage_observation_with_roots(
    home: &std::path::Path,
    extra_roots: &[std::path::PathBuf],
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Observation, String> {
    if matches!(id, "opencode" | "antigravity" | "hermes") {
        let summary = read_db_usage(home, extra_roots, id, now, cancelled)?;
        return merge_local_usage(Err(String::new()), Ok(summary));
    }
    let files = crate::usage::read_session_files_with_roots(home, extra_roots, id, cancelled)?;
    if id == "codex" {
        let (summary, records) = crate::usage::codex_records(&files, now)?;
        let message = if summary.warnings.is_empty() {
            "Reported Codex rollout tokens (ccusage method).".into()
        } else {
            summary.warnings.join(" ")
        };
        return Ok(Observation {
            usage: Some(summary.usage),
            history: summary.history,
            model_usage: summary.model_usage,
            model_history: summary.model_history,
            message: Some(message),
            authentication: Some("notRequired".into()),
            codex_records: records,
            ..Default::default()
        });
    }
    match crate::usage::from_files(&files, id, now) {
        Ok(result) => {
            let models: Vec<crate::Model> = result
                .model_usage
                .iter()
                .map(|m| crate::Model {
                    id: m.model.clone(),
                    name: m.model.clone(),
                    size_bytes: None,
                    loaded: None,
                })
                .collect();
            let message = if result.warnings.is_empty() {
                format!(
                    "Observed local token usage over the last {} UTC days (ccusage method). Not account-wide or billing usage.",
                    crate::usage::RETAINED_USAGE_DAYS
                )
            } else {
                format!(
                    "Observed supported records only; incomplete coverage. {}",
                    result.warnings.join(" ")
                )
            };
            Ok(Observation {
                models,
                usage: Some(result.usage),
                history: result.history,
                model_usage: result.model_usage,
                model_history: result.model_history,
                partial_usage: None,
                message: Some(message),
                ..Default::default()
            })
        }
        Err(error) => {
            if id == "claude" {
                if let Ok((partial, models)) = crate::claude_inputs::from_files(&files, now) {
                    let message = partial.note.clone();
                    return Ok(Observation {
                        models,
                        partial_usage: Some(partial),
                        message: Some(message),
                        ..Default::default()
                    });
                }
            }
            Err(error)
        }
    }
}
