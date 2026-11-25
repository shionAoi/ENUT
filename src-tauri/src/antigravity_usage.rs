//! Local read-only Antigravity SQLite + Protobuf usage adapter (ccusage method).
use crate::usage::{
    aggregate_window, open_absolute_nofollow, TokenRecord, UsageSummary, RETAINED_USAGE_DAYS,
};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::ErrorKind,
    path::{Path, PathBuf},
};

const SOURCE: &str = "Antigravity local conversations (ccusage method)";
const DEFAULT_MODEL: &str = "gemini-internal-model";
const UNRESOLVED_PLACEHOLDER_PREFIX: &str = "model_placeholder_m";
const UNRESOLVED_ID_PREFIX: &str = "antigravity-model-";
const MAX_SAFE: u64 = 9_007_199_254_740_991;
const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 4;

const DEFAULT_ROOTS: [&str; 6] = [
    ".gemini/antigravity",
    ".gemini/antigravity-cli",
    ".gemini/antigravity-ide",
    ".gemini/antigravity-backup",
    ".gemini/antigravity-acp",
    ".config/antigravity",
];

pub fn load(home: &Path, now: DateTime<Utc>) -> Result<UsageSummary, String> {
    load_until(home, now, &|| false)
}

pub(crate) fn load_until(
    home: &Path,
    now: DateTime<Utc>,
    cancelled: &dyn Fn() -> bool,
) -> Result<UsageSummary, String> {
    if !home.is_absolute() {
        return Err("Usage home directory must be absolute.".into());
    }
    let roots: Vec<PathBuf> = DEFAULT_ROOTS
        .iter()
        .map(|root| home.join(root).join("conversations"))
        .collect();
    load_from_roots(&roots, now, cancelled)
}

pub fn load_from_roots(
    roots: &[PathBuf],
    now: DateTime<Utc>,
    cancelled: &dyn Fn() -> bool,
) -> Result<UsageSummary, String> {
    if cancelled() {
        return Err("Local usage scan was cancelled.".into());
    }
    let mut warnings = Vec::new();
    let mut db_paths = Vec::new();
    let mut seen_canonical = HashSet::new();
    let mut accessible_roots = 0usize;
    let mut visited_entries = 0usize;

    for root in roots {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        let root_dir = match open_absolute_nofollow(root) {
            Ok(dir) => {
                accessible_roots += 1;
                dir
            }
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(_) => {
                warnings.push("Linked or unavailable Antigravity directory was skipped.".into());
                continue;
            }
        };

        let (scan_dir, scan_base) = match root_dir.symlink_metadata("conversations") {
            Ok(meta) if meta.file_type().is_symlink() => {
                warnings.push("Linked or unavailable Antigravity directory was skipped.".into());
                continue;
            }
            Ok(meta) if meta.file_type().is_dir() => {
                match root_dir.open_dir_nofollow("conversations") {
                    Ok(child) => (child, root.join("conversations")),
                    Err(_) => {
                        warnings.push(
                            "Linked or unavailable Antigravity directory was skipped.".into(),
                        );
                        continue;
                    }
                }
            }
            _ => (root_dir, root.clone()),
        };

        collect_db_files(
            &scan_dir,
            &scan_base,
            0,
            &mut visited_entries,
            &mut seen_canonical,
            &mut db_paths,
            &mut warnings,
            cancelled,
        )?;
    }

    if accessible_roots == 0 {
        return Err(
            "Approved local Antigravity directory unavailable; linked directories are not followed."
                .into(),
        );
    }

    db_paths.sort();
    let mut events = Vec::new();
    for db_path in db_paths {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        match parse_sqlite_file(&db_path) {
            Ok(parsed) => events.extend(parsed),
            Err(()) => warnings.push("An unreadable Antigravity database was skipped.".into()),
        }
    }

    let mut merged = deduplicate_events(events);
    merged.sort_by_key(|event| event.timestamp);

    let mut records = Vec::new();
    for (slot_idx, event) in merged.into_iter().enumerate() {
        if let Some(record) = event_to_record(event, slot_idx) {
            records.push(record);
        }
    }

    let mut summary = aggregate_window(records, now, SOURCE, RETAINED_USAGE_DAYS)?;
    warnings.sort();
    warnings.dedup();
    summary.warnings = warnings;
    Ok(summary)
}

#[allow(clippy::too_many_arguments)]
fn collect_db_files(
    dir: &Dir,
    base: &Path,
    depth: usize,
    visited: &mut usize,
    seen_canonical: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if depth > MAX_DEPTH || *visited >= MAX_ENTRIES {
        warnings.push("Antigravity scan limits reached; observed usage is incomplete.".into());
        return Ok(());
    }
    let Ok(entries) = dir.entries() else {
        return Ok(());
    };
    for entry in entries {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        if *visited >= MAX_ENTRIES {
            warnings.push("Antigravity scan limits reached; observed usage is incomplete.".into());
            break;
        }
        *visited += 1;
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let Ok(meta) = dir.symlink_metadata(&name) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            warnings.push("Linked Antigravity entry was skipped.".into());
            continue;
        }
        let full = base.join(&name);
        if meta.file_type().is_dir() {
            let Ok(child) = dir.open_dir_nofollow(&name) else {
                warnings.push("Linked or unavailable Antigravity directory was skipped.".into());
                continue;
            };
            collect_db_files(
                &child,
                &full,
                depth + 1,
                visited,
                seen_canonical,
                out,
                warnings,
                cancelled,
            )?;
        } else if meta.file_type().is_file()
            && full.extension().and_then(|e| e.to_str()) == Some("db")
        {
            let canonical = std::fs::canonicalize(&full).unwrap_or_else(|_| full.clone());
            if seen_canonical.insert(canonical) {
                out.push(full);
            }
        }
    }
    Ok(())
}

// --- Domain structures & deduplication ---

#[derive(Debug, Clone)]
struct AntigravityUsageEvent {
    timestamp: DateTime<Utc>,
    session_id: String,
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_output_tokens: u64,
    total_tokens: u64,
    message_id: Option<String>,
    identities: Vec<String>,
    timestamp_rank: u8,
    message_id_rank: u8,
}

#[derive(Debug, Default)]
struct GeneratorMetadata {
    model: Option<String>,
    model_id: Option<u64>,
    usage: Option<ModelUsage>,
    retry_usages: Vec<ModelUsage>,
    timestamp: Option<DateTime<Utc>>,
    step_indices: Vec<i64>,
}

#[derive(Debug, Default)]
struct StepMetadata {
    model: Option<String>,
    model_id: Option<u64>,
    usage: Option<ModelUsage>,
    retry_usages: Vec<ModelUsage>,
    timestamp: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
struct ModelUsage {
    model_id: Option<u64>,
    input_tokens: u64,
    total_output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    visible_output_tokens: u64,
    message_id: Option<String>,
    response_id: Option<String>,
    provider_assigned_message_id: Option<String>,
}

impl ModelUsage {
    fn is_token_bearing(&self) -> bool {
        self.input_tokens > 0
            || self.total_output_tokens > 0
            || self.cache_creation_tokens > 0
            || self.cache_read_tokens > 0
            || self.reasoning_tokens > 0
            || self.visible_output_tokens > 0
    }

    fn same_token_counts(&self, other: &Self) -> bool {
        self.model_id == other.model_id
            && self.input_tokens == other.input_tokens
            && self.total_output_tokens == other.total_output_tokens
            && self.cache_creation_tokens == other.cache_creation_tokens
            && self.cache_read_tokens == other.cache_read_tokens
            && self.reasoning_tokens == other.reasoning_tokens
            && self.visible_output_tokens == other.visible_output_tokens
    }

    fn is_mirror_of(&self, primary: &Self) -> bool {
        let primary_keys = primary.identity_keys();
        self.same_token_counts(primary)
            && self
                .identity_keys()
                .iter()
                .all(|identity| primary_keys.contains(identity))
    }

    fn identity_keys(&self) -> Vec<String> {
        [
            self.response_id
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(|v| format!("response:{v}")),
            self.provider_assigned_message_id
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(|v| format!("provider:{v}")),
            self.message_id
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(|v| format!("message:{v}")),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn preferred_message_id(&self) -> (Option<String>, u8) {
        if let Some(v) = self.response_id.as_deref().filter(|v| !v.is_empty()) {
            return (Some(v.to_string()), 3);
        }
        if let Some(v) = self
            .provider_assigned_message_id
            .as_deref()
            .filter(|v| !v.is_empty())
        {
            return (Some(v.to_string()), 2);
        }
        (
            self.message_id
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(str::to_owned),
            1,
        )
    }
}

#[derive(Clone, Copy)]
struct EventContext<'a> {
    model: Option<&'a str>,
    model_id: Option<u64>,
    timestamp: Option<(DateTime<Utc>, u8)>,
    trajectory_timestamp: Option<DateTime<Utc>>,
    session_id: &'a str,
    fallback_identities: &'a [String],
}

fn append_usage_event(
    events: &mut Vec<AntigravityUsageEvent>,
    identity_timestamps: &mut HashMap<String, (DateTime<Utc>, u8)>,
    usage: ModelUsage,
    context: EventContext<'_>,
) {
    if !usage.is_token_bearing() {
        return;
    }
    let mut identities = usage.identity_keys();
    if identities.is_empty() {
        identities.extend(context.fallback_identities.iter().cloned());
    }
    let Some((timestamp, timestamp_rank)) = context
        .timestamp
        .or_else(|| {
            identities
                .iter()
                .chain(context.fallback_identities)
                .find_map(|id| identity_timestamps.get(id).copied())
        })
        .or_else(|| context.trajectory_timestamp.map(|ts| (ts, 1)))
    else {
        return;
    };

    let total_output_tokens = usage.total_output_tokens.max(
        usage
            .visible_output_tokens
            .saturating_add(usage.reasoning_tokens),
    );
    let output_tokens = usage
        .visible_output_tokens
        .max(total_output_tokens.saturating_sub(usage.reasoning_tokens));
    let reasoning_tokens = usage
        .reasoning_tokens
        .max(total_output_tokens.saturating_sub(output_tokens));

    let id_model = usage.model_id.and_then(model_from_id);
    let row_model = context.model.and_then(normalize_antigravity_model);
    let same_model_id = context.model_id == usage.model_id;
    let model = id_model
        .clone()
        .filter(|m| is_resolved_model(m))
        .or_else(|| {
            row_model
                .as_ref()
                .filter(|m| same_model_id && is_resolved_model(m))
                .cloned()
        })
        .or(id_model)
        .or(row_model)
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let (message_id, message_id_rank) = usage.preferred_message_id();
    for identity in &identities {
        let should_update =
            identity_timestamps
                .get(identity)
                .is_none_or(|(old_ts, old_rank)| {
                    timestamp_rank > *old_rank
                        || (timestamp_rank == *old_rank && timestamp < *old_ts)
                });
        if should_update {
            identity_timestamps.insert(identity.clone(), (timestamp, timestamp_rank));
        }
    }

    events.push(AntigravityUsageEvent {
        timestamp,
        session_id: context.session_id.to_string(),
        model,
        input_tokens: usage.input_tokens,
        output_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        reasoning_tokens,
        total_output_tokens,
        total_tokens: usage
            .input_tokens
            .saturating_add(usage.cache_creation_tokens)
            .saturating_add(usage.cache_read_tokens)
            .saturating_add(total_output_tokens),
        message_id,
        identities,
        timestamp_rank,
        message_id_rank,
    });
}

fn deduplicate_events(
    events: impl IntoIterator<Item = AntigravityUsageEvent>,
) -> Vec<AntigravityUsageEvent> {
    let mut slots: Vec<Option<AntigravityUsageEvent>> = Vec::new();
    let mut identity_indexes: HashMap<String, usize> = HashMap::new();
    for event in events {
        let mut matching: Vec<usize> = event
            .identities
            .iter()
            .filter_map(|id| identity_indexes.get(id).copied())
            .collect();
        matching.sort_unstable();
        matching.dedup();
        let Some(target_idx) = matching.first().copied() else {
            let idx = slots.len();
            for id in &event.identities {
                identity_indexes.insert(id.clone(), idx);
            }
            slots.push(Some(event));
            continue;
        };

        for dup_idx in matching.into_iter().skip(1) {
            if let Some(dup) = slots[dup_idx].take() {
                if let Some(target) = slots[target_idx].as_mut() {
                    merge_usage_event(target, dup);
                }
            }
        }
        if let Some(target) = slots[target_idx].as_mut() {
            merge_usage_event(target, event);
            for id in &target.identities {
                identity_indexes.insert(id.clone(), target_idx);
            }
        }
    }
    slots.into_iter().flatten().collect()
}

fn merge_usage_event(target: &mut AntigravityUsageEvent, duplicate: AntigravityUsageEvent) {
    target.input_tokens = target.input_tokens.max(duplicate.input_tokens);
    target.output_tokens = target.output_tokens.max(duplicate.output_tokens);
    target.cache_creation_tokens = target
        .cache_creation_tokens
        .max(duplicate.cache_creation_tokens);
    target.cache_read_tokens = target.cache_read_tokens.max(duplicate.cache_read_tokens);
    target.reasoning_tokens = target.reasoning_tokens.max(duplicate.reasoning_tokens);
    target.total_output_tokens = target
        .total_output_tokens
        .max(duplicate.total_output_tokens)
        .max(target.output_tokens.saturating_add(target.reasoning_tokens));
    target.total_tokens = target
        .input_tokens
        .saturating_add(target.cache_creation_tokens)
        .saturating_add(target.cache_read_tokens)
        .saturating_add(target.total_output_tokens);
    if model_resolution(&duplicate.model) > model_resolution(&target.model) {
        target.model = duplicate.model;
    }
    let use_duplicate_ts = duplicate.timestamp_rank > target.timestamp_rank
        || (duplicate.timestamp_rank == target.timestamp_rank
            && duplicate.timestamp < target.timestamp);
    if use_duplicate_ts {
        target.timestamp = duplicate.timestamp;
        target.timestamp_rank = duplicate.timestamp_rank;
    }
    if duplicate.message_id_rank > target.message_id_rank {
        target.message_id = duplicate.message_id;
        target.message_id_rank = duplicate.message_id_rank;
    }
    for identity in duplicate.identities {
        if !target.identities.contains(&identity) {
            target.identities.push(identity);
        }
    }
}

fn event_to_record(event: AntigravityUsageEvent, slot_idx: usize) -> Option<TokenRecord> {
    if event.model.is_empty()
        || event.model.len() > 512
        || event.model.chars().any(char::is_control)
    {
        return None;
    }
    let cached = event
        .cache_creation_tokens
        .checked_add(event.cache_read_tokens)?;
    let total = event
        .input_tokens
        .checked_add(event.total_output_tokens)?
        .checked_add(cached)?;
    if total == 0 || total > MAX_SAFE {
        return None;
    }
    let preferred_key = event
        .identities
        .iter()
        .find(|k| k.starts_with("response:"))
        .or_else(|| event.identities.iter().find(|k| k.starts_with("provider:")))
        .or_else(|| event.identities.iter().find(|k| k.starts_with("message:")))
        .or_else(|| event.identities.first());
    let id = match preferred_key {
        Some(key) => bounded_id("antigravity", key),
        None => bounded_id(
            "antigravity",
            &format!("anon:{}:{slot_idx}", event.session_id),
        ),
    };
    Some(TokenRecord {
        id,
        timestamp: event.timestamp,
        model: event.model,
        input_tokens: event.input_tokens,
        output_tokens: event.total_output_tokens,
        cached_tokens: cached,
        cache_creation_tokens: event.cache_creation_tokens,
        cache_read_tokens: event.cache_read_tokens,
        total_tokens: total,
    })
}

fn bounded_id(prefix: &str, raw: &str) -> String {
    let candidate = format!("{prefix}:{raw}");
    if candidate.len() <= 1024 {
        return candidate;
    }
    let digest = Sha256::digest(raw.as_bytes());
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    format!("{prefix}:sha256:{hex}")
}

// --- SQLite reader ---

fn open_sqlite_readonly(path: &Path) -> Result<Connection, ()> {
    crate::usage::open_sqlite_readonly_nofollow(path)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, ()> {
    let exists: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|_| ())?;
    Ok(exists != 0)
}

fn parse_sqlite_file(path: &Path) -> Result<Vec<AntigravityUsageEvent>, ()> {
    let conn = open_sqlite_readonly(path)?;
    conn.execute_batch("BEGIN DEFERRED TRANSACTION;")
        .map_err(|_| ())?;
    let page_count: i64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .map_err(|_| ())?;
    if page_count == 0 {
        return Ok(Vec::new());
    }
    if !table_exists(&conn, "gen_metadata")? {
        if table_exists(&conn, "steps")? || table_exists(&conn, "trajectory_metadata_blob")? {
            return Err(());
        }
        return Ok(Vec::new());
    }
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let trajectory_timestamp = read_trajectory_timestamp(&conn)?;
    let generation_rows = read_generation_rows(&conn)?;
    let step_rows = read_step_rows(&conn)?;
    let generation_model = generation_rows
        .iter()
        .rev()
        .find_map(|(_, meta)| row_model(meta.model.as_deref(), meta.model_id));

    let mut events = Vec::new();
    let mut identity_timestamps: HashMap<String, (DateTime<Utc>, u8)> = HashMap::new();

    for (idx, meta) in &step_rows {
        if let Some(ts) = meta.timestamp {
            let mut keys = vec![format!("step:{session_id}:{idx}")];
            for u in meta.usage.iter().chain(&meta.retry_usages) {
                keys.extend(u.identity_keys());
            }
            for key in keys {
                let entry = identity_timestamps.entry(key).or_insert((ts, 3));
                if ts < entry.0 {
                    entry.0 = ts;
                }
            }
        }
    }
    for (idx, meta) in &generation_rows {
        if let Some(ts) = meta.timestamp {
            let mut keys = generation_identities(&session_id, *idx, &meta.step_indices);
            for u in meta.usage.iter().chain(&meta.retry_usages) {
                keys.extend(u.identity_keys());
            }
            for key in keys {
                let entry = identity_timestamps.entry(key).or_insert((ts, 3));
                if ts < entry.0 {
                    entry.0 = ts;
                }
            }
        }
    }

    for (idx, meta) in step_rows {
        let model =
            row_model(meta.model.as_deref(), meta.model_id).or_else(|| generation_model.clone());
        let timestamp = meta.timestamp.map(|ts| (ts, 3));
        let fallback_identities = [format!("step:{session_id}:{idx}")];
        if let Some(usage) = meta.usage {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: meta.model_id,
                    timestamp,
                    trajectory_timestamp,
                    session_id: &session_id,
                    fallback_identities: &fallback_identities,
                },
            );
        }
        for usage in meta.retry_usages {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: meta.model_id,
                    timestamp,
                    trajectory_timestamp,
                    session_id: &session_id,
                    fallback_identities: &[],
                },
            );
        }
    }

    let mut current_model = None;
    for (idx, meta) in generation_rows {
        let resolved_model = row_model(meta.model.as_deref(), meta.model_id).or_else(|| {
            meta.usage
                .as_ref()
                .and_then(|u| u.model_id)
                .and_then(model_from_id)
        });
        if let Some(m) = resolved_model {
            current_model = Some(m);
        }
        let model = current_model.clone();
        let fallback_identities = generation_identities(&session_id, idx, &meta.step_indices);
        let timestamp = meta.timestamp.map(|ts| (ts, 3));
        if let Some(usage) = meta.usage {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: meta.model_id,
                    timestamp,
                    trajectory_timestamp,
                    session_id: &session_id,
                    fallback_identities: &fallback_identities,
                },
            );
        }
        for usage in meta.retry_usages {
            append_usage_event(
                &mut events,
                &mut identity_timestamps,
                usage,
                EventContext {
                    model: model.as_deref(),
                    model_id: meta.model_id,
                    timestamp,
                    trajectory_timestamp,
                    session_id: &session_id,
                    fallback_identities: &[],
                },
            );
        }
    }

    Ok(events)
}

fn generation_identities(session_id: &str, idx: i64, step_indices: &[i64]) -> Vec<String> {
    if step_indices.is_empty() {
        vec![format!("gen:{session_id}:{idx}")]
    } else {
        step_indices
            .iter()
            .map(|s| format!("step:{session_id}:{s}"))
            .collect()
    }
}

fn read_generation_rows(conn: &Connection) -> Result<Vec<(i64, GeneratorMetadata)>, ()> {
    let mut stmt = conn
        .prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC")
        .map_err(|_| ())?;
    let mut rows = stmt.query([]).map_err(|_| ())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|_| ())? {
        let idx: i64 = row.get(0).map_err(|_| ())?;
        let blob: Vec<u8> = row.get(1).map_err(|_| ())?;
        let meta = parse_generator_metadata(&blob).map_err(|_| ())?;
        out.push((idx, meta));
    }
    Ok(out)
}

fn read_step_rows(conn: &Connection) -> Result<Vec<(i64, StepMetadata)>, ()> {
    if !table_exists(conn, "steps")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT idx, metadata FROM steps WHERE metadata IS NOT NULL ORDER BY idx ASC")
        .map_err(|_| ())?;
    let mut rows = stmt.query([]).map_err(|_| ())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|_| ())? {
        let idx: i64 = row.get(0).map_err(|_| ())?;
        let blob: Vec<u8> = row.get(1).map_err(|_| ())?;
        let meta = parse_step_metadata(&blob).map_err(|_| ())?;
        out.push((idx, meta));
    }
    Ok(out)
}

fn read_trajectory_timestamp(conn: &Connection) -> Result<Option<DateTime<Utc>>, ()> {
    if !table_exists(conn, "trajectory_metadata_blob")? {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare("SELECT data FROM trajectory_metadata_blob ORDER BY rowid ASC")
        .map_err(|_| ())?;
    let mut rows = stmt.query([]).map_err(|_| ())?;
    let mut timestamp = None;
    while let Some(row) = rows.next().map_err(|_| ())? {
        let blob: Vec<u8> = row.get(0).map_err(|_| ())?;
        let candidate = parse_trajectory_timestamp(&blob).map_err(|_| ())?;
        if timestamp.is_none() {
            timestamp = candidate;
        }
    }
    Ok(timestamp)
}

// --- Zero-dependency Protobuf wire decoder ---

#[derive(Clone, Copy)]
enum ProtoValue<'a> {
    Varint(u64),
    Fixed64,
    Bytes(&'a [u8]),
    Fixed32,
}

#[derive(Clone, Copy)]
struct ProtoField<'a> {
    number: u32,
    value: ProtoValue<'a>,
}

type ProtoResult<T> = Result<T, &'static str>;

fn parse_generator_metadata(blob: &[u8]) -> ProtoResult<GeneratorMetadata> {
    let root = decode_fields(blob)?;
    let chat_model = field_bytes(&root, 1).ok_or("missing chat model field")?;
    let chat_fields = decode_fields(chat_model)?;
    let usage = field_bytes(&chat_fields, 4)
        .map(parse_model_usage)
        .transpose()?;
    let mut retry_usages = field_bytes_all(&chat_fields, 17)
        .into_iter()
        .map(parse_retry_info)
        .collect::<ProtoResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    drop_primary_mirror(usage.as_ref(), &mut retry_usages);
    let timestamp = field_bytes(&chat_fields, 9)
        .map(parse_generation_info_timestamp)
        .transpose()?
        .flatten();
    let model = [19, 21]
        .into_iter()
        .find_map(|num| field_text(&chat_fields, num));
    let model_id = field_varint(&chat_fields, 3).filter(|v| *v != 0);
    let step_indices = parse_step_indices(&root);
    Ok(GeneratorMetadata {
        model,
        model_id,
        usage,
        retry_usages,
        timestamp,
        step_indices,
    })
}

fn drop_primary_mirror(primary: Option<&ModelUsage>, retries: &mut Vec<ModelUsage>) {
    if let Some(primary) = primary {
        if let Some(pos) = retries.iter().position(|retry| retry.is_mirror_of(primary)) {
            retries.remove(pos);
        }
    }
}

fn parse_step_indices(root: &[ProtoField<'_>]) -> Vec<i64> {
    let mut indices = Vec::new();
    for field in root {
        if field.number != 2 {
            continue;
        }
        match field.value {
            ProtoValue::Varint(v) => {
                if let Ok(idx) = i64::try_from(v) {
                    indices.push(idx);
                }
            }
            ProtoValue::Bytes(mut packed) => {
                while !packed.is_empty() {
                    match read_varint(&mut packed) {
                        Ok(v) => match i64::try_from(v) {
                            Ok(idx) => indices.push(idx),
                            Err(_) => break,
                        },
                        Err(_) => break,
                    }
                }
            }
            ProtoValue::Fixed32 | ProtoValue::Fixed64 => {}
        }
    }
    indices
}

fn parse_step_metadata(blob: &[u8]) -> ProtoResult<StepMetadata> {
    let fields = decode_fields(blob)?;
    let usage = field_bytes(&fields, 9)
        .map(parse_model_usage)
        .transpose()?;
    let mut retry_usages = field_bytes_all(&fields, 28)
        .into_iter()
        .map(parse_retry_info)
        .collect::<ProtoResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    drop_primary_mirror(usage.as_ref(), &mut retry_usages);
    let (model, model_id) = match field_bytes(&fields, 24) {
        Some(info_blob) => {
            let info = decode_fields(info_blob)?;
            (
                field_text(&info, 12).or_else(|| field_text(&info, 8)),
                field_varint(&info, 1).filter(|v| *v != 0),
            )
        }
        None => (None, None),
    };
    let timestamp = field_bytes(&fields, 8)
        .or_else(|| field_bytes(&fields, 1))
        .map(parse_timestamp_message)
        .transpose()?
        .flatten();
    Ok(StepMetadata {
        model,
        model_id,
        usage,
        retry_usages,
        timestamp,
    })
}

fn parse_retry_info(blob: &[u8]) -> ProtoResult<Option<ModelUsage>> {
    let fields = decode_fields(blob)?;
    field_bytes(&fields, 2).map(parse_model_usage).transpose()
}

fn parse_model_usage(blob: &[u8]) -> ProtoResult<ModelUsage> {
    let fields = decode_fields(blob)?;
    Ok(ModelUsage {
        model_id: field_varint(&fields, 1).filter(|v| *v != 0),
        input_tokens: field_varint(&fields, 2).unwrap_or(0),
        total_output_tokens: field_varint(&fields, 3).unwrap_or(0),
        cache_creation_tokens: field_varint(&fields, 4).unwrap_or(0),
        cache_read_tokens: field_varint(&fields, 5).unwrap_or(0),
        reasoning_tokens: field_varint(&fields, 9).unwrap_or(0),
        visible_output_tokens: field_varint(&fields, 10).unwrap_or(0),
        message_id: field_text(&fields, 7),
        response_id: field_text(&fields, 11),
        provider_assigned_message_id: field_text(&fields, 12),
    })
}

fn parse_generation_info_timestamp(blob: &[u8]) -> ProtoResult<Option<DateTime<Utc>>> {
    let fields = decode_fields(blob)?;
    match field_bytes(&fields, 4) {
        Some(ts_blob) => parse_timestamp_message(ts_blob),
        None => Ok(None),
    }
}

fn parse_trajectory_timestamp(blob: &[u8]) -> ProtoResult<Option<DateTime<Utc>>> {
    let fields = decode_fields(blob)?;
    field_bytes(&fields, 2)
        .map(parse_timestamp_message)
        .transpose()
        .map(Option::flatten)
}

fn parse_timestamp_message(blob: &[u8]) -> ProtoResult<Option<DateTime<Utc>>> {
    let fields = decode_fields(blob)?;
    let Some(seconds) = field_varint(&fields, 1)
        .and_then(|v| i64::try_from(v).ok())
        .filter(|s| *s > 0)
    else {
        return Ok(None);
    };
    let nanos = field_varint(&fields, 2).unwrap_or(0).min(999_999_999);
    let millis = seconds
        .saturating_mul(1_000)
        .saturating_add((nanos / 1_000_000) as i64);
    Ok(DateTime::from_timestamp_millis(millis))
}

fn decode_fields(mut blob: &[u8]) -> ProtoResult<Vec<ProtoField<'_>>> {
    let mut fields = Vec::new();
    while !blob.is_empty() {
        let tag = read_varint(&mut blob)?;
        let number = u32::try_from(tag >> 3).map_err(|_| "protobuf field number overflow")?;
        if number == 0 {
            return Err("protobuf field number is zero");
        }
        let value = match tag & 7 {
            0 => ProtoValue::Varint(read_varint(&mut blob)?),
            1 => {
                take_bytes(&mut blob, 8)?;
                ProtoValue::Fixed64
            }
            2 => ProtoValue::Bytes(take_length_delimited(&mut blob)?),
            5 => {
                take_bytes(&mut blob, 4)?;
                ProtoValue::Fixed32
            }
            _ => return Err("unsupported protobuf wire type"),
        };
        fields.push(ProtoField { number, value });
    }
    Ok(fields)
}

fn read_varint(blob: &mut &[u8]) -> ProtoResult<u64> {
    let mut value = 0_u64;
    for shift in (0..10).map(|i| i * 7) {
        let byte = *blob.first().ok_or("truncated protobuf varint")?;
        *blob = &blob[1..];
        let payload = u64::from(byte & 0x7f);
        if shift == 63 && payload > 1 {
            return Err("protobuf varint overflow");
        }
        value |= payload << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        if shift == 63 {
            return Err("protobuf varint overflow");
        }
    }
    Err("protobuf varint overflow")
}

fn take_bytes<'a>(blob: &mut &'a [u8], length: usize) -> ProtoResult<&'a [u8]> {
    if blob.len() < length {
        return Err("truncated protobuf fixed-width value");
    }
    let (value, rest) = blob.split_at(length);
    *blob = rest;
    Ok(value)
}

fn take_length_delimited<'a>(blob: &mut &'a [u8]) -> ProtoResult<&'a [u8]> {
    let length = usize::try_from(read_varint(blob)?).map_err(|_| "protobuf length overflow")?;
    take_bytes(blob, length)
}

fn field_varint(fields: &[ProtoField<'_>], number: u32) -> Option<u64> {
    fields.iter().rev().find_map(|field| match field {
        ProtoField {
            number: n,
            value: ProtoValue::Varint(v),
        } if *n == number => Some(*v),
        _ => None,
    })
}

fn field_bytes<'a>(fields: &'a [ProtoField<'a>], number: u32) -> Option<&'a [u8]> {
    fields.iter().find_map(|field| match field {
        ProtoField {
            number: n,
            value: ProtoValue::Bytes(v),
        } if *n == number => Some(*v),
        _ => None,
    })
}

fn field_bytes_all<'a>(fields: &'a [ProtoField<'a>], number: u32) -> Vec<&'a [u8]> {
    fields
        .iter()
        .filter_map(|field| match field {
            ProtoField {
                number: n,
                value: ProtoValue::Bytes(v),
            } if *n == number => Some(*v),
            _ => None,
        })
        .collect()
}

fn field_text(fields: &[ProtoField<'_>], number: u32) -> Option<String> {
    fields
        .iter()
        .rev()
        .find_map(|field| match field {
            ProtoField {
                number: n,
                value: ProtoValue::Bytes(v),
            } if *n == number => Some(*v),
            _ => None,
        })
        .and_then(|v| std::str::from_utf8(v).ok())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

// --- Model normalization & placeholder resolution ---

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ModelResolution {
    Default,
    Placeholder,
    Resolved,
}

fn model_resolution(model: &str) -> ModelResolution {
    if model == DEFAULT_MODEL {
        ModelResolution::Default
    } else if model.starts_with(UNRESOLVED_PLACEHOLDER_PREFIX)
        || model.starts_with(UNRESOLVED_ID_PREFIX)
    {
        ModelResolution::Placeholder
    } else {
        ModelResolution::Resolved
    }
}

fn is_resolved_model(model: &str) -> bool {
    model_resolution(model) == ModelResolution::Resolved
}

fn model_name_from_id(model_id: u64) -> String {
    match model_id {
        246 => "gemini-2.5-pro".to_string(),
        312 => "gemini-2.5-flash".to_string(),
        313 | 329 => "gemini-2.5-flash-thinking".to_string(),
        330 => "gemini-2.5-flash-lite".to_string(),
        281 | 282 => "claude-sonnet-4".to_string(),
        290 | 291 => "claude-opus-4".to_string(),
        333 | 334 => "claude-4.5-sonnet".to_string(),
        340 | 341 => "claude-4.5-haiku".to_string(),
        342 => "model_openai_gpt_oss_120b_medium".to_string(),
        1_000.. => format!("{UNRESOLVED_PLACEHOLDER_PREFIX}{}", model_id - 1_000),
        _ => format!("{UNRESOLVED_ID_PREFIX}{model_id}"),
    }
}

fn model_from_id(model_id: u64) -> Option<String> {
    normalize_antigravity_model(&model_name_from_id(model_id))
}

fn row_model(model: Option<&str>, model_id: Option<u64>) -> Option<String> {
    model
        .filter(|m| !m.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| model_id.and_then(model_from_id))
}

const GEMINI_FLASH_EFFORT_FAMILIES: [&str; 3] =
    ["gemini-3.8-flash", "gemini-3.7-flash", "gemini-3.6-flash"];
const GEMINI_FLASH_EFFORTS: [&str; 3] = ["high", "medium", "low"];

fn is_gemini_flash_effort_variant(model: &str) -> bool {
    let Some((family, effort)) = model.rsplit_once('-') else {
        return false;
    };
    GEMINI_FLASH_EFFORTS.contains(&effort) && GEMINI_FLASH_EFFORT_FAMILIES.contains(&family)
}

fn gemini_flash_effort_variant(display_name: &str) -> Option<String> {
    let (base, rest) = display_name.split_once(" (")?;
    let effort = rest.strip_suffix(')')?;
    if base.contains('-') {
        return None;
    }
    let variant = format!("{}-{effort}", base.replace(' ', "-"));
    is_gemini_flash_effort_variant(&variant).then_some(variant)
}

fn normalize_antigravity_model(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(model) = gemini_flash_effort_variant(&lower) {
        return Some(model);
    }
    let base = lower
        .find('(')
        .map_or(lower.as_str(), |index| lower[..index].trim());
    let normalized = match base {
        "gemini 3.8 flash" | "gemini 3.8 flash thinking" => "gemini-3.8-flash",
        "gemini 3.7 flash" | "gemini 3.7 flash thinking" => "gemini-3.7-flash",
        "gemini 3.7 pro" | "gemini 3.7 pro thinking" => "gemini-3.7-pro",
        "gemini 3.6 flash" | "gemini 3 flash" => "gemini-3.6-flash",
        "gemini 3.6 pro" => "gemini-3.6-pro",
        "gemini 3 pro" | "gemini 3 pro thinking" => "gemini-3-pro",
        "gemini 2.5 flash" => "gemini-2.5-flash",
        "gemini 2.5 pro" => "gemini-2.5-pro",
        "gemini 2.0 flash" | "gemini 2 flash" => "gemini-2.0-flash",
        "gemini 2.0 pro" => "gemini-2.0-pro",
        "gemini 1.5 flash" => "gemini-1.5-flash",
        "gemini 1.5 pro" => "gemini-1.5-pro",
        "model_placeholder_m318" => "gemini-3.8-flash-high",
        "model_placeholder_m319" => "gemini-3.8-flash-medium",
        "model_placeholder_m320" => "gemini-3.8-flash-low",
        "model_placeholder_m298" => "gemini-3.7-flash-high",
        "model_placeholder_m299" => "gemini-3.7-flash-medium",
        "model_placeholder_m300" => "gemini-3.7-flash-low",
        "model_placeholder_m71" => "gemini-3.6-flash-high",
        "model_placeholder_m72" => "gemini-3.6-flash-medium",
        "model_placeholder_m73" => "gemini-3.6-flash-low",
        "model_placeholder_m417" | "model_placeholder_m418" | "model_placeholder_m419" => {
            "gemini-4-argon"
        }
        "model_placeholder_m402" => "claude-opus-5-5",
        "model_placeholder_m50" => "gemini-3.1-flash-lite",
        "model_placeholder_m26" => "claude-opus-4-6",
        "model_placeholder_m35" => "claude-sonnet-4-6",
        "model_placeholder_m36" | "model_placeholder_m37" | "model_placeholder_m16" => {
            "gemini-3.1-pro"
        }
        "model_placeholder_m18" | "model_placeholder_m84" | "model_placeholder_m47" => {
            "gemini-3-flash-preview"
        }
        "model_placeholder_m132" | "model_placeholder_m133" => "gemini-3.5-flash-high",
        "model_placeholder_m187" => "gemini-3.5-flash-extra-low",
        "model_placeholder_m20" => "gemini-3.5-flash-medium",
        "model_openai_gpt_oss_120b_medium" => "gpt-oss-120b-medium",
        "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
        "gemini-3-flash-agent"
        | "gemini-3-flash-agent-a"
        | "gemini-3-flash-agent-b"
        | "gemini-3-flash-a"
        | "gemini-3-flash-b" => "gemini-3.5-flash-high",
        "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
        "gemini-3.5-flash-low" => "gemini-3.5-flash-medium",
        "gemini-3.1-pro-high" | "gemini-3.1-pro-low" => "gemini-3.1-pro",
        "gemini-3-pro-high" | "gemini-3-pro-low" => "gemini-3-pro",
        "claude 3.7 sonnet" | "claude 3.7 sonnet thinking" => "claude-3-7-sonnet",
        "claude 3.5 sonnet" => "claude-3-5-sonnet",
        "claude 3.5 haiku" => "claude-3-5-haiku",
        "claude 3 opus" => "claude-3-opus",
        _ => {
            let converted = base.replace(' ', "-");
            if converted.starts_with("gemini-")
                || converted.starts_with("claude-")
                || converted.starts_with("gpt-")
            {
                return Some(converted);
            }
            return Some(trimmed.to_string());
        }
    };
    Some(normalized.to_string())
}
