//! Pure Gemini CLI metadata adapter aligned with ccusage normalization.
//! Schema/replay: https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/services/chatRecordingService.ts
use crate::usage::{ParseResult, TokenRecord};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const INVALID: &str = "Gemini session data is invalid or unsupported.";
const INCOMPLETE: &str = "Gemini usage metadata is incomplete or invalid.";
const DEFAULT_MODEL: &str = "unknown";
const MAX_SAFE: u64 = 9_007_199_254_740_991;

const INPUT_KEYS: &[&str] = &["input", "prompt", "input_tokens", "prompt_tokens"];
const OUTPUT_KEYS: &[&str] = &["output", "candidates", "output_tokens", "candidates_tokens"];
const CACHED_KEYS: &[&str] = &["cached", "cached_tokens"];
const THOUGHTS_KEYS: &[&str] = &[
    "thoughts",
    "reasoning",
    "thoughts_tokens",
    "reasoning_tokens",
];
const TOOL_KEYS: &[&str] = &["tool", "tool_tokens"];
const TOTAL_KEYS: &[&str] = &["total", "total_tokens"];

fn safe_field(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}

fn field(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| safe_field(s))
}

fn checked_add_safe(left: u64, right: u64) -> Option<u64> {
    left.checked_add(right).filter(|n| *n <= MAX_SAFE)
}

/// Read an alias group strictly: every present key in the group must be a valid u64 <= MAX_SAFE.
fn read_alias_u64(tokens: &Map<String, Value>, keys: &[&str]) -> Result<Option<u64>, ()> {
    let mut found = None;
    for key in keys {
        if let Some(val) = tokens.get(*key) {
            let n = val.as_u64().filter(|n| *n <= MAX_SAFE).ok_or(())?;
            if found.is_none() {
                found = Some(n);
            }
        }
    }
    Ok(found)
}

fn parse_timestamp(object: &Map<String, Value>) -> Option<DateTime<Utc>> {
    ["timestamp", "created_at", "createdAt"]
        .iter()
        .find_map(|key| object.get(*key)?.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn token_record(message: &Value, hint: &str) -> Option<TokenRecord> {
    let tokens = message.get("tokens")?.as_object()?;
    let input = read_alias_u64(tokens, INPUT_KEYS).ok()??;
    let output = read_alias_u64(tokens, OUTPUT_KEYS).ok()??;
    let cached = read_alias_u64(tokens, CACHED_KEYS).ok()??;
    let total = read_alias_u64(tokens, TOTAL_KEYS).ok()??;
    let thoughts = read_alias_u64(tokens, THOUGHTS_KEYS).ok()?.unwrap_or(0);
    let tool = read_alias_u64(tokens, TOOL_KEYS).ok()?.unwrap_or(0);

    let inclusive_total = input
        .checked_add(output)
        .and_then(|v| v.checked_add(thoughts))
        .and_then(|v| v.checked_add(tool));
    let exclusive_total = inclusive_total.and_then(|v| v.checked_add(cached));
    let raw_sum_with_cache = input
        .checked_add(tool)
        .and_then(|v| v.checked_add(output))
        .and_then(|v| v.checked_add(cached));

    let inclusive_cached = cached > 0
        && cached <= input
        && ((inclusive_total == Some(total) && exclusive_total != Some(total))
            || raw_sum_with_cache.is_none_or(|s| s > total));

    let input_without_cache = if inclusive_cached {
        input - cached
    } else {
        input
    };
    let input_tokens = checked_add_safe(input_without_cache, tool)?;
    let accounted = checked_add_safe(checked_add_safe(input_tokens, output)?, cached)?;
    if accounted > total {
        return None;
    }

    Some(TokenRecord {
        id: format!("{hint}:{}", field(message.get("id")?)?),
        timestamp: DateTime::parse_from_rfc3339(message.get("timestamp")?.as_str()?)
            .ok()?
            .to_utc(),
        model: field(message.get("model")?)?.to_owned(),
        input_tokens,
        output_tokens: output,
        cached_tokens: cached,
        cache_creation_tokens: 0,
        cache_read_tokens: cached,
        total_tokens: total,
    })
}

fn stats_record(
    tokens: &Map<String, Value>,
    model: &str,
    timestamp: DateTime<Utc>,
    id: String,
) -> Option<TokenRecord> {
    if !safe_field(model) {
        return None;
    }
    let input = read_alias_u64(tokens, INPUT_KEYS).ok()??;
    let output = read_alias_u64(tokens, OUTPUT_KEYS).ok()??;
    let cached = read_alias_u64(tokens, CACHED_KEYS).ok()?.unwrap_or(0);
    let thoughts = read_alias_u64(tokens, THOUGHTS_KEYS).ok()?.unwrap_or(0);
    let tool = read_alias_u64(tokens, TOOL_KEYS).ok()?.unwrap_or(0);
    let reported_total = read_alias_u64(tokens, TOTAL_KEYS).ok()?;

    // Subtract cached overlap from input (ccusage subtract_cached_overlap_tokens).
    let input_without_cache = input.saturating_sub(input.min(cached));
    let input_tokens = checked_add_safe(input_without_cache, tool)?;
    let base_sum = checked_add_safe(checked_add_safe(input_tokens, output)?, cached)?;
    let derived_total = checked_add_safe(base_sum, thoughts)?;
    let total_tokens = match reported_total {
        Some(t) if t >= base_sum => t,
        Some(_) => return None,
        None => derived_total,
    };
    if input_tokens == 0 && output == 0 && cached == 0 && total_tokens == 0 {
        return None;
    }
    let safe_id = if id.len() <= 1024 && !id.chars().any(char::is_control) {
        id
    } else {
        format!("gemini:sha256:{:x}", Sha256::digest(id.as_bytes()))
    };
    Some(TokenRecord {
        id: safe_id,
        timestamp,
        model: model.to_owned(),
        input_tokens,
        output_tokens: output,
        cached_tokens: cached,
        cache_creation_tokens: 0,
        cache_read_tokens: cached,
        total_tokens,
    })
}

#[derive(Default)]
struct Replay {
    order: Vec<String>,
    // Include non-usage messages: rewind targets can be user messages.
    messages: HashMap<String, Option<TokenRecord>>,
    stats_records: Vec<TokenRecord>,
    current_model: Option<String>,
    stats_seq: usize,
    incomplete: bool,
}

impl Replay {
    fn upsert(&mut self, message: &Value, hint: &str) -> Result<(), String> {
        let object = message.as_object().ok_or(INVALID)?;
        if object.keys().any(|k| k.starts_with('$')) {
            return Err(INVALID.into());
        }
        let Some(id) = message.get("id").and_then(field) else {
            self.incomplete = true;
            return Ok(());
        };
        let record = if message.get("type").and_then(Value::as_str) == Some("gemini") {
            let record = token_record(message, hint);
            self.incomplete |= record.is_none();
            record
        } else {
            None
        };
        if !self.messages.contains_key(id) {
            self.order.push(id.to_owned());
        }
        // Full message updates replace, including updates that lose usage metadata.
        self.messages.insert(id.to_owned(), record);
        Ok(())
    }

    fn remove(&mut self, ids: &[String]) {
        let removed: HashSet<_> = ids.iter().collect();
        self.order.retain(|id| !removed.contains(id));
        for id in ids {
            self.messages.remove(id);
        }
    }

    fn checkpoint(&mut self, messages: &Value, hint: &str, replace: bool) -> Result<(), String> {
        let messages = messages.as_array().ok_or(INVALID)?;
        if replace {
            self.order.clear();
            self.messages.clear();
        }
        for message in messages {
            self.upsert(message, hint)?;
        }
        Ok(())
    }

    fn process_stats(&mut self, object: &Map<String, Value>, stats: &Map<String, Value>, hint: &str) {
        let Some(timestamp) = parse_timestamp(object) else {
            self.incomplete = true;
            return;
        };
        let seq = self.stats_seq;
        self.stats_seq += 1;

        if let Some(models) = stats.get("models").and_then(Value::as_object) {
            let mut keys: Vec<&String> = models.keys().collect();
            keys.sort();
            let mut emitted = false;
            for model in keys {
                let Some(tokens) = models[model].get("tokens").and_then(Value::as_object) else {
                    continue;
                };
                if let Some(rec) = stats_record(
                    tokens,
                    model,
                    timestamp,
                    format!("{hint}:stats:{seq}:{model}"),
                ) {
                    self.stats_records.push(rec);
                    emitted = true;
                }
            }
            if emitted {
                return;
            }
        }
        let model = object
            .get("model")
            .and_then(field)
            .or(self.current_model.as_deref())
            .unwrap_or(DEFAULT_MODEL);
        if let Some(rec) = stats_record(stats, model, timestamp, format!("{hint}:stats:{seq}")) {
            self.stats_records.push(rec);
        }
    }

    fn process(&mut self, record: &Value, hint: &str) -> Result<(), String> {
        let object = record.as_object().ok_or(INVALID)?;
        if let Some(m) = object.get("model").and_then(field) {
            self.current_model = Some(m.to_owned());
        }
        let controls: Vec<_> = object.keys().filter(|k| k.starts_with('$')).collect();
        if !controls.is_empty() {
            // Ambiguous or future controls must not silently publish stale counts.
            if controls.len() != 1 || object.len() != 1 {
                return Err(INVALID.into());
            }
            match controls[0].as_str() {
                "$rewindTo" => {
                    let id = record["$rewindTo"]
                        .as_str()
                        .filter(|s| safe_field(s))
                        .ok_or(INVALID)?;
                    let index = self
                        .order
                        .iter()
                        .position(|candidate| candidate == id)
                        .unwrap_or(0);
                    let removed = self.order[index..].to_vec();
                    self.remove(&removed);
                }
                "$patch" => self.patch(&record["$patch"])?,
                "$set" => {
                    let updates = record["$set"].as_object().ok_or(INVALID)?;
                    if updates.keys().any(|k| k.starts_with('$')) {
                        return Err(INVALID.into());
                    }
                    if let Some(messages) = updates.get("messages") {
                        self.checkpoint(messages, hint, true)?;
                    }
                    // Other $set fields only update conversation metadata, not messages.
                }
                _ => return Err(INVALID.into()),
            }
        } else if object.get("type").and_then(Value::as_str) != Some("gemini")
            && object
                .get("stats")
                .or_else(|| object.get("result").and_then(|r| r.get("stats")))
                .and_then(Value::as_object)
                .is_some()
        {
            if let Some(stats) = object
                .get("stats")
                .or_else(|| object.get("result").and_then(|r| r.get("stats")))
                .and_then(Value::as_object)
            {
                self.process_stats(object, stats, hint);
            }
        } else if object.contains_key("id") {
            self.upsert(record, hint)?;
        } else if record.get("sessionId").and_then(Value::as_str).is_some()
            && (record.get("projectHash").and_then(Value::as_str).is_some()
                || record.get("messages").is_some())
        {
            if let Some(messages) = record.get("messages") {
                self.checkpoint(messages, hint, false)?;
            }
        } else {
            return Err(INVALID.into());
        }
        Ok(())
    }

    fn patch(&mut self, value: &Value) -> Result<(), String> {
        let patch = value.as_object().ok_or(INVALID)?;
        allowed_keys(
            patch,
            &[
                "id",
                "content",
                "toolCalls",
                "updates",
                "removeIds",
                "orderIds",
            ],
        )?;
        validate_content_patch(patch, false)?;
        if let Some(updates) = patch.get("updates") {
            for update in updates.as_array().ok_or(INVALID)? {
                let update = update.as_object().ok_or(INVALID)?;
                allowed_keys(update, &["id", "content", "toolCalls"])?;
                validate_content_patch(update, true)?;
            }
        }
        if let Some(ids) = patch.get("removeIds") {
            self.remove(&id_list(ids)?);
        }
        if let Some(ids) = patch.get("orderIds") {
            let ids = id_list(ids)?;
            let mut seen = HashSet::new();
            let suffix: Vec<_> = ids
                .into_iter()
                .filter(|id| self.messages.contains_key(id) && seen.insert(id.clone()))
                .collect();
            self.order.retain(|id| !seen.contains(id));
            self.order.extend(suffix);
        }
        // Verified patches change content/tool results only; never token/model fields.
        Ok(())
    }
}

fn allowed_keys(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(INVALID.into());
    }
    Ok(())
}

fn validate_content_patch(patch: &Map<String, Value>, require_id: bool) -> Result<(), String> {
    if require_id || patch.contains_key("id") {
        patch.get("id").and_then(field).ok_or(INVALID)?;
    }
    if let Some(tools) = patch.get("toolCalls") {
        for tool in tools.as_array().ok_or(INVALID)? {
            let tool = tool.as_object().ok_or(INVALID)?;
            allowed_keys(tool, &["id", "result"])?;
            tool.get("id").and_then(field).ok_or(INVALID)?;
        }
    }
    Ok(())
}

fn id_list(value: &Value) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or(INVALID)?
        .iter()
        .map(|v| {
            field(v)
                .map(str::to_owned)
                .ok_or_else(|| INVALID.to_owned())
        })
        .collect()
}

/// Parse approved file contents only; the caller owns consent and bounded file IO.
/// `session_hint` must be a stable, non-private namespace shared by JSON/JSONL twins.
pub fn parse(text: &str, session_hint: &str) -> Result<ParseResult, String> {
    if !safe_field(session_hint)
        || session_hint.contains(['/', ':'])
        || session_hint == "."
        || session_hint == ".."
    {
        return Err("Gemini session identifier is invalid.".into());
    }
    if text.trim().is_empty() || text.len() > 16 * 1024 * 1024 {
        return Err(INVALID.into());
    }
    let mut replay = Replay::default();
    if let Ok(record) = serde_json::from_str::<Value>(text) {
        if record.is_array() {
            // Silently ignore top-level arrays such as logs.json.
            return Ok(ParseResult::default());
        }
        replay.process(&record, session_hint)?;
    } else {
        for (index, line) in text.lines().enumerate() {
            if index >= 100_000 {
                return Err(INVALID.into());
            }
            if line.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(line).map_err(|_| INVALID)?;
            replay.process(&record, session_hint)?;
        }
    }
    let mut records: Vec<TokenRecord> = replay
        .order
        .into_iter()
        .filter_map(|id| replay.messages.remove(&id).flatten())
        .collect();
    records.extend(replay.stats_records);
    Ok(ParseResult {
        records,
        warnings: if replay.incomplete {
            vec![INCOMPLETE.into()]
        } else {
            vec![]
        },
    })
}
