# Desktop IPC Contract

Shared contract for independently implemented UI and backend. JSON keys are camelCase; all timestamps are RFC3339 UTC strings. Missing metrics are null, never fabricated zeroes.

```ts
type AgentKind = 'cloud' | 'local';
type AgentStatus = 'detected' | 'ready' | 'unavailable' | 'error';
interface Model { id: string; name: string; sizeBytes: number | null; loaded: boolean | null }
interface Usage {
  inputTokens: number; outputTokens: number; cachedTokens: number;
  totalTokens: number; spendUsd: number | null;
  source: string; scope: 'device' | 'account'; asOf: string;
  periodStart?: string | null; periodEnd?: string | null;
}
interface DailyUsage { date: string; inputTokens: number; outputTokens: number; cachedTokens: number; totalTokens: number; recordedResponses?: number }
interface PartialUsage { inputTokens: number; cachedTokens: number; source: string; asOf: string; periodStart: string; periodEnd: string; note: string }
interface Quota {
  id: string; label: string; remainingPercent: number | null;
  resetsAt: string | null; source: string;
}
interface Agent {
  id: 'codex' | 'claude' | 'gemini' | 'antigravity' | 'opencode' | 'hermes' | 'ollama' | 'lmstudio';
  name: string; kind: AgentKind; installed: boolean; running: boolean | null;
  connected: boolean; status: AgentStatus; message: string;
  monitoringAllowed?: boolean; historyCached?: boolean;
  models: Model[]; usage: Usage | null; quotas: Quota[];
  reportedPeriodUsage?: Usage | null; // Current successful OpenCode stats only. Not restored from cache.
  savedPeriodUsage?: Usage | null; // Previously verified OpenCode stats; never labeled live.
  updatedAt: string | null; capabilities: string[];
  usageHistory?: DailyUsage[]; modelUsage?: {model: string; totalTokens: number}[]; partialUsage?: PartialUsage | null;
  modelHistory?: {date: string; model: string; totalTokens: number; inputTokens?: number; outputTokens?: number}[];
  context?: {mcps: {name: string; status: string}[]; skills: string[]; note: string} | null;
}
interface BillingReport { provider: 'openai' | 'anthropic'; spendUsd: number | null; periodStart: string; periodEnd: string; asOf: string | null; message: string }
interface Snapshot { agents: Agent[]; scannedAt: string; platform: string; warnings: string[]; billing?: BillingReport[] }
```

Tauri commands:

- `scan_agents()` -> Snapshot. Installation discovery only; no usage files, credentials, or network before per-agent approval. Filter absent entries in backend and UI.
- Discovery additionally recognizes narrowly matched process names without requesting process arguments/environment. Known running local runtimes can appear without an executable on PATH. Missing process-name evidence is unknown, not proof of being stopped.
- `refresh_usage()` -> Snapshot. Refresh approved integrations only, using bounded non-inference operations. Serialize refreshes or reject overlapping work, and keep errors scoped to one agent.
- `set_permission({ agentId: string, allowed: boolean })` -> Snapshot. Grant/revoke remembered monitoring permission. A grant is explicit consent for the named integration's disclosed operations. Revoke immediately invalidates the named tool's in-flight observations, clears runtime metrics and deletes that tool's monitor-owned saved history and approval. Success waits for that tool's in-flight local scans to exit, even if a blocking OS operation cannot be interrupted. Native credentials and source transcripts remain unchanged. Storage failure rejects the command without claiming durable success; failed Revoke still invalidates in-flight observations and requests cancellation.
- `hide_to_tray()` -> void or an explicit unavailable error. Hide the main window only when a native tray handle exists, preserving an accessible Open/Quit path. No filesystem, account, shell or generation operation is exposed through this command.

The native backend owns the 45-second background refresh timer. It calls the same installation discovery and permission-gated Service/BillingStore entrypoints as the visible window. Closing hides only with a working tray; explicit Quit ends session access. There is no autostart registration. The UI does not receive unbound push snapshots; on becoming visible it reads the current state through the existing commands, retaining frontend revocation masks.

Implemented billing commands (separate permission from integration status):
- `connect_billing({ provider: 'openai' | 'anthropic', credential: string })` -> Snapshot. Explicit separately entered reporting credential, session memory only; never persist or log. Non-inference GET reporting, no budget mutation. UI must label permissions/limitations; status errors must not echo credentials.
  The returned snapshot acknowledges permission without waiting for provider requests. The UI follows it with `refresh_usage`; permission mutations must not queue behind that refresh.
- `disconnect_billing({ provider: 'openai' | 'anthropic' })` -> Snapshot. Clear session credentials and account totals.
- `read_process_control()` -> `{ totalMemoryBytes, systemMemoryBytes, totalCpuPercent, agents: [{ agentId, name, memoryBytes, cpuPercent, processes: [{ pid, task, memoryBytes, cpuPercent }] }], sampledAt }`. Live local RAM, CPU %, and sanitized descendant task names for installed, approved AI tools only (excluding auth-paused/revoked tools and the monitor's own process tree; no environment variables, prompts, credentials, or process termination).

Billing reports are month-to-date UTC organization-wide costs. They never populate device token counts or remaining subscription balances. OpenAI and Anthropic each have one report per session/provider, preventing per-app duplication. Billing refreshes at most every five minutes, status at most every 30 seconds. Incomplete paginated reports are rejected, not summed as a complete month. Anthropic uses the supported legacy Cost API and excludes Priority Tier.

Local token data covers recognized readable records from up to 30 UTC days for Claude Code and up to 90 for Gemini CLI. The store retains up to 90 UTC days of observed aggregates. Daily `recordedResponses` counts supported deduplicated model responses, not tasks; dated `modelHistory` rows may carry optional `inputTokens` and `outputTokens`. Missing fields in old saved rows are unknown, not zero. Saved transcript state is not a billed/lifetime ledger: deleted, compacted, rewound, invalid or unreadable records can be absent. `partialUsage` carries reliable Claude input/cache counters only; output/total cannot be inferred from these fields. Never add partial input counters to a complete token total or count cache twice. Ordinary Claude assistant output counts are placeholders. Complete timestamped SDK stream accounting is supported only where actually persisted/exported. `usageHistory` contains known days only; absent dates are unknown, never synthetic zeroes. Revocation clears all new fields and models as well as the original usage/quota fields.

Desktop-only build feature: `desktop`. Backend core tests must run with `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features` without GTK/WebKit. Frontend browser mode returns a real empty snapshot with a desktop-required notice, never demonstration agent data. No UI HTTP fetch or credential storage: call the constrained Tauri commands.

Desktop configuration uses `http://localhost:1420` in development, `../dist` as frontendDist, and fixed port 1420 in Vite. Required frontend scripts: `dev`, `build`, `check`, `test`, `desktop:dev`, `desktop:build`. Desktop scripts enable the Cargo `desktop` feature. App identifier: `com.local.aiusagemonitor`; product name: `EZTYN AI MONITOR`. The existing application identifier and internal Rust library name remain stable for compatibility; they are not display branding.

## Native authentication and active-session evidence

Agent adds optional `authentication` (`unknown`, `available`, `required`, `notRequired`, or null) and `activeSessions` (null when unsupported/unavailable). Missing fields from older snapshots are unknown, never a zero count or proof of login. Authentication evidence is independent from `connected`, which means a supported observation succeeded. `running` is only process/runtime evidence, not a session count.

`activeSessions` contains `sessions: [{id, state: "running"}]`, `source`, `scope`, and `asOf`. A validated complete empty list is a real zero within the stated source scope. No titles, prompts, project paths, account identifiers or credentials are included. Provider-wide token totals and recorded model history do not become active-session token consumption. Permission revocation clears these fields in both native publication and frontend masks; a failed refresh also discards current session/authentication evidence rather than relabeling stale observations as live.

Connect checks reusable native status only after consent, except explicit Gemini Connect starts the existing owned Google login/code modal and grants only on Finish and connect. Startup only restores saved monitoring grants and never starts native login. Only explicit native authentication-required evidence enables the separate Login action; a timeout, missing quota, unsupported API-key subscription balance, or malformed response does not. Local recorded metadata and runtime inventory require monitoring permission, not cloud login. Unsupported providers report Unavailable with a source-specific explanation; this feature adds no session instrumentation or tracking hooks.

`monitoringAllowed` represents the current explicit monitoring grant independently of `connected` (observation success). `historyCached` indicates previously observed token data retained/restored without current connection. Approved cached Usage remains visible offline with its original `updatedAt`; authentication, active sessions, models, quotas and Context are not restored. Normalized history and grants are stored in a versioned bounded local file, not a serialized Snapshot. Login pause preserves history but pauses the saved grant until explicit Connect. Missing-tool discovery does not delete saved data or approvals. Billing credentials/reports remain session-only.

## Quota and Context extension

The active native cloud integrations are Codex, Claude Code, Antigravity, OpenCode and Hermes. The Antigravity entry displays as Gemini in the UI, keeping `id: "antigravity"` and its native login routing. Its `Quota.label` contains `group · bucket · window`; `Quota.id` identifies the native bucket/window independently of display order. Tools groups these rows by their reported group, without filtering by model or window. A tool's **?** button discloses the original labels, resets, sources and connection help. Quotas appear only for approved, connected tools, never from cached usage history. Gemini transcript parsing remains an internal compatibility parser, not an active connector. Local Ollama and LM Studio remain supported without fictional subscription quotas.

- `modelHistory` preserves supported UTC date/model token pairs. An empty list means unavailable daily model history, not zero consumption. Never reconstruct date/model counts from aggregate model shares.
- `context` contains normalized names and status only. MCP states are `connected`, `configured`, `pending`, `disabled`, `failed`, `needs_auth`, `unknown`. Configuration alone never proves connectivity. Skill discovery does not imply execution or activation.
- Both fields follow existing session permission/generation guards and are cleared on revocation in Rust and masked against late frontend snapshots. Installation scanning must not read Context configuration or usage records. Never publish commands, environment values, URLs, credentials, private paths, raw errors or skill bodies.
- The main list displays quotas only. Usage and Context each select one installed tool. Do not add quotas across clients that might share a provider account. Missing consumption, reset time and history remain unavailable.

Antigravity reporting requires a recognized stable CLI version `>= 1.1.11` before `-p /usage --output-format json`. Only `command.data.groups[].buckets[]` supplies quota; `remaining_fraction` is a remaining allowance fraction. The envelope's `usage` counters are not historical consumption. Unknown/older versions must not receive the slash command because they can interpret it as a model prompt. No model, browser-cookie or internal-endpoint fallback is allowed.

## Authentication-only IPC

- `auth_methods({agentId})`: explicit native sign-in-method discovery; only installed integrations. Returns safe choice IDs, provider/method labels and flow kinds. Never returns native saved connections/credentials.
- `auth_start({agentId,choiceId})`: pause monitoring, wait for that tool's in-flight local scans to exit, validate a currently offered backend choice and create one owned native authentication attempt. Arbitrary executable paths/argv are not accepted.
- `auth_poll({attemptId})`: transient `attemptId`, `toolId`, `state`, `kind`, instructions/output, optional verified browser URL/device code. Never serialize this into `Snapshot`. States include preparing, waiting, authenticated (Codex verified only), finished (native process exit only), failed, cancelled, expired.
- `auth_submit({attemptId,code})`: a single bounded code to the active native terminal's stdin only; no argv, environment, storage or logging. Device-code attempts reject it.
- `auth_open_browser({attemptId})`: open only the active validated device challenge; accepts no URL argument.
- `auth_cancel({attemptId})`: cancel only the matching owned attempt, kill/reap its direct child and clear transient metadata. No logout or unrelated-process termination.

The native coordinator serializes authentication transitions and blocks background monitoring during authentication. Polling expires attempts after15minutes even if the UI is unresponsive. Completion does not silently regrant monitoring; use Connect again for the signed-in account. Existing native credentials may persist, unlike monitor permissions. See [sign-in behavior and limitations](integrations/authentication.md).
