# Gemini CLI local usage

This connector parses **observed device tokens**, not subscription quota, reset
times, credit, or spend. Those account metrics remain unavailable. No inference,
account calls, internal Google quota endpoints, or configuration changes occur.

## Consent and privacy

Select **Connect** on **Gemini CLI** to opt in to reading transcript metadata.
This is separate from the **Gemini** Antigravity quota connector and never starts
login. Usage covers eligible records from the last 90 UTC days. Revoking Gemini
CLI access discards pending results, removes its saved aggregates, and blocks new
scans. The
parser itself accepts text only: no filesystem, environment, processes, or network.
It returns only bounded IDs/models, timestamps and counters, with generic errors
and warnings. It does not return or retain prompts, thoughts, tool results,
summaries, project paths, or credentials. Raw JSON necessarily contains private
conversation text in memory during parsing; never log or persist that text.
Development tests use synthetic data, not installed CLI sessions.

## Locations and format compatibility

The normal location is `~/.gemini/tmp/<project>/chats/` (Windows uses the user's
home directory). Older project directories use hashes; current storage uses
registry-assigned project identifiers/slugs. Current subagent files can be nested
under `chats/<parent-session-id>/`. Home overrides and sandbox runtime roots may
differ; this parser does not discover them or authorize broader scanning.

The caller owns bounded, approved directory reads, symlink safety and file
selection. Legacy `session-*.json` contains a conversation object with `messages`;
current `.jsonl` contains metadata, full message updates and control records.
Migration can leave both twins: **prefer JSONL**, never sum both. Supply a stable,
non-private `session_hint` common to twins. IDs are namespaced as `hint:message-id`
because upstream accepts caller-provided IDs as well as generated UUIDs.

Synthetic legacy shape (not user data):

```json
{"sessionId":"synthetic","projectHash":"synthetic-project","messages":[{"id":"m1","type":"gemini","timestamp":"2026-11-07T12:00:00Z","model":"gemini-2.5-pro","tokens":{"input":100,"output":20,"cached":30,"thoughts":7,"tool":3,"total":130}}]}
```

JSONL may start with `{"sessionId":"synthetic","projectHash":"synthetic-project"}`
and then one message/control per line. Message timestamps must be valid RFC3339;
session `startTime`/`lastUpdated` never substitute for missing message time. IDs
and models are nonempty ASCII letters/digits or `-_.:/`, at most 256 bytes.
Hints exclude `/` and `:` and cannot be `.` or `..`. Input is capped at 16 MiB
and JSONL at 100,000 lines; callers must bound reads before allocating text.

## Accounting and replay

- Only `type: "gemini"` messages count. User, tool and informational records do not.
- Required unsigned integer counters are `input`, `output`, `cached`, `total`.
  Missing/null/negative/fractional/string/overflow counters produce no usage
  record, never invented zeroes. Explicit reported zeroes remain zeroes.
- Input is `promptTokenCount`; cached is `cachedContentTokenCount`, a **subset of
  input**, not an additional category. Output is exactly `candidatesTokenCount`,
  **not output plus thoughts**. Optional `thoughts` and `tool` map to
  `thoughtsTokenCount` and `toolUsePromptTokenCount`; they are validated when
  present but not exposed separately by the shared interface. Total is the
  reported `totalTokenCount`, never reconstructed as input + output + cached.
  Therefore visible categories need not add up to total. This schema cannot
  distinguish upstream's own default-zero values from measured zeroes.
- Repeated IDs replace their whole message in first-insertion order; they are
  never summed. An incomplete replacement removes previously counted usage.
- `$rewindTo` removes its target **and everything after it**; an unknown target
  clears history, matching the upstream loader. This measures the active saved
  transcript, **not a complete lifetime/billed ledger**: rewound, removed,
  compacted or deleted turns may already have consumed tokens.
- `$patch` content/tool-result changes do not change usage. `updates`, `removeIds`
  and `orderIds` are replayed; ordering affects later rewind. Unsupported patch
  fields (including token patches) fail closed. `$set.messages` replaces the
  transcript; other `$set` fields are conversation metadata only.
- Unknown/ambiguous controls, malformed JSON, malformed control structures and
  limits return `Gemini session data is invalid or unsupported.` without partial
  usage. Incomplete messages produce one generic warning:
  `Gemini usage metadata is incomplete or invalid.` Invalid hints return
  `Gemini session identifier is invalid.` No diagnostic echoes input content.

## Verified sources and limitations

Verified against official public `main` source on 2026-11-07, not a stable
versioned persistence API; future schema changes may require adapter updates.
No live installed CLI/account was accessed for this implementation.

- [Persisted types and token categories](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/services/chatRecordingTypes.ts)
- [Recorder, migration, ID generation and JSONL replay](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/services/chatRecordingService.ts)
- [Runtime roots, project identifiers and slug migration](https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/config/storage.ts)
- [Session management and saved content](https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/session-management.md)

The recorder writes default zeroes when SDK fields are absent. This adapter
does not extend that behavior to absent persisted counters. Unlike the upstream
loader's best-effort malformed-line handling and fabricated fallback dates, the
monitor fails closed on malformed lines and requires actual message timestamps.
