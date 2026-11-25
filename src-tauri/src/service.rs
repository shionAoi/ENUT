use crate::history_store::{HistoryStore, SavedState, STORAGE_ERROR};
use crate::{
    dto::now,
    inventory::{StatusScan, STATUS_SCAN},
    Agent, Discovery, Snapshot, StatusReader, Usage,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

pub(crate) const AGENTS: [(&str, &str, bool); 8] = [
    ("codex", "Codex", false),
    ("claude", "Claude Code", false),
    ("antigravity", "Antigravity", false),
    ("gemini", "Gemini CLI", false),
    ("opencode", "OpenCode", false),
    ("hermes", "Hermes", false),
    ("ollama", "Ollama", true),
    ("lmstudio", "LM Studio", true),
];
pub(crate) fn known_agent(id: &str) -> bool {
    AGENTS.iter().any(|(agent, _, _)| *agent == id)
}
struct Permission {
    allowed: bool,
    generation: Arc<AtomicBool>,
}
impl Permission {
    fn invalidate(&mut self) {
        self.generation.store(true, Ordering::Release);
        self.generation = Default::default();
    }
}
struct State {
    snapshot: Snapshot,
    permissions: HashMap<String, Permission>,
    refreshed: HashMap<String, Instant>,
    authenticating: HashSet<String>,
    pending_auth: Option<(String, Arc<()>)>,
    saved: SavedState,
    store: Option<Result<HistoryStore, String>>,
}
pub struct Service<D: Discovery, R: StatusReader> {
    discovery: D,
    reader: R,
    state: Mutex<State>,
    refresh_lock: tokio::sync::Mutex<()>,
    status_locks: [Arc<tokio::sync::Mutex<()>>; AGENTS.len()],
    permission_changed: tokio::sync::watch::Sender<()>,
}

impl State {
    fn cancel_pending_auth_for(&mut self, tool: &str) {
        if self.pending_auth.as_ref().is_some_and(|(id, _)| id == tool) {
            self.pending_auth = None;
        }
    }

    fn auth_busy(&self, tool: &str) -> bool {
        self.authenticating.contains(tool)
            || self.pending_auth.as_ref().is_some_and(|(id, _)| id == tool)
    }
}

pub struct PendingAuth<'a> {
    state: &'a Mutex<State>,
    identity: Arc<()>,
}
impl PendingAuth<'_> {
    fn matches(&self, state: &State) -> bool {
        state
            .pending_auth
            .as_ref()
            .is_some_and(|(_, identity)| Arc::ptr_eq(identity, &self.identity))
    }
    /// Claim the current reservation for launch while holding the command's auth gate.
    pub fn claim(self) -> bool {
        let mut state = self.state.lock().unwrap();
        if self.matches(&state) {
            let (id, _) = state.pending_auth.take().unwrap();
            state.authenticating.insert(id);
            true
        } else {
            false
        }
    }
}
impl Drop for PendingAuth<'_> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        if self.matches(&state) {
            state.pending_auth = None;
        }
    }
}

impl<D: Discovery, R: StatusReader> Service<D, R> {
    pub fn new(discovery: D, reader: R) -> Self {
        Self { discovery, reader, state: Mutex::new(State {
            snapshot: Snapshot { agents: vec![], scanned_at: now(), platform: std::env::consts::OS.into(), warnings: vec![
                "Monitoring permissions and normalized usage history are saved locally until Revoke. Local token metrics follow the ccusage read-only log/database method (uncached input, output, cache write/read). Subscription quotas remain unavailable where no supported interface exists. API billing needs separate session-only reporting permission.".into(),
            ], billing: vec![] }, permissions: HashMap::new(), refreshed: HashMap::new(), authenticating: HashSet::new(), pending_auth: None, saved: SavedState::default(), store: None,
        }), refresh_lock: tokio::sync::Mutex::new(()), status_locks: Default::default(), permission_changed: tokio::sync::watch::channel(()).0 }
    }
    pub fn with_storage(discovery: D, reader: R, path: &std::path::Path) -> Self {
        let service = Self::new(discovery, reader);
        {
            let mut state = service.state.lock().unwrap();
            match HistoryStore::load(path) {
                Ok((store, saved)) => {
                    for (id, tool) in &saved.tools {
                        state.permissions.insert(
                            id.clone(),
                            Permission {
                                allowed: tool.allowed,
                                generation: Default::default(),
                            },
                        );
                    }
                    state.saved = saved;
                    state.store = Some(Ok(store));
                }
                Err(error) => {
                    state.snapshot.warnings.push(error.clone());
                    state.store = Some(Err(error));
                }
            }
        }
        service
    }
    fn persist(state: &mut State, candidate: SavedState) -> Result<(), String> {
        let result = match &state.store {
            None => Ok(()),
            Some(Ok(store)) => store.save(&candidate),
            Some(Err(error)) => Err(error.clone()),
        };
        if result.is_err() {
            if !state.snapshot.warnings.iter().any(|w| w == STORAGE_ERROR) {
                state.snapshot.warnings.push(STORAGE_ERROR.into());
            }
        } else {
            state.snapshot.warnings.retain(|w| w != STORAGE_ERROR);
            state.saved = candidate;
        }
        result
    }
    fn expire_history(state: &mut State) {
        let mut saved = state.saved.clone();
        let changed = match saved.prune(chrono::Utc::now().date_naive()) {
            Ok(changed) if changed.is_empty() => return,
            Ok(changed) => changed,
            Err(_) => {
                if !state.snapshot.warnings.iter().any(|w| w == STORAGE_ERROR) {
                    state.snapshot.warnings.push(STORAGE_ERROR.into());
                }
                return;
            }
        };
        for agent in &mut state.snapshot.agents {
            if agent.monitoring_allowed && changed.contains(&agent.id) {
                saved.tools[&agent.id].apply(agent);
                agent.history_cached &= !agent.connected;
            }
        }
        // Keep the previous saved state on write failure so the next scan can retry.
        let _ = Self::persist(state, saved);
    }
    fn snapshot(&self) -> Snapshot {
        let mut state = self.state.lock().unwrap();
        Self::expire_history(&mut state);
        state.snapshot.clone()
    }

    /// Publish current normalized state synchronously under the revocation lock.
    /// Callers must not await or re-enter this service from the callback.
    pub fn with_snapshot<T>(&self, publish: impl FnOnce(&Snapshot) -> T) -> T {
        let state = self.state.lock().unwrap();
        publish(&state.snapshot)
    }

    pub fn approved_installed_agents(&self) -> HashSet<String> {
        let state = self.state.lock().unwrap();
        state
            .snapshot
            .agents
            .iter()
            .filter(|a| {
                a.installed
                    && !state.auth_busy(&a.id)
                    && state.permissions.get(&a.id).is_some_and(|p| p.allowed)
            })
            .map(|a| a.id.clone())
            .collect()
    }

    pub fn begin_auth(&self, id: &str) -> Result<Snapshot, String> {
        let mut state = self.state.lock().unwrap();
        self.pause_auth(&mut state, id)?;
        state.authenticating.insert(id.into());
        Ok(state.snapshot.clone())
    }

    pub fn prepare_auth(&self, id: &str) -> Result<PendingAuth<'_>, String> {
        let mut state = self.state.lock().unwrap();
        state.pending_auth = None;
        self.pause_auth(&mut state, id)?;
        let identity = Arc::new(());
        state.pending_auth = Some((id.into(), identity.clone()));
        Ok(PendingAuth {
            state: &self.state,
            identity,
        })
    }

    fn pause_auth(&self, state: &mut State, id: &str) -> Result<(), String> {
        if !state
            .snapshot
            .agents
            .iter()
            .any(|a| a.id == id && a.installed && a.kind == "cloud")
        {
            return Err("An installed cloud integration is required for sign-in.".into());
        }
        let mut saved = state.saved.clone();
        if let Some(tool) = saved.tools.get_mut(id) {
            tool.allowed = false;
        }
        // Cancel current access even when durable pause fails; never start auth then.
        if let Some(p) = state.permissions.get_mut(id) {
            p.allowed = false;
            p.invalidate();
        }
        self.permission_changed.send_modify(|_| {});
        state.refreshed.remove(id);
        if let Some(agent) = state.snapshot.agents.iter_mut().find(|a| a.id == id) {
            clear_runtime(agent);
            agent.monitoring_allowed = false;
            agent.message = "Monitoring paused for native sign-in. Saved history is retained; finish and connect to resume.".into();
        }
        Self::persist(state, saved)?;
        Ok(())
    }
    pub fn end_auth(&self, id: &str) {
        self.state.lock().unwrap().authenticating.remove(id);
    }
    fn status_lock(&self, id: &str) -> Option<&Arc<tokio::sync::Mutex<()>>> {
        AGENTS
            .iter()
            .position(|(agent, _, _)| *agent == id)
            .map(|index| &self.status_locks[index])
    }
    pub async fn wait_for_status_idle(&self, id: &str) {
        if let Some(status) = self.status_lock(id) {
            let _idle = status.lock().await;
        }
    }

    pub fn scan_agents(&self) -> Snapshot {
        let found: Vec<_> = AGENTS
            .iter()
            .filter_map(|(id, name, local)| {
                let running = self.discovery.running(id);
                (self.discovery.installed(id) || running == Some(true))
                    .then_some((*id, *name, *local, running))
            })
            .collect();
        let mut state = self.state.lock().unwrap();
        Self::expire_history(&mut state);
        let removed: Vec<_> = state
            .snapshot
            .agents
            .iter()
            .filter(|agent| !found.iter().any(|(id, _, _, _)| *id == agent.id))
            .map(|agent| agent.id.clone())
            .collect();
        for id in &removed {
            state.cancel_pending_auth_for(id);
            if let Some(permission) = state.permissions.get_mut(id) {
                permission.invalidate();
            }
            state.refreshed.remove(id);
        }
        if !removed.is_empty() {
            self.permission_changed.send_modify(|_| {});
        }
        state
            .snapshot
            .agents
            .retain(|agent| !removed.contains(&agent.id));
        // Keep approved runtime evidence until refreshed; scanning never reads usage.
        for (id, name, local, running) in found {
            if let Some(agent) = state.snapshot.agents.iter_mut().find(|a| a.id == id) {
                agent.running = running;
                continue;
            }
            let allowed = state.permissions.get(id).is_some_and(|p| p.allowed);
            state.snapshot.agents.push(Agent {
                id: id.into(), name: name.into(), kind: if local { "local" } else { "cloud" }.into(),
                installed: true, running, connected: false, status: "detected".into(),
                monitoring_allowed: allowed, history_cached: false,
                message: match id {
                    "codex" => "Approve Codex account-status, local Context metadata, and read-only local rollout token usage (ccusage method). The CLI may refresh its saved sign-in; no model turn is started.",
                    "antigravity" => "Approve version-gated Antigravity quota reporting, local Context metadata, and read-only conversation token usage (ccusage method). No model turn is started.",
                    "gemini" => "Approve local Gemini CLI transcript metadata for token history (ccusage method). No login, provider quotas, or model turn is started.",
                    "opencode" => "Approve OpenCode status APIs and read-only local session database token history (ccusage method) for supported limits, recorded tokens, MCP status and skill names.",
                    "hermes" => "Approve Hermes provider quota reporting, local Context metadata, and read-only state.db token history (ccusage method). No chat session is started.",
                    _ if local => "Approval permits only GET model inventory at the standard loopback endpoint. No inference, credentials, or usage logs.",
                    _ => "Approve read-only local session token history (ccusage method) and Context metadata. Subscription credentials are never collected by this monitor.",
                }.into(),
                models: vec![], usage: None, reported_period_usage: None, saved_period_usage: None, quotas: vec![], updated_at: None,
                capabilities: if local { vec!["modelInventory".into()] } else if matches!(id, "codex" | "antigravity" | "hermes" | "opencode") { vec!["quota".into(),"localUsage".into(),"history".into(),"context".into()] } else if id == "gemini" { vec!["localUsage".into(),"history".into()] } else { vec!["localUsage".into(),"history".into(),"context".into()] },
                usage_history: vec![], model_usage: vec![], partial_usage:None, model_history:vec![], context:None, authentication:None, active_sessions:None,
            });
            if allowed {
                if let Some(saved) = state.saved.tools.get(id).cloned() {
                    saved.apply(state.snapshot.agents.last_mut().unwrap());
                }
            }
        }
        state.snapshot.scanned_at = now();
        state.snapshot.clone()
    }

    pub fn set_permission(&self, agent_id: &str, allowed: bool) -> Result<Snapshot, String> {
        if !known_agent(agent_id) {
            return Err("Unknown integration.".into());
        }
        let mut state = self.state.lock().unwrap();
        if allowed && state.auth_busy(agent_id) {
            return Err(
                "Finish or cancel native sign-in before granting monitoring access.".into(),
            );
        }
        Self::expire_history(&mut state);
        let mut saved = state.saved.clone();
        if allowed {
            saved.tools.entry(agent_id.into()).or_default().allowed = true;
            Self::persist(&mut state, saved)?;
        } else {
            state.cancel_pending_auth_for(agent_id);
            saved.tools.remove(agent_id);
            // Keep the deletion intent even if the disk refuses it.
            state.saved = saved;
        }
        let permission = state
            .permissions
            .entry(agent_id.into())
            .or_insert(Permission {
                allowed: false,
                generation: Default::default(),
            });
        permission.allowed = allowed;
        permission.invalidate();
        self.permission_changed.send_modify(|_| {});
        state.refreshed.remove(agent_id);
        if let Some(agent) = state.snapshot.agents.iter_mut().find(|a| a.id == agent_id) {
            clear_runtime(agent);
            agent.monitoring_allowed = allowed;
            agent.message = if allowed {
                "Monitoring approval remembered on this device. Refresh to check supported status."
            } else {
                "Permission revoked. Saved monitor history and remembered approval deleted; native credentials are unchanged."
            }
            .into();
        }
        if allowed {
            if let Some(saved) = state.saved.tools.get(agent_id).cloned() {
                if let Some(agent) = state.snapshot.agents.iter_mut().find(|a| a.id == agent_id) {
                    saved.apply(agent);
                }
            }
        } else {
            let candidate = state.saved.clone();
            if let Err(error) = Self::persist(&mut state, candidate) {
                if let Some(agent) = state.snapshot.agents.iter_mut().find(|a| a.id == agent_id) {
                    agent.message = error.clone();
                }
                return Err(error);
            }
        }
        Ok(state.snapshot.clone())
    }

    pub async fn refresh_usage(&self) -> Snapshot {
        self.refresh_usage_with(|| {}).await
    }

    /// Notify the caller after each applied observation, outside the state lock.
    pub async fn refresh_usage_with(&self, mut applied: impl FnMut()) -> Snapshot {
        // Serialize asynchronous work without holding a synchronous mutex across await.
        let _refresh = self.refresh_lock.lock().await;
        let work: Vec<_> = {
            let mut state = self.state.lock().unwrap();
            Self::expire_history(&mut state);
            state
                .snapshot
                .agents
                .iter()
                .filter_map(|a| {
                    if state.auth_busy(&a.id) {
                        return None;
                    }
                    let p = state.permissions.get(&a.id)?;
                    let recent = state
                        .refreshed
                        .get(&a.id)
                        .is_some_and(|t| t.elapsed().as_secs() < 30);
                    (p.allowed && !recent).then(|| (a.id.clone(), p.generation.clone()))
                })
                .collect()
        };
        for (id, generation) in work {
            let result = {
                let Some(status) = self.status_lock(&id) else {
                    continue;
                };
                // A detached same-tool scan must not stall the other integrations.
                let Ok(guard) = status.clone().try_lock_owned() else {
                    continue;
                };
                let mut changes = self.permission_changed.subscribe();
                // A preceding connector may have been waiting while this permission changed.
                if !self.current(&id, &generation) {
                    continue;
                }
                let seconds = if matches!(id.as_str(), "antigravity" | "opencode" | "hermes") {
                    30
                } else {
                    5
                };
                let read = tokio::time::timeout(
                    std::time::Duration::from_secs(seconds),
                    STATUS_SCAN.scope(
                        Arc::new(StatusScan {
                            _guard: guard,
                            invalidated: generation.clone(),
                        }),
                        async { self.reader.read(id.clone()).await },
                    ),
                );
                tokio::pin!(read);
                // Cancellation covers the whole connector, including follow-up status
                // commands and Context reads, not just publication of its snapshot.
                loop {
                    tokio::select! {
                        biased;
                        changed = changes.changed() => {
                            if changed.is_err() || !self.current(&id, &generation) {
                                break Err("Status access cancelled after permission changed.".into());
                            }
                        }
                        result = &mut read => break result.unwrap_or_else(|_| Err("Status request timed out.".into())),
                    }
                }
            };
            let mut state = self.state.lock().unwrap();
            if !state
                .permissions
                .get(&id)
                .is_some_and(|p| p.allowed && Arc::ptr_eq(&p.generation, &generation))
            {
                continue;
            }
            state.refreshed.insert(id.clone(), Instant::now());
            let persistent = state.store.is_some();
            let Some(agent) = state.snapshot.agents.iter_mut().find(|a| a.id == id) else {
                continue;
            };
            match result {
                Ok(observation) => {
                    agent.authentication = observation.authentication.or_else(|| {
                        if agent.kind == "local" || matches!(agent.id.as_str(), "claude" | "gemini") {
                            Some("notRequired".into())
                        } else if !observation.quotas.is_empty() {
                            Some("available".into())
                        } else {
                            None
                        }
                    });
                    agent.active_sessions = observation.active_sessions;
                    agent.models = observation.models;
                    agent.quotas = observation.quotas;
                    // Only this observation can attest provenance, not a restored cache row.
                    // When OpenCode provides dated local history, keep dated usage instead of undated period fallback.
                    agent.reported_period_usage = observation.usage.as_ref()
                        .filter(|u| id == "opencode" && observation.history.is_empty() && u.scope == "device" && u.period_start.is_some() && u.period_end.is_some()
                            && agent.authentication.as_deref() != Some("required"))
                        .map(|u| Usage { spend_usd: None, ..u.clone() });
                    agent.usage = observation.usage;
                    agent.usage_history = observation.history;
                    agent.model_usage = observation.model_usage;
                    agent.partial_usage = observation.partial_usage;
                    agent.model_history = observation.model_history;
                    agent.context = observation.context;
                    agent.running = if agent.kind == "local" {
                        Some(true)
                    } else {
                        agent.running
                    };
                    agent.connected = true;
                    agent.status = "ready".into();
                    agent.updated_at = Some(now());
                    agent.message = observation.message.unwrap_or_else(||if agent.id == "codex" { "Provider-reported subscription windows and local rollout usage (ccusage method). API billing requires separate organization reporting." } else { "Reported local inventory only. Token usage and cloud spend are unavailable." }.into());
                    if persistent {
                        let observed = agent.clone();
                        let mut saved = state.saved.clone();
                        let tool = saved.tools.entry(id.clone()).or_default();
                        tool.allowed = true;
                        match tool.merge(&observed, observation.codex_records) {
                            Ok(()) => {
                                let history = tool.clone();
                                // Memory still retains the last observation when a write fails.
                                let result = Self::persist(&mut state, saved.clone());
                                state.saved = saved;
                                let agent = state
                                    .snapshot
                                    .agents
                                    .iter_mut()
                                    .find(|a| a.id == id)
                                    .unwrap();
                                history.apply(agent);
                                agent.history_cached = false;
                                if result.is_err() {
                                    agent.message = STORAGE_ERROR.into();
                                }
                            }
                            Err(_) => {
                                if !state.snapshot.warnings.iter().any(|w| w == STORAGE_ERROR) {
                                    state.snapshot.warnings.push(STORAGE_ERROR.into());
                                }
                            }
                        }
                    }
                }
                Err(message) => {
                    // Failed refresh cannot keep claiming a session is currently running
                    // or that old authentication evidence is still valid.
                    agent.active_sessions = None;
                    agent.authentication = None;
                    agent.reported_period_usage = None;
                    agent.status = "unavailable".into();
                    agent.connected = false;
                    agent.history_cached = agent.usage.is_some()
                        || agent.partial_usage.is_some()
                        || !agent.usage_history.is_empty()
                        || !agent.model_history.is_empty();
                    // A failed HTTP request does not prove that a process is stopped.
                    agent.running = None;
                    agent.message = message;
                    // Keep last successful observation with its original freshness.
                }
            }
            drop(state);
            applied();
        }
        self.snapshot()
    }
    fn current(&self, id: &str, generation: &Arc<AtomicBool>) -> bool {
        let state = self.state.lock().unwrap();
        !state.auth_busy(id)
            && state
                .permissions
                .get(id)
                .is_some_and(|p| p.allowed && Arc::ptr_eq(&p.generation, generation))
    }
}
fn clear_runtime(agent: &mut Agent) {
    agent.connected = false;
    agent.models.clear();
    agent.usage = None;
    agent.reported_period_usage = None;
    agent.saved_period_usage = None;
    agent.quotas.clear();
    agent.usage_history.clear();
    agent.model_usage.clear();
    agent.partial_usage = None;
    agent.model_history.clear();
    agent.context = None;
    agent.authentication = None;
    agent.active_sessions = None;
    agent.updated_at = None;
    agent.running = None;
    agent.history_cached = false;
    agent.status = "detected".into();
}
