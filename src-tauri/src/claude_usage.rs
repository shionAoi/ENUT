//! Pure Claude Code metadata decoder aligned with ccusage accounting.
use crate::usage::{ParseResult, SessionFile, TokenRecord};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};

const WARNING: &str = "Claude usage is incomplete or unsupported.";
const MAX_SAFE: u64 = 9_007_199_254_740_991;
const MAX_LINES: usize = 200_000;

struct PendingStream {
    record: TokenRecord,
    uncached: u64,
    creation: u64,
    read: u64,
    terminal: bool,
}

#[derive(Clone, Copy)]
struct ParsedUsage {
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    cached_tokens: u64,
    total_tokens: u64,
    has_cache_creation_split: bool,
    speed_rank: u8,
}

struct ClaudeCandidate {
    message_id: Option<String>,
    request_id: Option<String>,
    response_key: Option<String>,
    session_id: String,
    timestamp: DateTime<Utc>,
    is_sidechain: bool,
    has_cache_creation_split: bool,
    speed_rank: u8,
    has_cost_usd: bool,
    model: String,
    usage: ParsedUsage,
    anon_seq: usize,
}

impl ClaudeCandidate {
    fn to_record(&self) -> TokenRecord {
        let raw_id = match (&self.message_id, &self.request_id, &self.response_key) {
            (Some(mid), Some(rid), _) => format!("claude:{mid}:{rid}"),
            (Some(mid), None, Some(rkey)) => format!("claude:{mid}:{}:{rkey}", self.session_id),
            (Some(mid), None, None) => format!(
                "claude:{mid}:{}:{}",
                self.session_id,
                self.timestamp.timestamp_millis()
            ),
            (None, _, _) => format!(
                "claude:anon:{}:{}:{}",
                self.session_id,
                self.timestamp.timestamp_millis(),
                self.anon_seq
            ),
        };
        TokenRecord {
            id: safe_record_id(&raw_id),
            timestamp: self.timestamp,
            model: self.model.clone(),
            input_tokens: self.usage.input_tokens,
            output_tokens: self.usage.output_tokens,
            cached_tokens: self.usage.cached_tokens,
            cache_creation_tokens: self.usage.cache_creation_tokens,
            cache_read_tokens: self.usage.cache_read_tokens,
            total_tokens: self.usage.total_tokens,
        }
    }
}

/// Decode a single Claude session JSONL file (ordinary assistant, AgentProgress, and SDK stream events).
pub fn parse(text: &str, session_hint: &str) -> Result<ParseResult, String> {
    let mut deduper = ClaudeDeduper::default();
    let mut stream_records = BTreeMap::new();
    let mut warn = false;
    parse_single_text(
        text,
        session_hint,
        true,
        &mut deduper,
        &mut stream_records,
        &mut warn,
    )?;
    let mut records: Vec<TokenRecord> = stream_records.into_values().collect();
    records.extend(deduper.into_records());
    Ok(ParseResult {
        records,
        warnings: if warn { vec![WARNING.to_owned()] } else { vec![] },
    })
}

/// Decode and deduplicate multiple Claude session JSONL files deterministically across files.
pub fn parse_files(files: &[SessionFile]) -> ParseResult {
    let mut ordered: Vec<&SessionFile> = files.iter().collect();
    ordered.sort_by(|a, b| file_order(&a.path_hint, &b.path_hint));
    let mut deduper = ClaudeDeduper::default();
    let mut stream_records = BTreeMap::new();
    let mut warn = false;
    for file in ordered {
        let _ = parse_single_text(
            &file.text,
            &file.hint,
            false,
            &mut deduper,
            &mut stream_records,
            &mut warn,
        );
    }
    let mut records: Vec<TokenRecord> = stream_records.into_values().collect();
    records.extend(deduper.into_records());
    ParseResult {
        records,
        warnings: if warn { vec![WARNING.to_owned()] } else { vec![] },
    }
}

fn file_order(left: &str, right: &str) -> std::cmp::Ordering {
    let is_subagent = |path: &str| path.contains("/subagents/") || path.contains("\\subagents\\");
    is_subagent(left)
        .cmp(&is_subagent(right))
        .then_with(|| left.cmp(right))
}

fn parse_single_text(
    text: &str,
    session_hint: &str,
    strict_json: bool,
    deduper: &mut ClaudeDeduper,
    stream_records: &mut BTreeMap<String, TokenRecord>,
    warn: &mut bool,
) -> Result<(), String> {
    let mut local_streams = BTreeMap::new();
    let mut pending = None;
    let mut stream_invalid = false;
    let mut chains = ResponseChains::new(session_hint);
    let mut line_count = 0usize;

    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        line_count += 1;
        if line_count > MAX_LINES {
            *warn = true;
            break;
        }
        let row: Value = match serde_json::from_str::<Value>(line) {
            Ok(val) if val.is_object() => val,
            _ if strict_json => return Err("Invalid Claude usage JSONL.".to_owned()),
            _ => {
                *warn = true;
                continue;
            }
        };
        match row["type"].as_str() {
            Some("stream_event") => {
                if consume_stream(&row, &mut pending, &mut local_streams).is_err() {
                    stream_invalid = true;
                    pending = None;
                }
            }
            Some("user" | "tool" | "system" | "file-history-snapshot" | "result") => {
                if chains.is_tracking() {
                    chains.link(&row);
                }
            }
            Some("progress") => {
                if !ingest_envelope(&row, true, session_hint, &mut chains, deduper)
                    && chains.is_tracking()
                {
                    chains.link(&row);
                }
            }
            Some("assistant") | None => {
                if !ingest_envelope(&row, false, session_hint, &mut chains, deduper)
                    && chains.is_tracking()
                {
                    chains.link(&row);
                }
            }
            _ => {
                if !ingest_envelope(&row, false, session_hint, &mut chains, deduper)
                    && !ingest_envelope(&row, true, session_hint, &mut chains, deduper)
                {
                    if chains.is_tracking() {
                        chains.link(&row);
                    }
                    let has_usage_payload = row.get("usage").is_some()
                        || row
                            .get("message")
                            .and_then(|m| m.get("usage"))
                            .is_some()
                        || row
                            .get("data")
                            .and_then(|d| d.get("message"))
                            .and_then(|m| m.get("message"))
                            .and_then(|m| m.get("usage"))
                            .is_some();
                    if has_usage_payload {
                        *warn = true;
                    }
                }
            }
        }
    }

    if pending.is_some() {
        stream_invalid = true;
    }
    if stream_invalid {
        // An invalid stream could supersede an earlier instance of the same ID.
        local_streams.clear();
        *warn = true;
    } else {
        for (id, rec) in local_streams {
            stream_records.insert(id, rec);
        }
    }
    Ok(())
}

fn ingest_envelope(
    row: &Value,
    progress_nested: bool,
    session_hint: &str,
    chains: &mut ResponseChains,
    deduper: &mut ClaudeDeduper,
) -> bool {
    let (container, message, outer_session) = if progress_nested {
        let Some(dm) = row.get("data").and_then(|d| d.get("message")) else {
            return false;
        };
        let Some(msg) = dm.get("message") else {
            return false;
        };
        (dm, msg, row.get("sessionId"))
    } else {
        let Some(msg) = row.get("message") else {
            return false;
        };
        (row, msg, row.get("sessionId"))
    };

    let Some(usage_val) = message.get("usage") else {
        return false;
    };
    if usage_val.is_null() {
        return true;
    }

    if container
        .get("isApiErrorMessage")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    if let Some(version) = container.get("version") {
        if version.is_null() {
            return true;
        }
        let Some(v) = version.as_str() else {
            return true;
        };
        if !is_semver_prefix(v) {
            return true;
        }
    }

    let Some(timestamp) = container
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
    else {
        return true;
    };

    let session_id = match outer_session {
        Some(Value::String(s)) if !s.is_empty() && !s.chars().any(char::is_control) => s.clone(),
        Some(Value::String(_)) => return true,
        _ => session_hint.to_owned(),
    };
    let request_id = match container.get("requestId") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::String(_)) => return true,
        _ => None,
    };
    let message_id = match message.get("id") {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::String(_)) => return true,
        _ => None,
    };
    let Some(model) = valid_ordinary_model(message.get("model")) else {
        return true;
    };

    let Some(usage) = extract_claude_usage(usage_val, true) else {
        return true;
    };

    let is_sidechain = container
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let has_cost_usd = container.get("costUSD").and_then(Value::as_f64).is_some();

    let response_key = match (&request_id, &message_id) {
        (None, Some(mid)) if !progress_nested => chains.response_key(row, &session_id, mid, &usage),
        _ => None,
    };

    let advisors = extract_advisor_usages(usage_val);
    let base_mid = message_id.clone();

    let main_seq = deduper.next_seq();
    deduper.push(ClaudeCandidate {
        message_id,
        request_id: request_id.clone(),
        response_key: response_key.clone(),
        session_id: session_id.clone(),
        timestamp,
        is_sidechain,
        has_cache_creation_split: usage.has_cache_creation_split,
        speed_rank: usage.speed_rank,
        has_cost_usd,
        model,
        usage,
        anon_seq: main_seq,
    });

    for (idx, (adv_model, adv_usage)) in advisors.into_iter().enumerate() {
        let adv_mid = base_mid.as_ref().map(|mid| format!("{mid}:advisor:{idx}"));
        let adv_seq = deduper.next_seq();
        deduper.push(ClaudeCandidate {
            message_id: adv_mid,
            request_id: request_id.clone(),
            response_key: response_key.clone(),
            session_id: session_id.clone(),
            timestamp,
            is_sidechain,
            has_cache_creation_split: adv_usage.has_cache_creation_split,
            speed_rank: adv_usage.speed_rank,
            has_cost_usd: false,
            model: adv_model,
            usage: adv_usage,
            anon_seq: adv_seq,
        });
    }
    true
}

fn valid_ordinary_model(value: Option<&Value>) -> Option<String> {
    let s = value?.as_str()?;
    if s.is_empty() || s.len() > 512 || s == "<synthetic>" || s.chars().any(char::is_control) {
        return None;
    }
    Some(s.to_owned())
}

fn extract_claude_usage(val: &Value, read_iterations: bool) -> Option<ParsedUsage> {
    let obj = val.as_object()?;
    if obj.values().any(Value::is_null) && has_null_usage_field(obj) {
        return None;
    }
    let input_tokens = safe_u64(obj.get("input_tokens")?)?;
    let output_tokens = safe_u64(obj.get("output_tokens")?)?;
    let raw_creation = match obj.get("cache_creation_input_tokens") {
        Some(v) => safe_u64(v)?,
        None => 0,
    };
    let cache_read_tokens = match obj.get("cache_read_input_tokens") {
        Some(v) => safe_u64(v)?,
        None => 0,
    };
    let speed_rank = match obj.get("speed") {
        None => 0,
        Some(Value::String(s)) if s == "standard" => 1,
        Some(Value::String(s)) if s == "fast" => 2,
        _ => return None,
    };

    let iter_split = if read_iterations {
        obj.get("iterations").and_then(sum_message_iteration_splits)
    } else {
        None
    };
    let top_split = obj.get("cache_creation").and_then(parse_cache_split);
    let chosen_split = iter_split.or(top_split);

    let (cache_creation_tokens, has_cache_creation_split) = match chosen_split {
        Some((e5m, e1h)) => (checked_add_safe(e5m, e1h)?, true),
        None => (raw_creation, false),
    };

    let cached_tokens = checked_add_safe(cache_creation_tokens, cache_read_tokens)?;
    let total_tokens = checked_add_safe(
        checked_add_safe(input_tokens, output_tokens)?,
        cached_tokens,
    )?;
    Some(ParsedUsage {
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        cached_tokens,
        total_tokens,
        has_cache_creation_split,
        speed_rank,
    })
}

fn has_null_usage_field(obj: &serde_json::Map<String, Value>) -> bool {
    for key in [
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "speed",
    ] {
        if obj.get(key).is_some_and(Value::is_null) {
            return true;
        }
    }
    false
}

fn parse_cache_split(val: &Value) -> Option<(u64, u64)> {
    let obj = val.as_object()?;
    let e5m = match obj.get("ephemeral_5m_input_tokens") {
        Some(v) => safe_u64(v)?,
        None => 0,
    };
    let e1h = match obj.get("ephemeral_1h_input_tokens") {
        Some(v) => safe_u64(v)?,
        None => 0,
    };
    Some((e5m, e1h))
}

fn sum_message_iteration_splits(val: &Value) -> Option<(u64, u64)> {
    let arr = val.as_array()?;
    let mut sum_5m = 0u64;
    let mut sum_1h = 0u64;
    let mut has_message = false;
    for item in arr {
        let Some(obj) = item.as_object() else {
            continue;
        };
        if obj.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        has_message = true;
        let (e5m, e1h) = parse_cache_split(obj.get("cache_creation")?)?;
        sum_5m = checked_add_safe(sum_5m, e5m)?;
        sum_1h = checked_add_safe(sum_1h, e1h)?;
    }
    has_message.then_some((sum_5m, sum_1h))
}

fn extract_advisor_usages(usage_val: &Value) -> Vec<(String, ParsedUsage)> {
    let Some(arr) = usage_val.get("iterations").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        if item.get("type").and_then(Value::as_str) != Some("advisor_message") {
            continue;
        }
        let Some(model) = valid_ordinary_model(item.get("model")) else {
            continue;
        };
        let Some(usage) = extract_claude_usage(item, false) else {
            continue;
        };
        out.push((model, usage));
    }
    out
}

enum ChainNode {
    Link {
        parent: (String, String),
        owner: Option<usize>,
    },
    Block(usize),
}

struct ChainResponse {
    key: String,
    signature: (String, u64, u64, u64),
}

struct ResponseChains {
    default_session: String,
    nodes: HashMap<(String, String), ChainNode>,
    responses: Vec<ChainResponse>,
    tool_use_owners: HashMap<(String, String), usize>,
}

impl ResponseChains {
    fn new(default_session: &str) -> Self {
        Self {
            default_session: default_session.to_owned(),
            nodes: HashMap::new(),
            responses: Vec::new(),
            tool_use_owners: HashMap::new(),
        }
    }

    fn is_tracking(&self) -> bool {
        !self.nodes.is_empty()
    }

    fn link(&mut self, row: &Value) {
        let Some(uuid) = row
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        let Some(parent_uuid) = row
            .get("parentUuid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        let session = row
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.default_session)
            .to_owned();
        let parent_key = (session.clone(), parent_uuid.to_owned());
        if !self.nodes.contains_key(&parent_key) {
            return;
        }
        let owner = if row.get("type").and_then(Value::as_str) == Some("user") {
            let Some(blocks) = row
                .pointer("/message/content")
                .and_then(Value::as_array)
                .filter(|b| !b.is_empty())
            else {
                return;
            };
            let Some(resp_owner) = self.response_of(&parent_key) else {
                return;
            };
            let all_match = blocks.iter().all(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_result")
                    && block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .is_some_and(|tid| {
                            self.tool_use_owners.get(&(session.clone(), tid.to_owned()))
                                == Some(&resp_owner)
                        })
            });
            if !all_match {
                return;
            }
            Some(resp_owner)
        } else {
            None
        };
        self.nodes.insert(
            (session, uuid.to_owned()),
            ChainNode::Link {
                parent: parent_key,
                owner,
            },
        );
    }

    fn response_key(
        &mut self,
        row: &Value,
        session: &str,
        message_id: &str,
        usage: &ParsedUsage,
    ) -> Option<String> {
        let uuid = row
            .get("uuid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let parent = row
            .get("parentUuid")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let sig = (
            message_id.to_owned(),
            usage.input_tokens,
            usage.cache_creation_tokens,
            usage.cache_read_tokens,
        );
        let joined = parent.and_then(|p| self.chained_response(&(session.to_owned(), p), &sig));
        let idx = match joined {
            Some(idx) => idx,
            None => {
                let root_uuid = uuid.clone()?;
                let idx = self.responses.len();
                self.responses.push(ChainResponse {
                    key: root_uuid,
                    signature: sig,
                });
                idx
            }
        };
        if let Some(blocks) = row.pointer("/message/content").and_then(Value::as_array) {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    if let Some(tid) = block
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        self.tool_use_owners
                            .insert((session.to_owned(), tid.to_owned()), idx);
                    }
                }
            }
        }
        if let Some(u) = uuid {
            self.nodes
                .insert((session.to_owned(), u), ChainNode::Block(idx));
        }
        Some(self.responses[idx].key.clone())
    }

    fn response_of(&self, start: &(String, String)) -> Option<usize> {
        let mut cur = start;
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(cur)? {
                ChainNode::Link { parent, .. } => cur = parent,
                ChainNode::Block(idx) => return Some(*idx),
            }
        }
        None
    }

    fn chained_response(
        &self,
        start: &(String, String),
        sig: &(String, u64, u64, u64),
    ) -> Option<usize> {
        let mut owners = Vec::new();
        let mut cur = start;
        for _ in 0..=self.nodes.len() {
            match self.nodes.get(cur)? {
                ChainNode::Link { parent, owner } => {
                    if let Some(o) = owner {
                        owners.push(*o);
                    }
                    cur = parent;
                }
                ChainNode::Block(idx) => {
                    let ok = self.responses[*idx].signature == *sig
                        && owners.iter().all(|o| o == idx);
                    return ok.then_some(*idx);
                }
            }
        }
        None
    }
}

#[derive(Default)]
struct ClaudeDeduper {
    entries: Vec<ClaudeCandidate>,
    merged: HashMap<usize, usize>,
    by_msg_req: HashMap<(String, String), usize>,
    by_reqless_ts: HashMap<(String, String, i64), usize>,
    by_reqless_resp: HashMap<(String, String, String), usize>,
    by_replay_any: HashMap<(String, String), usize>,
    by_replay_sidechain: HashMap<(String, String), usize>,
    seq: usize,
}

impl ClaudeDeduper {
    fn next_seq(&mut self) -> usize {
        let s = self.seq;
        self.seq += 1;
        s
    }

    fn resolve(&self, mut idx: usize) -> usize {
        while let Some(&next) = self.merged.get(&idx) {
            idx = next;
        }
        idx
    }

    fn push(&mut self, candidate: ClaudeCandidate) {
        let Some(mid) = candidate.message_id.clone() else {
            self.entries.push(candidate);
            return;
        };
        let sess = candidate.session_id.clone();
        let ts_ms = candidate.timestamp.timestamp_millis();
        let cand_req_id = candidate.request_id.clone();
        let cand_resp_key = candidate.response_key.clone();

        let existing = if let Some(ref rid) = cand_req_id {
            self.by_msg_req
                .get(&(mid.clone(), rid.clone()))
                .copied()
                .map(|i| self.resolve(i))
                .or_else(|| {
                    if candidate.is_sidechain {
                        self.by_replay_any
                            .get(&(mid.clone(), sess.clone()))
                            .copied()
                            .map(|i| self.resolve(i))
                    } else {
                        self.by_replay_sidechain
                            .get(&(mid.clone(), sess.clone()))
                            .copied()
                            .map(|i| self.resolve(i))
                    }
                })
        } else {
            let exact = self
                .by_reqless_ts
                .get(&(mid.clone(), sess.clone(), ts_ms))
                .copied()
                .map(|i| self.resolve(i));
            let resp = cand_resp_key
                .as_ref()
                .and_then(|rk| self.by_reqless_resp.get(&(mid.clone(), sess.clone(), rk.clone())))
                .copied()
                .map(|i| self.resolve(i));
            let matched = match (exact, resp) {
                (Some(a), Some(b)) if a != b => Some(self.merge_slots(a, b)),
                (a, b) => a.or(b),
            };
            matched.or_else(|| {
                if candidate.is_sidechain {
                    self.by_replay_any
                        .get(&(mid.clone(), sess.clone()))
                        .copied()
                        .map(|i| self.resolve(i))
                } else {
                    self.by_replay_sidechain
                        .get(&(mid.clone(), sess.clone()))
                        .copied()
                        .map(|i| self.resolve(i))
                }
            })
        };

        let slot = match existing {
            Some(idx) => {
                if should_replace(&candidate, &self.entries[idx]) {
                    self.entries[idx] = candidate;
                }
                idx
            }
            None => {
                let idx = self.entries.len();
                self.entries.push(candidate);
                idx
            }
        };

        // Index both incoming candidate keys and survivor keys so absorbed copies map cleanly.
        if let Some(rid) = cand_req_id {
            self.by_msg_req.entry((mid.clone(), rid)).or_insert(slot);
        } else {
            self.by_reqless_ts
                .entry((mid.clone(), sess.clone(), ts_ms))
                .or_insert(slot);
            if let Some(rk) = cand_resp_key {
                self.by_reqless_resp
                    .entry((mid.clone(), sess.clone(), rk))
                    .or_insert(slot);
            }
        }
        let survivor = &self.entries[slot];
        if let Some(ref rid) = survivor.request_id {
            self.by_msg_req.insert((mid.clone(), rid.clone()), slot);
        } else {
            self.by_reqless_ts.insert(
                (
                    mid.clone(),
                    survivor.session_id.clone(),
                    survivor.timestamp.timestamp_millis(),
                ),
                slot,
            );
            if let Some(ref rk) = survivor.response_key {
                self.by_reqless_resp
                    .insert((mid.clone(), survivor.session_id.clone(), rk.clone()), slot);
            }
        }
        self.by_replay_any
            .entry((mid.clone(), sess))
            .or_insert(slot);
        self.by_replay_any
            .entry((mid.clone(), survivor.session_id.clone()))
            .or_insert(slot);
        if survivor.is_sidechain {
            self.by_replay_sidechain
                .entry((mid, survivor.session_id.clone()))
                .or_insert(slot);
        } else {
            self.by_replay_sidechain
                .remove(&(mid, survivor.session_id.clone()));
        }
    }

    fn merge_slots(&mut self, kept: usize, absorbed: usize) -> usize {
        if should_replace(&self.entries[absorbed], &self.entries[kept]) {
            self.entries.swap(kept, absorbed);
        }
        self.merged.insert(absorbed, kept);
        kept
    }

    fn into_records(self) -> Vec<TokenRecord> {
        let mut out = Vec::with_capacity(self.entries.len());
        for (idx, entry) in self.entries.into_iter().enumerate() {
            if !self.merged.contains_key(&idx) {
                out.push(entry.to_record());
            }
        }
        out
    }
}

fn should_replace(candidate: &ClaudeCandidate, existing: &ClaudeCandidate) -> bool {
    if candidate.is_sidechain != existing.is_sidechain {
        return existing.is_sidechain;
    }
    if candidate.usage.total_tokens != existing.usage.total_tokens {
        return candidate.usage.total_tokens > existing.usage.total_tokens;
    }
    let detail = |c: &ClaudeCandidate| (c.speed_rank, c.has_cache_creation_split, c.has_cost_usd);
    detail(candidate) > detail(existing)
}

fn is_semver_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    let consume_digits = |i: &mut usize| {
        let start = *i;
        while bytes.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i > start
    };
    if !consume_digits(&mut i) || bytes.get(i) != Some(&b'.') {
        return false;
    }
    i += 1;
    if !consume_digits(&mut i) || bytes.get(i) != Some(&b'.') {
        return false;
    }
    i += 1;
    bytes.get(i).is_some_and(u8::is_ascii_digit)
}

fn safe_u64(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n <= MAX_SAFE)
}

fn checked_add_safe(left: u64, right: u64) -> Option<u64> {
    left.checked_add(right).filter(|n| *n <= MAX_SAFE)
}

fn safe_record_id(raw: &str) -> String {
    if !raw.is_empty() && raw.len() <= 1024 && !raw.chars().any(char::is_control) {
        raw.to_owned()
    } else {
        format!("claude:sha256:{:x}", Sha256::digest(raw.as_bytes()))
    }
}

fn identifier(value: &Value, max: usize) -> Result<String, ()> {
    let s = value.as_str().ok_or(())?;
    if s.is_empty()
        || s.len() > max
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
    {
        return Err(());
    }
    Ok(s.to_owned())
}

fn stream_count(value: &Value) -> Result<u64, ()> {
    safe_u64(value).ok_or(())
}

fn update_stream_totals(p: &mut PendingStream, output: u64) -> Result<(), ()> {
    let cached = checked_add_safe(p.creation, p.read).ok_or(())?;
    let total = checked_add_safe(checked_add_safe(p.uncached, output).ok_or(())?, cached)
        .ok_or(())?;
    p.record.input_tokens = p.uncached;
    p.record.output_tokens = output;
    p.record.cache_creation_tokens = p.creation;
    p.record.cache_read_tokens = p.read;
    p.record.cached_tokens = cached;
    p.record.total_tokens = total;
    Ok(())
}

fn consume_stream(
    row: &Value,
    pending: &mut Option<PendingStream>,
    records: &mut BTreeMap<String, TokenRecord>,
) -> Result<(), ()> {
    // Events without a message ID are only attributable in a serial stream.
    if !row["parent_tool_use_id"].is_null() {
        return Err(());
    }
    let event = &row["event"];
    match event["type"].as_str().ok_or(())? {
        "message_start" => {
            if pending.is_some() {
                return Err(());
            }
            let message = &event["message"];
            if message["role"] != "assistant" || message["type"] != "message" {
                return Err(());
            }
            let timestamp = DateTime::parse_from_rfc3339(row["timestamp"].as_str().ok_or(())?)
                .map_err(|_| ())?
                .with_timezone(&Utc);
            let usage = &message["usage"];
            let uncached = stream_count(&usage["input_tokens"])?;
            let read = stream_count(&usage["cache_read_input_tokens"])?;
            let creation = stream_count(&usage["cache_creation_input_tokens"])?;
            let cached = checked_add_safe(creation, read).ok_or(())?;
            checked_add_safe(uncached, cached).ok_or(())?;
            *pending = Some(PendingStream {
                record: TokenRecord {
                    id: format!("claude:{}", identifier(&message["id"], 200)?),
                    timestamp,
                    model: identifier(&message["model"], 128)?,
                    input_tokens: uncached,
                    output_tokens: 0,
                    cached_tokens: cached,
                    cache_creation_tokens: creation,
                    cache_read_tokens: read,
                    total_tokens: 0,
                },
                uncached,
                creation,
                read,
                terminal: false,
            });
        }
        "message_delta" => {
            let p = pending.as_mut().ok_or(())?;
            let usage = &event["usage"];
            let output = stream_count(&usage["output_tokens"])?;
            if output < p.record.output_tokens || p.terminal {
                return Err(());
            }
            for (key, target) in [
                ("input_tokens", &mut p.uncached),
                ("cache_creation_input_tokens", &mut p.creation),
                ("cache_read_input_tokens", &mut p.read),
            ] {
                if let Some(value) = usage.get(key) {
                    *target = stream_count(value)?;
                }
            }
            p.terminal = matches!(
                event["delta"]["stop_reason"].as_str(),
                Some(
                    "end_turn"
                        | "tool_use"
                        | "max_tokens"
                        | "stop_sequence"
                        | "pause_turn"
                        | "refusal"
                )
            );
            update_stream_totals(p, output)?;
        }
        "message_stop" => {
            let p = pending.take().ok_or(())?;
            if !p.terminal {
                return Err(());
            }
            records.insert(p.record.id.clone(), p.record);
        }
        "content_block_start" | "content_block_delta" | "content_block_stop" | "ping" => {}
        _ => return Err(()),
    }
    Ok(())
}
