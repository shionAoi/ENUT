//! Partial accounting only: assistant output counts are documented placeholders.
//! https://code.claude.com/docs/en/agent-sdk/cost-tracking#track-per-step-usage
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const INVALID: &str =
    "Claude input metadata is incomplete or unsupported; output and total remain unknown.";
pub struct InputRecord {
    pub id: String,
    pub model: String,
    pub timestamp: DateTime<Utc>,
    pub input_tokens: u64,
    pub cached_tokens: u64,
}

pub fn parse_inputs(text: &str) -> Result<Vec<InputRecord>, String> {
    if text.len() > 16 * 1024 * 1024 {
        return Err(INVALID.into());
    }
    let mut rows = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if index >= 100_000 {
            return Err(INVALID.into());
        }
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|_| INVALID)?;
        if value["type"].as_str() != Some("assistant") {
            continue;
        }
        let message = &value["message"];
        if message["role"].as_str() != Some("assistant") {
            continue;
        }
        if message["usage"].is_null() {
            continue;
        }
        let field = |key: &str| {
            message[key].as_str().filter(|s| {
                !s.is_empty()
                    && s.len() <= 200
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
            })
        };
        let id = field("id").ok_or(INVALID)?;
        let model = field("model").ok_or(INVALID)?;
        let timestamp = DateTime::parse_from_rfc3339(value["timestamp"].as_str().ok_or(INVALID)?)
            .map_err(|_| INVALID)?
            .to_utc();
        let count = |key: &str| message["usage"][key].as_u64().ok_or(INVALID);
        let cached = count("cache_read_input_tokens")?;
        let input = count("input_tokens")?
            .checked_add(cached)
            .and_then(|v| v.checked_add(count("cache_creation_input_tokens").ok()?))
            .ok_or(INVALID)?;
        rows.insert(
            id.to_string(),
            InputRecord {
                id: id.into(),
                model: model.into(),
                timestamp,
                input_tokens: input,
                cached_tokens: cached,
            },
        );
    }
    Ok(rows.into_values().collect())
}

pub fn load(
    home: &Path,
    now: DateTime<Utc>,
) -> Result<(crate::PartialUsage, Vec<crate::Model>), String> {
    let files = crate::usage::read_session_files(home, "claude")?;
    from_files(&files, now)
}

pub(crate) fn from_files(
    files: &crate::usage::SessionFiles,
    now: DateTime<Utc>,
) -> Result<(crate::PartialUsage, Vec<crate::Model>), String> {
    let mut warnings = files.warnings.clone();
    let mut unique: BTreeMap<String, InputRecord> = BTreeMap::new();
    let start = now
        .date_naive()
        .checked_sub_days(chrono::Days::new(29))
        .ok_or(INVALID)?
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();
    for file in &files.files {
        match parse_inputs(&file.text) {
            Ok(rows) => {
                for row in rows {
                    if row.timestamp < start || row.timestamp > now {
                        continue;
                    }
                    if unique
                        .get(&row.id)
                        .is_none_or(|old| row.timestamp >= old.timestamp)
                    {
                        unique.insert(row.id.clone(), row);
                    }
                }
            }
            Err(_) => warnings.push("Some unsupported session files were excluded.".into()),
        }
    }
    if unique.is_empty() {
        return Err(INVALID.into());
    }
    let mut input = 0u64;
    let mut cached = 0u64;
    let mut models = BTreeSet::new();
    for row in unique.into_values() {
        input = input
            .checked_add(row.input_tokens)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or(INVALID)?;
        cached = cached
            .checked_add(row.cached_tokens)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or(INVALID)?;
        models.insert(row.model);
    }
    warnings.sort();
    warnings.dedup();
    let note = if warnings.is_empty() {
        "Observed recognized input/cache records; output and total tokens are unavailable.".into()
    } else {
        format!(
            "Partial coverage: {} Output and total tokens are unavailable.",
            warnings.join(" ")
        )
    };
    Ok((
        crate::PartialUsage {
            input_tokens: input,
            cached_tokens: cached,
            source: "Claude assistant input/cache metadata (not total consumption)".into(),
            as_of: now.to_rfc3339(),
            period_start: start.to_rfc3339(),
            period_end: now.to_rfc3339(),
            note,
        },
        models
            .into_iter()
            .map(|model| crate::Model {
                id: model.clone(),
                name: model,
                size_bytes: None,
                loaded: None,
            })
            .collect(),
    ))
}
