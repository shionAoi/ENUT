//! Local read-only OpenCode SQLite and legacy JSON usage adapter (ccusage method).
use crate::usage::{
    aggregate_window, open_absolute_nofollow, TokenRecord, UsageSummary, RETAINED_USAGE_DAYS,
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use chrono::{DateTime, Utc};
use rusqlite::{types::ValueRef, Connection};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

const SOURCE: &str = "OpenCode local database (ccusage method)";
const MAX_SAFE: u64 = 9_007_199_254_740_991;
const MAX_JSON_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_JSON_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_DEPTH: usize = 4;

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
    load_from_roots(&[home.join(".local/share/opencode")], now, cancelled)
}

pub fn load_from_roots(
    roots: &[PathBuf],
    now: DateTime<Utc>,
    cancelled: &dyn Fn() -> bool,
) -> Result<UsageSummary, String> {
    if cancelled() {
        return Err("Local usage scan was cancelled.".into());
    }
    let mut claimed_ids: HashSet<String> = HashSet::new();
    let mut records: Vec<TokenRecord> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut accessible_roots = 0usize;
    let mut total_json_bytes = 0usize;
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
                warnings.push("Linked or unavailable OpenCode directory was skipped.".into());
                continue;
            }
        };

        let db_paths = discover_databases(root, &root_dir, &mut visited_entries, &mut warnings);
        for db_path in db_paths {
            if cancelled() {
                return Err("Local usage scan was cancelled.".into());
            }
            load_database(
                &db_path,
                &mut claimed_ids,
                &mut records,
                &mut warnings,
                cancelled,
            )?;
        }

        load_legacy_json(
            &root_dir,
            &mut claimed_ids,
            &mut records,
            &mut warnings,
            &mut visited_entries,
            &mut total_json_bytes,
            cancelled,
        )?;
    }

    if accessible_roots == 0 {
        return Err(
            "Approved local OpenCode data directory unavailable; linked directories are not followed."
                .into(),
        );
    }

    let mut summary = aggregate_window(records, now, SOURCE, RETAINED_USAGE_DAYS)?;
    warnings.sort();
    warnings.dedup();
    summary.warnings = warnings;
    Ok(summary)
}

fn is_channel_db_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("opencode-") else {
        return false;
    };
    let Some(channel) = rest.strip_suffix(".db") else {
        return false;
    };
    !channel.is_empty()
        && channel
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn discover_databases(
    root: &Path,
    root_dir: &Dir,
    visited: &mut usize,
    warnings: &mut Vec<String>,
) -> Vec<PathBuf> {
    let Ok(entries) = root_dir.entries() else {
        warnings.push("OpenCode directory listing unavailable.".into());
        return Vec::new();
    };
    let mut primary = None;
    let mut channels = Vec::new();
    for entry in entries {
        if *visited >= MAX_ENTRIES {
            warnings.push("OpenCode scan limits reached; observed usage is incomplete.".into());
            break;
        }
        *visited += 1;
        let Ok(entry) = entry else {
            continue;
        };
        let os_name = entry.file_name();
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if name != "opencode.db" && !is_channel_db_name(name) {
            continue;
        }
        let Ok(meta) = root_dir.symlink_metadata(&os_name) else {
            warnings.push("An unreadable OpenCode database was skipped.".into());
            continue;
        };
        if meta.file_type().is_symlink() {
            warnings.push("Linked OpenCode database was skipped.".into());
            continue;
        }
        if !meta.file_type().is_file() {
            continue;
        }
        let full = root.join(name);
        if name == "opencode.db" {
            primary = Some(full);
        } else {
            channels.push(full);
        }
    }
    channels.sort();
    let mut out = Vec::new();
    if let Some(p) = primary {
        out.push(p);
    }
    out.extend(channels);
    out
}

fn open_sqlite_readonly(path: &Path) -> Result<Connection, ()> {
    crate::usage::open_sqlite_readonly_nofollow(path)
}

fn list_tables(conn: &Connection) -> Result<HashSet<String>, ()> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|_| ())?;
    let mut rows = stmt.query([]).map_err(|_| ())?;
    let mut tables = HashSet::new();
    while let Some(row) = rows.next().map_err(|_| ())? {
        if let Ok(name) = row.get::<_, String>(0) {
            tables.insert(name);
        }
    }
    Ok(tables)
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>, ()> {
    let sql = match table {
        "message" => "SELECT * FROM message LIMIT 0",
        "session_message" => "SELECT * FROM session_message LIMIT 0",
        "session_v2" => "SELECT * FROM session_v2 LIMIT 0",
        _ => return Err(()),
    };
    let stmt = conn.prepare(sql).map_err(|_| ())?;
    Ok(stmt.column_names().into_iter().map(str::to_owned).collect())
}

fn load_database(
    db_path: &Path,
    claimed_ids: &mut HashSet<String>,
    records: &mut Vec<TokenRecord>,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let Ok(conn) = open_sqlite_readonly(db_path) else {
        warnings.push("An unreadable OpenCode database was skipped.".into());
        return Ok(());
    };
    let Ok(tables) = list_tables(&conn) else {
        warnings.push("An unreadable OpenCode database was skipped.".into());
        return Ok(());
    };

    if tables.contains("message") {
        load_v1_messages(&conn, claimed_ids, records, warnings, cancelled)?;
    }
    if tables.contains("session_message") {
        load_v2_session_messages(
            &conn,
            tables.contains("session_v2"),
            claimed_ids,
            records,
            warnings,
            cancelled,
        )?;
    }
    Ok(())
}

fn load_v1_messages(
    conn: &Connection,
    claimed_ids: &mut HashSet<String>,
    records: &mut Vec<TokenRecord>,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let Ok(cols) = table_columns(conn, "message") else {
        warnings.push("An unreadable OpenCode message table was skipped.".into());
        return Ok(());
    };
    if !cols.contains("id") || !cols.contains("session_id") || !cols.contains("data") {
        return Ok(());
    }
    let sql = if cols.contains("time_created") {
        "SELECT id, session_id, data, time_created FROM message"
    } else {
        "SELECT id, session_id, data, NULL FROM message"
    };
    let Ok(mut stmt) = conn.prepare(sql) else {
        warnings.push("An unreadable OpenCode message table was skipped.".into());
        return Ok(());
    };
    let Ok(mut rows) = stmt.query([]) else {
        warnings.push("An unreadable OpenCode message table was skipped.".into());
        return Ok(());
    };
    while let Ok(Some(row)) = rows.next() {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        let Ok(msg_id) = row.get::<_, String>(0) else {
            continue;
        };
        let Ok(data) = row.get::<_, String>(2) else {
            continue;
        };
        let fallback_created = read_optional_i64(row, 3);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        let effective_id = if msg_id.is_empty() {
            value
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            msg_id
        };
        if effective_id.is_empty() || claimed_ids.contains(&effective_id) {
            continue;
        }
        if let Some(record) = parse_payload(&value, &effective_id, fallback_created) {
            claimed_ids.insert(effective_id);
            records.push(record);
        }
    }
    Ok(())
}

fn load_v2_session_messages(
    conn: &Connection,
    has_session_v2: bool,
    claimed_ids: &mut HashSet<String>,
    records: &mut Vec<TokenRecord>,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let Ok(sm_cols) = table_columns(conn, "session_message") else {
        warnings.push("An unreadable OpenCode session_message table was skipped.".into());
        return Ok(());
    };
    if !sm_cols.contains("id")
        || !sm_cols.contains("session_id")
        || !sm_cols.contains("type")
        || !sm_cols.contains("data")
    {
        return Ok(());
    }

    let fork_cutoffs = if has_session_v2 && sm_cols.contains("seq") {
        load_fork_cutoffs(conn)
    } else {
        HashMap::new()
    };

    let created_col = if sm_cols.contains("time_created") {
        "time_created"
    } else {
        "NULL"
    };
    let seq_col = if sm_cols.contains("seq") {
        "seq"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT id, session_id, type, data, {created_col}, {seq_col} \
         FROM session_message WHERE type IN ('assistant', 'compaction')"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        warnings.push("An unreadable OpenCode session_message table was skipped.".into());
        return Ok(());
    };
    let Ok(mut rows) = stmt.query([]) else {
        warnings.push("An unreadable OpenCode session_message table was skipped.".into());
        return Ok(());
    };

    while let Ok(Some(row)) = rows.next() {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        let Ok(msg_id) = row.get::<_, String>(0) else {
            continue;
        };
        let Ok(session_id) = row.get::<_, String>(1) else {
            continue;
        };
        let Ok(row_type) = row.get::<_, String>(2) else {
            continue;
        };
        let seq = read_optional_i64(row, 5);
        if fork_cutoffs
            .get(&session_id)
            .is_some_and(|cutoff| seq.is_some_and(|s| s <= *cutoff))
        {
            continue;
        }
        if msg_id.is_empty() || claimed_ids.contains(&msg_id) {
            continue;
        }
        let Ok(data) = row.get::<_, String>(3) else {
            continue;
        };
        let fallback_created = read_optional_i64(row, 4);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if row_type == "compaction"
            && value.get("status").and_then(|v| v.as_str()) != Some("completed")
        {
            continue;
        }
        if let Some(record) = parse_payload(&value, &msg_id, fallback_created) {
            claimed_ids.insert(msg_id);
            records.push(record);
        }
    }
    Ok(())
}

fn load_fork_cutoffs(conn: &Connection) -> HashMap<String, i64> {
    let mut cutoffs = HashMap::new();
    let Ok(cols) = table_columns(conn, "session_v2") else {
        return cutoffs;
    };
    if !cols.contains("id")
        || !cols.contains("fork_session_id")
        || !cols.contains("fork_boundary")
    {
        return cutoffs;
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, fork_session_id, fork_boundary FROM session_v2 \
         WHERE fork_session_id IS NOT NULL AND fork_boundary IS NOT NULL",
    ) else {
        return cutoffs;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return cutoffs;
    };
    while let Ok(Some(row)) = rows.next() {
        let Ok(fork_id) = row.get::<_, String>(0) else {
            continue;
        };
        let Ok(parent_id) = row.get::<_, String>(1) else {
            continue;
        };
        let Ok(boundary_raw) = row.get::<_, String>(2) else {
            continue;
        };
        if fork_id.is_empty() || parent_id.is_empty() {
            continue;
        }
        if let Some(cutoff) = resolve_fork_cutoff(conn, &parent_id, &boundary_raw) {
            cutoffs.insert(fork_id, cutoff);
        }
    }
    cutoffs
}

fn resolve_fork_cutoff(conn: &Connection, parent_id: &str, boundary_raw: &str) -> Option<i64> {
    let boundary: serde_json::Value = serde_json::from_str(boundary_raw).ok()?;
    let boundary_type = boundary.get("type")?.as_str()?;
    let parent_msg_id = boundary.get("messageID")?.as_str()?;
    let boundary_seq: Option<i64> = conn
        .query_row(
            "SELECT seq FROM session_message WHERE session_id = ?1 AND id = ?2 LIMIT 1",
            rusqlite::params![parent_id, parent_msg_id],
            |row| row.get(0),
        )
        .ok()
        .flatten();
    let boundary_seq = boundary_seq?;
    match boundary_type {
        "through" => Some(boundary_seq),
        "before" => conn
            .query_row(
                "SELECT MAX(seq) FROM session_message WHERE session_id = ?1 AND seq < ?2",
                rusqlite::params![parent_id, boundary_seq],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten(),
        _ => None,
    }
}

fn load_legacy_json(
    root_dir: &Dir,
    claimed_ids: &mut HashSet<String>,
    records: &mut Vec<TokenRecord>,
    warnings: &mut Vec<String>,
    visited: &mut usize,
    total_bytes: &mut usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let storage = match root_dir.open_dir_nofollow("storage") {
        Ok(dir) => dir,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            warnings.push("Linked or unavailable OpenCode legacy directory was skipped.".into());
            return Ok(());
        }
    };
    let message_dir = match storage.open_dir_nofollow("message") {
        Ok(dir) => dir,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            warnings.push("Linked or unavailable OpenCode legacy directory was skipped.".into());
            return Ok(());
        }
    };
    walk_legacy_json(
        &message_dir,
        0,
        claimed_ids,
        records,
        warnings,
        visited,
        total_bytes,
        cancelled,
    )
}

#[allow(clippy::too_many_arguments)]
fn walk_legacy_json(
    dir: &Dir,
    depth: usize,
    claimed_ids: &mut HashSet<String>,
    records: &mut Vec<TokenRecord>,
    warnings: &mut Vec<String>,
    visited: &mut usize,
    total_bytes: &mut usize,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if depth > MAX_DEPTH || *visited >= MAX_ENTRIES {
        warnings.push("OpenCode scan limits reached; observed usage is incomplete.".into());
        return Ok(());
    }
    let Ok(entries) = dir.entries() else {
        return Ok(());
    };
    let mut names: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.file_name()))
        .collect();
    names.sort();

    for name in names {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        if *visited >= MAX_ENTRIES {
            warnings.push("OpenCode scan limits reached; observed usage is incomplete.".into());
            break;
        }
        *visited += 1;
        let Ok(meta) = dir.symlink_metadata(&name) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            warnings.push("Linked OpenCode legacy entry was skipped.".into());
            continue;
        }
        if meta.file_type().is_dir() {
            let Ok(child) = dir.open_dir_nofollow(&name) else {
                warnings.push("Linked or unavailable OpenCode legacy directory was skipped.".into());
                continue;
            };
            walk_legacy_json(
                &child,
                depth + 1,
                claimed_ids,
                records,
                warnings,
                visited,
                total_bytes,
                cancelled,
            )?;
            continue;
        }
        if !meta.file_type().is_file() {
            continue;
        }
        let path_name = Path::new(&name);
        if path_name.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path_name.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if claimed_ids.contains(stem) {
            continue;
        }
        let mut opts = OpenOptions::new();
        opts.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NONBLOCK);
        }
        let Ok(file) = dir.open_with(&name, &opts) else {
            warnings.push("An unreadable OpenCode legacy file was skipped.".into());
            continue;
        };
        let Ok(fmeta) = file.metadata() else {
            continue;
        };
        if !fmeta.is_file()
            || fmeta.len() > MAX_JSON_FILE_BYTES
            || total_bytes.saturating_add(fmeta.len() as usize) > MAX_JSON_TOTAL_BYTES
        {
            warnings.push("OpenCode legacy scan limits reached; observed usage is incomplete.".into());
            continue;
        }
        let mut text = String::new();
        if file
            .take(MAX_JSON_FILE_BYTES + 1)
            .read_to_string(&mut text)
            .is_err()
            || text.len() as u64 > MAX_JSON_FILE_BYTES
            || total_bytes.saturating_add(text.len()) > MAX_JSON_TOTAL_BYTES
        {
            warnings.push("An oversized or unsupported OpenCode legacy file was skipped.".into());
            continue;
        }
        *total_bytes += text.len();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let msg_id = value
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(stem)
            .to_string();
        if claimed_ids.contains(&msg_id) {
            continue;
        }
        if let Some(record) = parse_payload(&value, &msg_id, None) {
            claimed_ids.insert(msg_id);
            records.push(record);
        }
    }
    Ok(())
}

fn parse_payload(
    value: &serde_json::Value,
    msg_id: &str,
    fallback_created: Option<i64>,
) -> Option<TokenRecord> {
    let tokens = value.get("tokens")?.as_object()?;
    let input = tokens.get("input").and_then(json_u64).unwrap_or(0);
    let output_base = tokens.get("output").and_then(json_u64).unwrap_or(0);
    let reasoning = tokens.get("reasoning").and_then(json_u64).unwrap_or(0);
    let cache = tokens.get("cache").and_then(|v| v.as_object());
    let cache_creation = cache
        .and_then(|c| c.get("write"))
        .and_then(json_u64)
        .unwrap_or(0);
    let cache_read = cache
        .and_then(|c| c.get("read"))
        .and_then(json_u64)
        .unwrap_or(0);
    let reported_total = tokens.get("total").and_then(json_u64).unwrap_or(0);

    let output = output_base.checked_add(reasoning)?;
    let cached = cache_creation.checked_add(cache_read)?;
    let component_sum = input.checked_add(output)?.checked_add(cached)?;
    let total = reported_total.max(component_sum);
    if total == 0 || total > MAX_SAFE {
        return None;
    }

    let model_obj = value.get("model").and_then(|v| v.as_object());
    let raw_model = model_obj
        .and_then(|m| {
            non_empty_str(m.get("id")).or_else(|| non_empty_str(m.get("modelID")))
        })
        .or_else(|| non_empty_str(value.get("modelID")))?;
    let raw_provider = model_obj
        .and_then(|m| non_empty_str(m.get("providerID")))
        .or_else(|| non_empty_str(value.get("providerID")));

    let model = format_model(raw_provider, raw_model)?;

    let created_ms = value
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(json_i64)
        .filter(|ms| *ms > 0)
        .or_else(|| fallback_created.filter(|ms| *ms > 0))?;
    let timestamp = DateTime::from_timestamp_millis(created_ms)?;

    Some(TokenRecord {
        id: bounded_id("opencode", msg_id),
        timestamp,
        model,
        input_tokens: input,
        output_tokens: output,
        cached_tokens: cached,
        cache_creation_tokens: cache_creation,
        cache_read_tokens: cache_read,
        total_tokens: total,
    })
}

fn format_model(provider: Option<&str>, model: &str) -> Option<String> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    let combined = match provider.map(str::trim).filter(|p| !p.is_empty()) {
        Some(provider) => {
            let colon_prefix = format!("{provider}:");
            let slash_prefix = format!("{provider}/");
            if model.starts_with(&colon_prefix) || model.starts_with(&slash_prefix) {
                model.to_string()
            } else {
                format!("{provider}:{model}")
            }
        }
        None => model.to_string(),
    };
    if combined.is_empty() || combined.len() > 512 || combined.chars().any(char::is_control) {
        return None;
    }
    Some(combined)
}

fn non_empty_str(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn json_u64(value: &serde_json::Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    if let Some(n) = value.as_i64() {
        return u64::try_from(n).ok();
    }
    value
        .as_f64()
        .filter(|f| f.is_finite() && *f >= 0.0 && *f <= MAX_SAFE as f64)
        .map(|f| f as u64)
}

fn json_i64(value: &serde_json::Value) -> Option<i64> {
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(n) = value.as_u64() {
        return i64::try_from(n).ok();
    }
    value
        .as_f64()
        .filter(|f| f.is_finite() && *f >= i64::MIN as f64 && *f <= i64::MAX as f64)
        .map(|f| f as i64)
}

fn read_optional_i64(row: &rusqlite::Row<'_>, idx: usize) -> Option<i64> {
    match row.get_ref(idx).ok()? {
        ValueRef::Integer(n) => Some(n),
        ValueRef::Real(f) if f.is_finite() => Some(f as i64),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse().ok(),
        _ => None,
    }
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
