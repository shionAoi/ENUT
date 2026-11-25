//! Credential-free live local process RAM/CPU sampling scoped to approved AI agents.
use crate::dto::{now, AgentProcessGroup, AgentSubprocess, ProcessControlSnapshot};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    time::{Duration, Instant},
};
use sysinfo::{MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const MAX_TASK_CHARS: usize = 40;
const MIN_SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const INITIAL_CPU_BASELINE_DELAY: Duration = Duration::from_millis(220);

#[derive(Clone, Debug)]
pub struct RawProcess {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub cmd: Vec<String>,
    pub memory_bytes: u64,
    pub cpu_usage_raw: f32,
}

fn clean_exe_name(raw: &str) -> String {
    let base = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(raw);
    let lower = base.to_ascii_lowercase();
    if lower.ends_with(".exe") && base.len() >= 4 {
        base[..base.len() - 4].to_string()
    } else {
        base.to_string()
    }
}

fn is_wrapper_runtime(name: &str) -> bool {
    let lower = clean_exe_name(name).to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "node" | "nodejs" | "bun" | "deno" | "python" | "python3"
    ) || lower.starts_with("python3.")
}

fn match_vendor_binary(name: &str) -> Option<&'static str> {
    let lower = clean_exe_name(name).to_ascii_lowercase();
    if lower.starts_with("codex-")
        && [
            "x86_64",
            "aarch64",
            "arm64",
            "linux",
            "darwin",
            "win",
            "pc-windows",
            "apple-darwin",
            "unknown-linux",
        ]
        .iter()
        .any(|tag| lower.contains(tag))
    {
        return Some("codex");
    }
    None
}

fn match_script_arg(arg: &str) -> Option<&'static str> {
    let norm = arg.replace('\\', "/").to_ascii_lowercase();
    let file = norm.rsplit('/').next().unwrap_or("");
    let stem = file
        .strip_suffix(".mjs")
        .or_else(|| file.strip_suffix(".cjs"))
        .or_else(|| file.strip_suffix(".js"))
        .or_else(|| file.strip_suffix(".py"))
        .unwrap_or(file);
    if norm.contains("/@anthropic-ai/claude-code/")
        || norm.contains("/claude-code/")
        || stem == "claude"
    {
        return Some("claude");
    }
    if norm.contains("/@openai/codex/") || stem == "codex" {
        return Some("codex");
    }
    if norm.contains("/@google/gemini-cli/") || norm.contains("/gemini-cli/") || stem == "gemini" {
        return Some("gemini");
    }
    if norm.contains("/opencode/") || stem == "opencode" {
        return Some("opencode");
    }
    if norm.contains("/.hermes/") || norm.contains("/hermes-agent/") || stem == "hermes" {
        return Some("hermes");
    }
    if norm.contains("/antigravity/") || stem == "agy" || stem == "antigravity" {
        return Some("antigravity");
    }
    None
}

pub fn direct_agent(proc: &RawProcess) -> Option<&'static str> {
    for &(id, _, _) in &crate::service::AGENTS {
        if crate::discovery::matches_process(id, &proc.name) {
            return Some(id);
        }
    }
    if let Some(id) = match_vendor_binary(&proc.name) {
        return Some(id);
    }
    if is_wrapper_runtime(&proc.name) {
        for arg in proc.cmd.iter().skip(1) {
            if arg.is_empty() || arg.starts_with('-') {
                continue;
            }
            return match_script_arg(arg);
        }
    }
    None
}

fn is_sensitive_flag(arg: &str) -> bool {
    if arg == "-H" || arg == "-k" {
        return true;
    }
    let lower = arg.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "--key"
            | "--api-key"
            | "--apikey"
            | "--token"
            | "--access-token"
            | "--refresh-token"
            | "--password"
            | "--passwd"
            | "--secret"
            | "--auth"
            | "--authorization"
            | "--bearer"
            | "--header"
    )
}

fn is_secret_prefix(token: &str) -> bool {
    ["sk-", "ghp_", "gho_", "github_pat_", "xoxb-", "AIza", "eyJ"]
        .iter()
        .any(|prefix| token.starts_with(prefix))
}

fn sanitize_url(token: &str) -> Option<String> {
    let scheme_end = token.find("://")?;
    let scheme = &token[..scheme_end];
    let rest = &token[scheme_end + 3..];
    let no_query = rest.split(['?', '#']).next().unwrap_or(rest);
    let authority_end = no_query.find('/').unwrap_or(no_query.len());
    let authority = &no_query[..authority_end];
    let path = &no_query[authority_end..];
    let host = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}{path}"))
}

fn sanitize_arg(token: &str) -> Option<String> {
    let trimmed = token.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return None;
    }
    if trimmed.contains('=') {
        let (lhs, _) = trimmed.split_once('=')?;
        if is_sensitive_flag(lhs) || !lhs.starts_with('-') {
            return None;
        }
        return Some(lhs.to_string());
    }
    if trimmed.contains("://") {
        return sanitize_url(trimmed);
    }
    if is_secret_prefix(trimmed) {
        return None;
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        let last = trimmed.rsplit(['/', '\\']).find(|s| !s.is_empty())?;
        return Some(last.to_string());
    }
    if trimmed.len() > 28 && !trimmed.starts_with('-') {
        return None;
    }
    Some(trimmed.to_string())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let prefix: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{prefix}…")
}

pub fn sanitize_task_label(name: &str, cmd: &[String]) -> String {
    let exe = cmd
        .first()
        .filter(|s| !s.trim().is_empty())
        .map(|s| clean_exe_name(s))
        .unwrap_or_else(|| clean_exe_name(name));
    let mut parts = Vec::new();
    if !exe.is_empty() {
        parts.push(exe);
    }
    let mut skip_next = false;
    for arg in cmd.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if is_sensitive_flag(arg) {
            skip_next = true;
            continue;
        }
        if let Some(clean) = sanitize_arg(arg) {
            if parts.len() == 1 && clean.eq_ignore_ascii_case(&parts[0]) {
                continue;
            }
            parts.push(clean);
            if parts.len() >= 4 {
                break;
            }
        }
    }
    let joined = parts.join(" ");
    let final_label = if joined.is_empty() {
        clean_exe_name(name)
    } else {
        joined
    };
    let final_label = if final_label.is_empty() {
        "process".to_string()
    } else {
        final_label
    };
    truncate_chars(&final_label, MAX_TASK_CHARS)
}

pub fn build_process_snapshot(
    processes: &[RawProcess],
    system_memory_bytes: u64,
    logical_cpus: usize,
    approved_agents: &HashSet<String>,
    self_pid: u32,
) -> ProcessControlSnapshot {
    let cpus = logical_cpus.max(1) as f32;
    let mut by_pid: HashMap<u32, &RawProcess> = HashMap::with_capacity(processes.len());
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for proc in processes {
        by_pid.insert(proc.pid, proc);
        if let Some(ppid) = proc.parent_pid {
            if ppid != proc.pid {
                children.entry(ppid).or_default().push(proc.pid);
            }
        }
    }

    // Exclude the monitor itself and any descendant CLI probes spawned by it.
    let mut excluded = HashSet::new();
    let mut queue = VecDeque::new();
    if by_pid.contains_key(&self_pid) {
        excluded.insert(self_pid);
        queue.push_back(self_pid);
        while let Some(pid) = queue.pop_front() {
            if let Some(kids) = children.get(&pid) {
                for &kid in kids {
                    if excluded.insert(kid) {
                        queue.push_back(kid);
                    }
                }
            }
        }
    }

    let mut direct: HashMap<u32, &'static str> = HashMap::new();
    for proc in processes {
        if excluded.contains(&proc.pid) {
            continue;
        }
        if let Some(agent_id) = direct_agent(proc) {
            direct.insert(proc.pid, agent_id);
        }
    }

    let mut global_claimed = HashSet::new();
    let mut groups = Vec::new();
    let mut total_memory_bytes: u64 = 0;
    let mut total_cpu_percent: f64 = 0.0;

    for &(agent_id, agent_name, _) in &crate::service::AGENTS {
        if !approved_agents.contains(agent_id) {
            continue;
        }
        let mut agent_queue = VecDeque::new();
        let mut claimed_for_agent = HashSet::new();
        for proc in processes {
            if excluded.contains(&proc.pid) || global_claimed.contains(&proc.pid) {
                continue;
            }
            if direct.get(&proc.pid) != Some(&agent_id) {
                continue;
            }
            let same_agent_parent = proc
                .parent_pid
                .and_then(|ppid| direct.get(&ppid))
                .is_some_and(|parent_agent| *parent_agent == agent_id);
            if !same_agent_parent && claimed_for_agent.insert(proc.pid) {
                agent_queue.push_back(proc.pid);
            }
        }
        while let Some(pid) = agent_queue.pop_front() {
            if !global_claimed.insert(pid) {
                continue;
            }
            if let Some(kids) = children.get(&pid) {
                for &kid in kids {
                    if excluded.contains(&kid) || global_claimed.contains(&kid) {
                        continue;
                    }
                    if direct.get(&kid).is_some_and(|other| *other != agent_id) {
                        continue;
                    }
                    if claimed_for_agent.insert(kid) {
                        agent_queue.push_back(kid);
                    }
                }
            }
        }

        if claimed_for_agent.is_empty() {
            continue;
        }

        let mut group_memory: u64 = 0;
        let mut group_cpu: f64 = 0.0;
        let mut group_processes = Vec::with_capacity(claimed_for_agent.len());
        for pid in claimed_for_agent {
            let Some(proc) = by_pid.get(&pid) else {
                continue;
            };
            let raw_cpu = if proc.cpu_usage_raw.is_finite() && proc.cpu_usage_raw >= 0.0 {
                proc.cpu_usage_raw
            } else {
                0.0
            };
            let cpu_percent = ((raw_cpu / cpus) as f64).clamp(0.0, 100.0);
            group_memory = group_memory.saturating_add(proc.memory_bytes);
            group_cpu += cpu_percent;
            group_processes.push(AgentSubprocess {
                pid: proc.pid,
                task: sanitize_task_label(&proc.name, &proc.cmd),
                memory_bytes: proc.memory_bytes,
                cpu_percent,
            });
        }
        group_processes.sort_by(|a, b| {
            b.memory_bytes
                .cmp(&a.memory_bytes)
                .then_with(|| a.pid.cmp(&b.pid))
        });
        let group_cpu = group_cpu.clamp(0.0, 100.0);
        total_memory_bytes = total_memory_bytes.saturating_add(group_memory);
        total_cpu_percent += group_cpu;
        groups.push(AgentProcessGroup {
            agent_id: agent_id.into(),
            name: agent_name.into(),
            memory_bytes: group_memory,
            cpu_percent: group_cpu,
            processes: group_processes,
        });
    }

    ProcessControlSnapshot {
        total_memory_bytes,
        system_memory_bytes,
        total_cpu_percent: total_cpu_percent.clamp(0.0, 100.0),
        agents: groups,
        sampled_at: now(),
    }
}

struct SamplerState {
    system: System,
    last_refresh: Option<Instant>,
    logical_cpus: usize,
}

impl Default for SamplerState {
    fn default() -> Self {
        let logical_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self {
            system: System::new(),
            last_refresh: None,
            logical_cpus,
        }
    }
}

#[derive(Default)]
pub struct ProcessMonitor {
    inner: tokio::sync::Mutex<SamplerState>,
}

impl ProcessMonitor {
    pub async fn sample(&self, approved_agents: HashSet<String>) -> ProcessControlSnapshot {
        let mut guard = self.inner.lock().await;
        guard
            .system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let system_memory_bytes = guard.system.total_memory();
        if approved_agents.is_empty() {
            return ProcessControlSnapshot {
                total_memory_bytes: 0,
                system_memory_bytes,
                total_cpu_percent: 0.0,
                agents: vec![],
                sampled_at: now(),
            };
        }

        let refresh = ProcessRefreshKind::nothing()
            .without_tasks()
            .with_memory()
            .with_cpu()
            .with_cmd(UpdateKind::OnlyIfNotSet);

        if guard.last_refresh.is_none() {
            guard
                .system
                .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
            tokio::time::sleep(INITIAL_CPU_BASELINE_DELAY).await;
            guard
                .system
                .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
            guard.last_refresh = Some(Instant::now());
        } else if guard
            .last_refresh
            .is_some_and(|when| when.elapsed() >= MIN_SAMPLE_INTERVAL)
        {
            guard
                .system
                .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
            guard.last_refresh = Some(Instant::now());
        }

        let processes: Vec<RawProcess> = guard
            .system
            .processes()
            .iter()
            .map(|(pid, proc)| RawProcess {
                pid: pid.as_u32(),
                parent_pid: proc.parent().map(|p| p.as_u32()),
                name: proc.name().to_string_lossy().into_owned(),
                cmd: proc
                    .cmd()
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect(),
                memory_bytes: proc.memory(),
                cpu_usage_raw: proc.cpu_usage(),
            })
            .collect();

        build_process_snapshot(
            &processes,
            system_memory_bytes,
            guard.logical_cpus,
            &approved_agents,
            std::process::id(),
        )
    }
}
