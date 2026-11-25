# Claude local usage metadata

## Capability and compatibility boundary

This pure parser performs no IO, authentication, inference, pricing, or quota
requests. The parent connector must obtain revocable Claude permission **before**
reading any usage file. Detection alone is not permission. Development tests use
synthetic records, never installed CLI output or real conversations.

**Ordinary Claude assistant transcript rows do not establish complete totals.**
Anthropic's current Agent SDK cost guide explicitly calls their `output_tokens`
a placeholder from `message_start`. Deduplicating these rows fixes duplicate
input, not missing output. They therefore produce a generic warning and no
record, even if `stop_reason` is populated. Null/missing usage is not zero.
Result-level totals are also unsupported: resumed sessions, cumulative results,
subagent inclusion, and model/time attribution cannot safely be reconstructed
as per-message history. Do not claim ordinary Claude history is implemented.

The recognized compatibility profile is timestamped Agent SDK `stream_event`
JSONL with API `message_start`, cumulative `message_delta`, and `message_stop`.
It emits a record only after a terminal stop reason and the stop event. It does
not sum deltas. This is an optional supported export shape, **not a documented
guarantee that Claude's on-disk transcripts persist stream events**. No stable,
versioned public local `message.usage` accounting contract was established.
Evidence was checked on 2026-11-07; no CLI/package version was tested live.
Unknown event schemas, interleaved starts, non-null `parent_tool_use_id`, invalid
counts, and incomplete streams fail closed for the entire supplied file with a
generic warning. Content block events are ignored, never interpreted as usage.

## Supported synthetic shape

Each line below is an independent JSON object; no conversation content is needed:

```jsonl
{"type":"stream_event","timestamp":"2026-11-07T10:00:00Z","parent_tool_use_id":null,"event":{"type":"message_start","message":{"type":"message","role":"assistant","id":"msg_example","model":"claude-sonnet-4-5","usage":{"input_tokens":10,"output_tokens":1,"cache_read_input_tokens":20,"cache_creation_input_tokens":30}}}}
{"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}}
{"type":"stream_event","event":{"type":"message_stop"}}
```

Normalized input is **60**: uncached input (10) + cache read (20) + cache
creation (30). Cached is **20**, the read-only subset already included in input;
do not add it again. Output is **9**, the last cumulative delta count, not the
start placeholder. Total is **69**, inclusive input + output. All three input
buckets must be explicit at start; omitted caches are unknown, not assumed zero.
Later supplied input/cache buckets replace their cumulative values. Missing
updates preserve the prior bucket; explicit null is invalid. Overflow is rejected.

Start timestamps must be valid RFC3339 and normalize to UTC. Models are 1–128
ASCII bytes, IDs 1–200, restricted to letters, digits, `-_.:`. IDs are
`claude:<API message ID>`: the opaque session hint is deliberately excluded so
copied/forked history deduplicates globally. Within one serial export, the last
complete record for a repeated ID replaces earlier records. The parent owns
cross-file conflict resolution and history bucketing; it must not sum copies.

## Approved file discovery and privacy

Default transcript convention: `~/.claude/projects/<encoded-project>/*.jsonl`.
Subagent convention: `<session-id>/subagents/agent-*.jsonl` within that project.
Subagent path support does not make placeholder usage complete. Custom
`CLAUDE_CONFIG_DIR` roots need separately disclosed, bounded approval; this
parser does not discover roots, read environment variables, or follow links.
The parent reader must enforce no-symlink and byte/file-count limits, restrict
enumeration to approved locations, and clear in-memory data on revocation.
Transcripts contain prompts, paths, tool results, and potentially secrets even
though only accounting metadata is retained. Never log raw rows or serde errors.
Diagnostics here are fixed strings, with no paths, identities, or payload values.
No hooks/configuration changes are needed or authorized. No token counts imply
USD spend, remaining subscription allowance, or a reset time.

## Primary evidence

The integrated widget also supports a deliberately **partial input/cache view**
for ordinary assistant transcript rows through `claude_inputs.rs`. It deduplicates
API message IDs across files, uses explicit input/cache fields, and ignores the
placeholder output count entirely. `partialUsage` is separate from complete
usage: it cannot supply total tokens, output tokens, history totals, USD spend,
remaining allowance, or reset times. Input includes uncached input, cache read,
and cache creation; cached input is the read subset already included in input.
Missing cache fields are unknown rather than invented zeroes. No hooks or
telemetry are enabled and Claude configuration is unchanged. Complete automatic
Claude accounting still needs a separately supported source or explicit consent
for additional instrumentation; native stats-cache data is not substituted.

- [Agent SDK cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking#read-output-tokens-from-the-result-message): assistant output is a placeholder; repeated messages share IDs; result totals have different scopes.
- [API streaming](https://platform.claude.com/docs/en/build-with-claude/streaming#event-types): delta usage is cumulative; completion requires message stop; later deltas may update input.
- [Agent SDK sessions](https://code.claude.com/docs/en/agent-sdk/sessions#resume-by-id): default local project JSONL paths and custom-root limitations.
- [Subagents](https://code.claude.com/docs/en/sub-agents#accessing-subagent-transcripts): nested subagent transcript convention.
- [Prompt caching](https://code.claude.com/docs/en/agent-sdk/cost-tracking#track-cache-tokens): cache creation and cache read are distinct from uncached input.

These document API/SDK behavior and storage locations, not a stable persisted
accounting schema. The TypeScript SDK source URL inspected during research
returned 404; no source-visible transcript contract is claimed from it.
