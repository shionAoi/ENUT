use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub size_bytes: Option<u64>,
    pub loaded: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    pub total_tokens: u64,
    pub spend_usd: Option<f64>,
    pub source: String,
    pub scope: String,
    pub as_of: String,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    pub date: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_responses: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model: String,
    pub total_tokens: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDayUsage {
    pub date: String,
    pub model: String,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInfo {
    pub name: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInfo {
    pub mcps: Vec<McpInfo>,
    pub skills: Vec<String>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialUsage {
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub source: String,
    pub as_of: String,
    pub period_start: String,
    pub period_end: String,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub id: String,
    pub label: String,
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSession {
    pub id: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActivity {
    pub sessions: Vec<ActiveSession>,
    pub source: String,
    pub scope: String,
    pub as_of: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub installed: bool,
    pub running: Option<bool>,
    pub connected: bool,
    pub monitoring_allowed: bool,
    pub history_cached: bool,
    pub status: String,
    pub message: String,
    pub models: Vec<Model>,
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reported_period_usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_period_usage: Option<Usage>,
    pub quotas: Vec<Quota>,
    pub updated_at: Option<String>,
    pub capabilities: Vec<String>,
    pub usage_history: Vec<DailyUsage>,
    pub model_usage: Vec<ModelUsage>,
    pub partial_usage: Option<PartialUsage>,
    pub model_history: Vec<ModelDayUsage>,
    pub context: Option<ContextInfo>,
    pub authentication: Option<String>,
    pub active_sessions: Option<SessionActivity>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub agents: Vec<Agent>,
    pub scanned_at: String,
    pub platform: String,
    pub warnings: Vec<String>,
    pub billing: Vec<crate::billing::BillingReport>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSubprocess {
    pub pid: u32,
    pub task: String,
    pub memory_bytes: u64,
    pub cpu_percent: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProcessGroup {
    pub agent_id: String,
    pub name: String,
    pub memory_bytes: u64,
    pub cpu_percent: f64,
    pub processes: Vec<AgentSubprocess>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessControlSnapshot {
    pub total_memory_bytes: u64,
    pub system_memory_bytes: u64,
    pub total_cpu_percent: f64,
    pub agents: Vec<AgentProcessGroup>,
    pub sampled_at: String,
}

pub(crate) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
