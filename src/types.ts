export type AgentKind = 'cloud' | 'local';
export interface Model { id: string; name: string; sizeBytes: number | null; loaded: boolean | null }
export interface Usage {
  inputTokens: number; outputTokens: number; cachedTokens: number; totalTokens: number;
  cacheCreationTokens?: number; cacheReadTokens?: number;
  spendUsd: number | null; source: string; scope: 'device' | 'account'; asOf: string;
  periodStart?: string | null; periodEnd?: string | null;
}
export interface DailyUsage {
  date: string; inputTokens: number; outputTokens: number; cachedTokens: number; totalTokens: number;
  cacheCreationTokens?: number; cacheReadTokens?: number; recordedResponses?: number;
}
export interface PartialUsage { inputTokens: number; cachedTokens: number; source: string; asOf: string; periodStart: string; periodEnd: string; note: string }
export interface Quota { id: string; label: string; remainingPercent: number | null; resetsAt: string | null; source: string }
export interface ModelDayUsage {
  date: string; model: string; totalTokens: number;
  inputTokens?: number; outputTokens?: number; cachedTokens?: number;
  cacheCreationTokens?: number; cacheReadTokens?: number;
}
export interface ContextInfo { mcps: { name: string; status: 'connected' | 'configured' | 'pending' | 'disabled' | 'failed' | 'needs_auth' | 'unknown' }[]; skills: string[]; note: string }
export interface SessionActivity {sessions:{id:string;state:'running'}[];source:string;scope:string;asOf:string}
export interface Agent {
  id: 'codex' | 'claude' | 'gemini' | 'antigravity' | 'opencode' | 'hermes' | 'ollama' | 'lmstudio';
  name: string; kind: AgentKind; installed: boolean; running: boolean | null;
  connected: boolean; status: 'detected' | 'ready' | 'unavailable' | 'error'; message: string;
  monitoringAllowed?: boolean; historyCached?: boolean;
  models: Model[]; usage: Usage | null; quotas: Quota[];
  reportedPeriodUsage?: Usage | null;
  savedPeriodUsage?: Usage | null;
  updatedAt: string | null; capabilities: string[];
  authentication?: 'unknown' | 'available' | 'required' | 'notRequired' | null;
  activeSessions?: SessionActivity | null;
  usageHistory?: DailyUsage[]; modelUsage?: { model: string; totalTokens: number }[]; partialUsage?: PartialUsage | null;
  modelHistory?: ModelDayUsage[]; context?: ContextInfo | null;
}
export interface BillingReport { provider: 'openai' | 'anthropic'; spendUsd: number | null; periodStart: string; periodEnd: string; asOf: string | null; message: string }
export interface AgentSubprocess { pid: number; task: string; memoryBytes: number; cpuPercent: number }
export interface AgentProcessGroup { agentId: Agent['id']; name: string; memoryBytes: number; cpuPercent: number; processes: AgentSubprocess[] }
export interface ProcessControlSnapshot { totalMemoryBytes: number; systemMemoryBytes: number; totalCpuPercent: number; agents: AgentProcessGroup[]; sampledAt: string }
export interface Snapshot { agents: Agent[]; scannedAt: string; platform: string; warnings: string[]; billing?: BillingReport[] }
export type KindFilter = 'all' | AgentKind;
export type Theme = 'light' | 'dark';
