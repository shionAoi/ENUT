//! Codex CLI local rollout and headless usage decoder aligned with ccusage.
use crate::usage::{ParseResult, SessionFile, TokenRecord};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

pub(crate) const SOURCE: &str = "Codex CLI local rollout usage";
pub(crate) const MODEL: &str = "Unreported model";
const FALLBACK_MODEL: &str = "gpt-5";
const CODEX_AUTO_REVIEW_MODEL: &str = "codex-auto-review";
const MAX_SAFE: u64 = 9_007_199_254_740_991;
const MAX_LINES: usize = 200_000;

const CODEX_AUTO_REVIEW_FALLBACKS: &[((i32, u32, u32), &str)] = &[
    ((2026, 7, 30), "gpt-5.6-luna"),
    ((2026, 3, 5), "gpt-5.4"),
    ((2026, 2, 5), "gpt-5.3-codex"),
    ((2025, 12, 11), "gpt-5.2-codex"),
    ((2025, 11, 13), "gpt-5.1-codex"),
    ((2025, 9, 15), "gpt-5-codex"),
    ((2025, 8, 7), FALLBACK_MODEL),
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CodexRawUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

impl CodexRawUsage {
    const fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.cached_input_tokens == 0
            && self.cache_creation_tokens == 0
            && self.output_tokens == 0
            && self.reasoning_output_tokens == 0
    }

    fn normalize(mut self) -> Self {
        self.cached_input_tokens = self.cached_input_tokens.min(self.input_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .min(self.input_tokens.saturating_sub(self.cached_input_tokens));
        self
    }

    fn subtract(self, previous: Option<Self>) -> Self {
        let prev = previous.unwrap_or_default();
        Self {
            input_tokens: self.input_tokens.saturating_sub(prev.input_tokens),
            cached_input_tokens: self
                .cached_input_tokens
                .saturating_sub(prev.cached_input_tokens),
            cache_creation_tokens: self
                .cache_creation_tokens
                .saturating_sub(prev.cache_creation_tokens),
            output_tokens: self.output_tokens.saturating_sub(prev.output_tokens),
            reasoning_output_tokens: self
                .reasoning_output_tokens
                .saturating_sub(prev.reasoning_output_tokens),
            total_tokens: self.total_tokens.saturating_sub(prev.total_tokens),
        }
        .normalize()
    }
}

struct ParsedEvent {
    record: TokenRecord,
    raw_usage: CodexRawUsage,
    replayable: bool,
}

struct PendingUsageRecord {
    response_id: String,
    event: ParsedEvent,
    thread_token_usage: Option<CodexRawUsage>,
}

struct RolloutParsed {
    session_id: Option<String>,
    forked_from_id: Option<String>,
    events: Vec<ParsedEvent>,
    warnings: Vec<String>,
    saw_cumulative: bool,
}

/// Parse a single Codex rollout JSONL file.
/// Returns `(result, has_cumulative_only)` where `has_cumulative_only` is true only
/// when cumulative snapshots were seen without any parseable usage delta.
pub fn parse(text: &str) -> (ParseResult, bool) {
    let parsed = parse_rollout(text);
    let has_cumulative_only = parsed.saw_cumulative && parsed.events.is_empty();
    let records = parsed.events.into_iter().map(|e| e.record).collect();
    (
        ParseResult {
            records,
            warnings: parsed.warnings,
        },
        has_cumulative_only,
    )
}

/// Parse multiple Codex rollout files with cross-file fork replay stripping and duplicate collapse.
pub fn parse_files(files: &[SessionFile]) -> (ParseResult, bool) {
    let mut ordered: Vec<&SessionFile> = files.iter().collect();
    ordered.sort_by(|a, b| a.path_hint.cmp(&b.path_hint));

    let rollouts: Vec<RolloutParsed> = ordered.iter().map(|f| parse_rollout(&f.text)).collect();

    // Index each session's pre-stripped replayable raw-usage sequence for fork prefix stripping.
    let mut parent_usage: HashMap<String, Vec<CodexRawUsage>> = HashMap::new();
    for rollout in &rollouts {
        if let Some(ref sid) = rollout.session_id {
            let seq: Vec<CodexRawUsage> = rollout
                .events
                .iter()
                .filter(|e| e.replayable)
                .map(|e| e.raw_usage)
                .collect();
            let replace = parent_usage
                .get(sid)
                .is_none_or(|existing| seq.len() > existing.len());
            if replace {
                parent_usage.insert(sid.clone(), seq);
            }
        }
    }

    let mut unique: BTreeMap<String, (DateTime<Utc>, Option<TokenRecord>)> = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut saw_cumulative = false;

    for rollout in rollouts {
        saw_cumulative |= rollout.saw_cumulative;
        warnings.extend(rollout.warnings);

        let mut inherited = rollout
            .forked_from_id
            .as_deref()
            .and_then(|pid| parent_usage.get(pid))
            .map(|parent| {
                let child_leading: Vec<CodexRawUsage> = rollout
                    .events
                    .iter()
                    .filter(|e| e.replayable)
                    .map(|e| e.raw_usage)
                    .take(parent.len())
                    .collect();
                replayed_parent_len(parent, &child_leading)
            })
            .unwrap_or(0);

        for event in rollout.events {
            if event.replayable && inherited > 0 {
                inherited -= 1;
                continue;
            }
            let row = event.record;
            match unique.entry(row.id.clone()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert((row.timestamp, Some(row)));
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    let (ts, old) = slot.get_mut();
                    if row.timestamp > *ts {
                        *ts = row.timestamp;
                        *old = Some(row);
                    } else if row.timestamp == *ts && old.as_ref() != Some(&row) {
                        *old = None;
                    }
                }
            }
        }
    }

    if unique.values().any(|(_, row)| row.is_none()) {
        warnings
            .push("Conflicting copies of a Codex response were excluded from this scan.".into());
    }
    let records: Vec<TokenRecord> = unique.into_values().filter_map(|(_, row)| row).collect();
    let has_cumulative_only = saw_cumulative && records.is_empty();
    warnings.sort();
    warnings.dedup();
    (ParseResult { records, warnings }, has_cumulative_only)
}

fn parse_rollout(text: &str) -> RolloutParsed {
    let mut events: Vec<ParsedEvent> = Vec::new();
    let mut pending_usage_records: Vec<PendingUsageRecord> = Vec::new();
    let mut compacted_response_ids: HashSet<String> = HashSet::new();
    let mut emitted_response_ids: HashSet<String> = HashSet::new();
    let mut occurrence_counts: HashMap<(String, String, u64, u64, u64, u64, u64), u64> =
        HashMap::new();

    let mut session_id: Option<String> = None;
    let mut forked_from_id: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut previous_totals: Option<CodexRawUsage> = None;
    let mut unsupported = false;
    let mut saw_cumulative = false;
    let mut line_count = 0usize;

    for line in text
        .split_inclusive('\n')
        .filter(|line| line.ends_with('\n'))
    {
        if line.trim().is_empty() {
            continue;
        }
        line_count += 1;
        if line_count > MAX_LINES {
            unsupported = true;
            break;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            unsupported = true;
            continue;
        };
        if !row.is_object() {
            unsupported = true;
            continue;
        }

        match row["type"].as_str() {
            Some("session_meta") => {
                if let Some(payload) = row.get("payload") {
                    if session_id.is_none() {
                        session_id = non_empty_trimmed(payload.get("id"));
                    }
                    if forked_from_id.is_none() {
                        forked_from_id = non_empty_trimmed(payload.get("forked_from_id")).or_else(
                            || {
                                non_empty_trimmed(
                                    payload.pointer("/source/subagent/thread_spawn/parent_thread_id"),
                                )
                            },
                        );
                    }
                    if let Some(model) = extract_model(payload) {
                        current_model = Some(model);
                    }
                }
            }
            Some("turn_context") => {
                if let Some(model) = row.get("payload").and_then(extract_model) {
                    current_model = Some(model);
                }
            }
            Some("compacted") => {
                if let Some(resp_id) = row
                    .pointer("/payload/compaction_response_id")
                    .and_then(|v| non_empty_trimmed(Some(v)))
                {
                    compacted_response_ids.insert(resp_id.clone());
                    if let Some(pos) = pending_usage_records
                        .iter()
                        .position(|p| p.response_id == resp_id)
                    {
                        let pending = pending_usage_records.remove(pos);
                        if emitted_response_ids.insert(resp_id) {
                            events.push(pending.event);
                        }
                    }
                }
            }
            Some("token_usage_record") => {
                match parse_token_usage_record(&row, current_model.as_deref()) {
                    Some((resp_id, event, thread_usage)) => {
                        if emitted_response_ids.contains(&resp_id) {
                            continue;
                        }
                        if compacted_response_ids.contains(&resp_id) {
                            if emitted_response_ids.insert(resp_id) {
                                events.push(event);
                            }
                        } else {
                            pending_usage_records.push(PendingUsageRecord {
                                response_id: resp_id,
                                event,
                                thread_token_usage: thread_usage,
                            });
                        }
                    }
                    None => unsupported = true,
                }
            }
            Some("event_msg") if row.pointer("/payload/type").and_then(Value::as_str) == Some("token_count") => {
                let payload = &row["payload"];
                let info = payload.get("info");
                if info
                    .and_then(|i| i.pointer("/total_token_usage/total_tokens"))
                    .and_then(Value::as_u64)
                    .is_some()
                {
                    saw_cumulative = true;
                }
                let total_candidate = match info.and_then(|i| i.get("total_token_usage")) {
                    None | Some(Value::Null) => UsageParse::Missing,
                    Some(v) => decode_raw_usage(v),
                };
                let last_candidate = match info.and_then(|i| i.get("last_token_usage")) {
                    None | Some(Value::Null) => UsageParse::Missing,
                    Some(v) => decode_raw_usage(v),
                };
                if matches!(total_candidate, UsageParse::Malformed)
                    || matches!(last_candidate, UsageParse::Malformed)
                {
                    if let UsageParse::Decoded(t) = total_candidate {
                        previous_totals = Some(t);
                    }
                    unsupported = true;
                    continue;
                }
                let total_usage = total_candidate.decoded();
                let last_usage = last_candidate.decoded();

                let cumulative_advanced = total_usage
                    .as_ref()
                    .is_none_or(|t| previous_totals.as_ref() != Some(t));
                let delta = if cumulative_advanced {
                    last_usage.or_else(|| total_usage.map(|t| t.subtract(previous_totals)))
                } else {
                    None
                };
                if let Some(t) = total_usage {
                    previous_totals = Some(t);
                }

                let Some(raw) = delta.map(CodexRawUsage::normalize) else {
                    continue;
                };
                if raw.is_empty() {
                    continue;
                }
                let Some(timestamp) = parse_timestamp(row.get("timestamp")) else {
                    continue;
                };
                let inline_model = info
                    .and_then(extract_model)
                    .or_else(|| extract_model(payload))
                    .or_else(|| extract_model(&row));
                if let Some(ref m) = inline_model {
                    current_model = Some(m.clone());
                }
                let model = resolve_model(
                    inline_model.as_deref().or(current_model.as_deref()),
                    timestamp,
                    FALLBACK_MODEL,
                );

                // Suppress matching pending token_usage_record to prevent double-counting.
                if cumulative_advanced {
                    if let Some(pos) = pending_usage_records.iter().rposition(|p| {
                        p.event.raw_usage == raw
                            || p.thread_token_usage
                                .zip(total_usage)
                                .is_some_and(|(rec, tot)| rec == tot.normalize())
                    }) {
                        let absorbed = pending_usage_records.remove(pos);
                        emitted_response_ids.insert(absorbed.response_id);
                    }
                }

                if let Some(record) =
                    build_token_count_record("tc", timestamp, model, raw, &mut occurrence_counts)
                {
                    events.push(ParsedEvent {
                        record,
                        raw_usage: raw,
                        replayable: true,
                    });
                } else {
                    unsupported = true;
                }
            }
            Some("event_msg") => {}
            _ => {
                // Headless `codex exec --json` usage lines.
                if let Some(parsed_headless) = parse_headless_usage(
                    &row,
                    &mut current_model,
                    &mut occurrence_counts,
                ) {
                    match parsed_headless {
                        Ok(event) => events.push(event),
                        Err(()) => unsupported = true,
                    }
                }
            }
        }
    }

    // Flush standalone / unabsorbed token_usage_record entries.
    for pending in pending_usage_records {
        if !emitted_response_ids.contains(&pending.response_id) {
            events.push(pending.event);
        }
    }

    let mut warnings = Vec::new();
    if unsupported {
        warnings.push("Some Codex rollout records could not be read as reported usage.".into());
    }
    RolloutParsed {
        session_id,
        forked_from_id,
        events,
        warnings,
        saw_cumulative,
    }
}

fn parse_token_usage_record(
    row: &Value,
    current_model: Option<&str>,
) -> Option<(String, ParsedEvent, Option<CodexRawUsage>)> {
    let p = row.get("payload")?;
    let safe_id = |field: &str| {
        p.get(field)?.as_str().filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        })
    };
    let opt_safe_id = |field: &str| -> Result<Option<&str>, ()> {
        match p.get(field) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_str()
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= 256
                        && value
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                })
                .map(Some)
                .ok_or(()),
        }
    };
    opt_safe_id("session_id").ok()?;
    opt_safe_id("turn_id").ok()?;
    let thread_id = opt_safe_id("thread_id").ok()?;
    let response_id = safe_id("response_id")?;
    let key = match thread_id {
        Some(tid) => format!("{tid}:{response_id}"),
        None => format!("response:{response_id}"),
    };

    let t = p.get("usage")?;
    let count = |field: &str| t.get(field)?.as_u64().filter(|n| *n <= MAX_SAFE);
    let raw_input = count("input_tokens")?;
    let raw_output = count("output_tokens")?;
    let raw_cached = match t.get("cached_input_tokens") {
        Some(v) => v.as_u64().filter(|n| *n <= MAX_SAFE)?,
        None => 0,
    };
    let raw_creation = match t
        .get("cache_write_input_tokens")
        .or_else(|| t.get("cache_creation_input_tokens"))
    {
        Some(v) => v.as_u64().filter(|n| *n <= MAX_SAFE)?,
        None => 0,
    };
    let raw_total = count("total_tokens")?;
    let raw_reasoning = match t.get("reasoning_output_tokens") {
        Some(v) => v.as_u64().filter(|n| *n <= MAX_SAFE)?,
        None => 0,
    };
    if raw_cached > raw_input
        || raw_reasoning > raw_output
        || raw_input.checked_add(raw_output) != Some(raw_total)
    {
        return None;
    }

    let timestamp = parse_timestamp(row.get("timestamp"))?;
    let raw = CodexRawUsage {
        input_tokens: raw_input,
        cached_input_tokens: raw_cached,
        cache_creation_tokens: raw_creation,
        output_tokens: raw_output,
        reasoning_output_tokens: raw_reasoning,
        total_tokens: raw_total,
    }
    .normalize();

    let (input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, cached_tokens, total_tokens) =
        normalize_for_record(raw)?;
    let inline_model = extract_model(p);
    let model = resolve_model(
        inline_model.as_deref().or(current_model),
        timestamp,
        MODEL,
    );

    let thread_token_usage = p
        .get("thread_token_usage")
        .or_else(|| p.get("thread_total_token_usage"))
        .and_then(|v| decode_raw_usage(v).decoded())
        .filter(|u| !u.is_empty());

    let record = TokenRecord {
        id: sha256_hex(key.as_bytes()),
        timestamp,
        model,
        input_tokens,
        output_tokens,
        cached_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        total_tokens,
    };
    Some((
        response_id.to_owned(),
        ParsedEvent {
            record,
            raw_usage: raw,
            replayable: false,
        },
        thread_token_usage,
    ))
}

fn parse_headless_usage(
    row: &Value,
    current_model: &mut Option<String>,
    occurrence_counts: &mut HashMap<(String, String, u64, u64, u64, u64, u64), u64>,
) -> Option<Result<ParsedEvent, ()>> {
    let containers = [
        Some(row),
        row.get("data"),
        row.get("result"),
        row.get("response"),
    ];
    let mut candidate = UsageParse::Missing;
    for container in containers.into_iter().flatten() {
        if let Some(usage_val) = container.get("usage") {
            if usage_val.is_null() {
                continue;
            }
            match decode_raw_usage(usage_val) {
                d @ UsageParse::Decoded(_) => {
                    candidate = d;
                    break;
                }
                UsageParse::Malformed => candidate = UsageParse::Malformed,
                UsageParse::Missing => {}
            }
        }
    }
    let raw = match candidate {
        UsageParse::Missing => return None,
        UsageParse::Malformed => return Some(Err(())),
        UsageParse::Decoded(u) => u.normalize(),
    };
    if raw.is_empty() {
        return None;
    }
    let timestamp = containers
        .into_iter()
        .flatten()
        .find_map(|c| {
            parse_timestamp(
                c.get("timestamp")
                    .or_else(|| c.get("created_at"))
                    .or_else(|| c.get("createdAt")),
            )
        })?;
    let inline_model = containers.into_iter().flatten().find_map(extract_model);
    if let Some(ref m) = inline_model {
        *current_model = Some(m.clone());
    }
    let model = resolve_model(
        inline_model.as_deref().or(current_model.as_deref()),
        timestamp,
        FALLBACK_MODEL,
    );
    match build_token_count_record("exec", timestamp, model, raw, occurrence_counts) {
        Some(record) => Some(Ok(ParsedEvent {
            record,
            raw_usage: raw,
            replayable: true,
        })),
        None => Some(Err(())),
    }
}

fn build_token_count_record(
    kind: &str,
    timestamp: DateTime<Utc>,
    model: String,
    raw: CodexRawUsage,
    occurrence_counts: &mut HashMap<(String, String, u64, u64, u64, u64, u64), u64>,
) -> Option<TokenRecord> {
    let (input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, cached_tokens, total_tokens) =
        normalize_for_record(raw)?;
    let ts_key = timestamp.to_rfc3339();
    let occ_key = (
        ts_key.clone(),
        model.clone(),
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        total_tokens,
    );
    let occ = occurrence_counts.entry(occ_key).or_insert(0);
    let idx = *occ;
    *occ += 1;
    let id_material = format!(
        "codex:{kind}:{ts_key}:{model}:{input_tokens}:{output_tokens}:{cache_creation_tokens}:{cache_read_tokens}:{total_tokens}:{idx}"
    );
    Some(TokenRecord {
        id: sha256_hex(id_material.as_bytes()),
        timestamp,
        model,
        input_tokens,
        output_tokens,
        cached_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        total_tokens,
    })
}

fn normalize_for_record(raw: CodexRawUsage) -> Option<(u64, u64, u64, u64, u64, u64)> {
    let raw = raw.normalize();
    let cached_read = raw.cached_input_tokens;
    let cache_creation = raw.cache_creation_tokens;
    let input_tokens = raw
        .input_tokens
        .saturating_sub(cached_read)
        .saturating_sub(cache_creation);
    let output_tokens = raw.output_tokens;
    let cached_tokens = cached_read.checked_add(cache_creation)?;
    let min_total = raw.input_tokens.checked_add(raw.output_tokens)?;
    let total_tokens = raw.total_tokens.max(min_total);
    if total_tokens > MAX_SAFE
        || input_tokens
            .checked_add(output_tokens)
            .and_then(|v| v.checked_add(cached_tokens))
            .is_none_or(|sum| sum > total_tokens)
    {
        return None;
    }
    Some((
        input_tokens,
        output_tokens,
        cache_creation,
        cached_read,
        cached_tokens,
        total_tokens,
    ))
}

enum UsageParse {
    Missing,
    Malformed,
    Decoded(CodexRawUsage),
}

impl UsageParse {
    const fn decoded(self) -> Option<CodexRawUsage> {
        match self {
            Self::Decoded(u) => Some(u),
            Self::Missing | Self::Malformed => None,
        }
    }
}

fn decode_raw_usage(val: &Value) -> UsageParse {
    let Some(obj) = val.as_object() else {
        return UsageParse::Malformed;
    };
    let read_opt = |keys: &[&str]| -> Result<Option<u64>, ()> {
        for key in keys {
            if let Some(v) = obj.get(*key) {
                if v.is_null() {
                    continue;
                }
                let Some(n) = v.as_u64().filter(|n| *n <= MAX_SAFE) else {
                    return Err(());
                };
                return Ok(Some(n));
            }
        }
        Ok(None)
    };
    let Ok(input) = read_opt(&["input_tokens", "prompt_tokens", "input"]) else {
        return UsageParse::Malformed;
    };
    let Ok(output) = read_opt(&["output_tokens", "completion_tokens", "output"]) else {
        return UsageParse::Malformed;
    };
    let Ok(cached) = read_opt(&[
        "cached_input_tokens",
        "cache_read_input_tokens",
        "cached_tokens",
    ]) else {
        return UsageParse::Malformed;
    };
    let Ok(cache_write) = read_opt(&["cache_write_input_tokens", "cache_creation_input_tokens"])
    else {
        return UsageParse::Malformed;
    };
    let Ok(reasoning) = read_opt(&["reasoning_output_tokens", "reasoning_tokens"]) else {
        return UsageParse::Malformed;
    };
    let Ok(total) = read_opt(&["total_tokens"]) else {
        return UsageParse::Malformed;
    };

    let input = input.unwrap_or(0);
    let output = output.unwrap_or(0);
    let cached_input = cached.unwrap_or(0).min(input);
    let cache_creation = cache_write
        .unwrap_or(0)
        .min(input.saturating_sub(cached_input));
    let derived_total = input.saturating_add(output);
    let total_tokens = total.filter(|t| *t > 0).unwrap_or(derived_total);

    UsageParse::Decoded(CodexRawUsage {
        input_tokens: input,
        cached_input_tokens: cached_input,
        cache_creation_tokens: cache_creation,
        output_tokens: output,
        reasoning_output_tokens: reasoning.unwrap_or(0),
        total_tokens,
    })
}

fn extract_model(container: &Value) -> Option<String> {
    non_empty_trimmed(container.get("model"))
        .or_else(|| non_empty_trimmed(container.get("model_name")))
        .or_else(|| non_empty_trimmed(container.pointer("/metadata/model")))
        .filter(|m| m.len() <= 512 && !m.chars().any(char::is_control))
}

fn resolve_model(candidate: Option<&str>, timestamp: DateTime<Utc>, default_model: &str) -> String {
    let model = candidate.unwrap_or(default_model);
    if model == CODEX_AUTO_REVIEW_MODEL {
        let date = (timestamp.year(), timestamp.month(), timestamp.day());
        let routed = CODEX_AUTO_REVIEW_FALLBACKS
            .iter()
            .filter(|(released_on, _)| *released_on <= date)
            .max_by_key(|(released_on, _)| *released_on)
            .map_or(FALLBACK_MODEL, |(_, m)| *m);
        return routed.to_owned();
    }
    model.to_owned()
}

fn non_empty_trimmed(val: Option<&Value>) -> Option<String> {
    let s = val?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

fn parse_timestamp(val: Option<&Value>) -> Option<DateTime<Utc>> {
    let val = val?;
    if let Some(s) = val.as_str() {
        return DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    if let Some(raw) = val.as_u64() {
        let millis = if raw > 10_000_000_000 {
            raw
        } else {
            raw.checked_mul(1_000)?
        };
        return Utc.timestamp_millis_opt(i64::try_from(millis).ok()?).single();
    }
    None
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn replayed_parent_len(parent: &[CodexRawUsage], child: &[CodexRawUsage]) -> usize {
    let prefix = parent
        .iter()
        .zip(child)
        .take_while(|(p, c)| p == c)
        .count();
    prefix.max(longest_parent_suffix_prefix(parent, child))
}

fn longest_parent_suffix_prefix(parent: &[CodexRawUsage], child: &[CodexRawUsage]) -> usize {
    let pattern = &child[..child.len().min(parent.len())];
    if pattern.is_empty() {
        return 0;
    }
    let mut failure = vec![0usize; pattern.len()];
    let mut matched = 0usize;
    for i in 1..pattern.len() {
        while matched > 0 && pattern[i] != pattern[matched] {
            matched = failure[matched - 1];
        }
        if pattern[i] == pattern[matched] {
            matched += 1;
            failure[i] = matched;
        }
    }
    let mut best = 0usize;
    for item in parent {
        while best > 0 && *item != pattern[best] {
            best = failure[best - 1];
        }
        if *item == pattern[best] {
            best += 1;
            if best == pattern.len() {
                return best;
            }
        }
    }
    best
}
