//! Shared metadata-only record interface for local usage adapters.
use crate::{DailyUsage, ModelDayUsage, ModelUsage, Usage};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use chrono::{DateTime, Utc};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Default, Debug)]
pub struct ParseResult {
    pub records: Vec<TokenRecord>,
    pub warnings: Vec<String>,
}

pub struct UsageSummary {
    pub usage: Usage,
    pub history: Vec<DailyUsage>,
    pub model_usage: Vec<ModelUsage>,
    pub model_history: Vec<ModelDayUsage>,
    pub warnings: Vec<String>,
}

const MAX_SAFE: u64 = 9_007_199_254_740_991;
pub(crate) const RETAINED_USAGE_DAYS: u64 = 90;
pub(crate) const MAX_CODEX_RECORDS: usize = 16384;
fn add(left: u64, right: u64) -> Result<u64, String> {
    left.checked_add(right)
        .filter(|n| *n <= MAX_SAFE)
        .ok_or_else(|| "Token counts exceed the supported display range.".into())
}

pub fn aggregate(
    records: Vec<TokenRecord>,
    now: DateTime<Utc>,
    source: &str,
) -> Result<UsageSummary, String> {
    aggregate_window(records, now, source, 30)
}

pub fn aggregate_window(
    records: Vec<TokenRecord>,
    now: DateTime<Utc>,
    source: &str,
    days: u64,
) -> Result<UsageSummary, String> {
    let start_date = now
        .date_naive()
        .checked_sub_days(chrono::Days::new(days - 1))
        .ok_or("Invalid reporting period.")?;
    let start = start_date.and_hms_opt(0, 0, 0).unwrap().and_utc();
    let mut unique: BTreeMap<String, TokenRecord> = BTreeMap::new();
    for record in records {
        if record.timestamp < start || record.timestamp > now {
            continue;
        }
        if record.id.is_empty()
            || record.id.len() > 1024
            || record.model.is_empty()
            || record.model.len() > 512
            || record.model.chars().any(char::is_control)
        {
            return Err("Unsupported usage metadata.".into());
        }
        let replace = unique.get(&record.id).is_none_or(|old| {
            record.timestamp > old.timestamp
                || (record.timestamp == old.timestamp && record.total_tokens >= old.total_tokens)
        });
        if replace {
            unique.insert(record.id.clone(), record);
        }
    }
    if unique.is_empty() {
        return Err(format!("No supported token records in the last {days} UTC calendar days. Missing records are not zero usage."));
    }
    let mut days: BTreeMap<String, DailyUsage> = BTreeMap::new();
    let mut models: BTreeMap<String, u64> = BTreeMap::new();
    let mut model_days: BTreeMap<(String, String), (u64, u64, u64, u64, u64, u64)> =
        BTreeMap::new();
    let mut usage = Usage {
        input_tokens: 0,
        output_tokens: 0,
        cached_tokens: 0,
        cache_creation_tokens: Some(0),
        cache_read_tokens: Some(0),
        total_tokens: 0,
        spend_usd: None,
        source: source.into(),
        scope: "device".into(),
        as_of: now.to_rfc3339(),
        period_start: Some(start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        period_end: Some(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    };
    for record in unique.into_values() {
        let cache_creation = record.cache_creation_tokens;
        let cache_read = record.cache_read_tokens;
        let cached = if cache_creation > 0 || cache_read > 0 {
            add(cache_creation, cache_read)?
        } else {
            record.cached_tokens
        };
        let (eff_creation, eff_read) = if cache_creation > 0 || cache_read > 0 {
            (cache_creation, cache_read)
        } else {
            (0, cached)
        };
        usage.input_tokens = add(usage.input_tokens, record.input_tokens)?;
        usage.output_tokens = add(usage.output_tokens, record.output_tokens)?;
        usage.cached_tokens = add(usage.cached_tokens, cached)?;
        usage.cache_creation_tokens = Some(add(
            usage.cache_creation_tokens.unwrap_or(0),
            eff_creation,
        )?);
        usage.cache_read_tokens = Some(add(usage.cache_read_tokens.unwrap_or(0), eff_read)?);
        usage.total_tokens = add(usage.total_tokens, record.total_tokens)?;
        let date = record.timestamp.date_naive().to_string();
        let (total, input, output, m_cached, m_creation, m_read) = model_days
            .entry((date.clone(), record.model.clone()))
            .or_default();
        *total = add(*total, record.total_tokens)?;
        *input = add(*input, record.input_tokens)?;
        *output = add(*output, record.output_tokens)?;
        *m_cached = add(*m_cached, cached)?;
        *m_creation = add(*m_creation, eff_creation)?;
        *m_read = add(*m_read, eff_read)?;
        let day = days.entry(date.clone()).or_insert(DailyUsage {
            date,
            input_tokens: 0,
            output_tokens: 0,
            cached_tokens: 0,
            cache_creation_tokens: Some(0),
            cache_read_tokens: Some(0),
            total_tokens: 0,
            recorded_responses: Some(0),
        });
        day.input_tokens = add(day.input_tokens, record.input_tokens)?;
        day.output_tokens = add(day.output_tokens, record.output_tokens)?;
        day.cached_tokens = add(day.cached_tokens, cached)?;
        day.cache_creation_tokens =
            Some(add(day.cache_creation_tokens.unwrap_or(0), eff_creation)?);
        day.cache_read_tokens = Some(add(day.cache_read_tokens.unwrap_or(0), eff_read)?);
        day.total_tokens = add(day.total_tokens, record.total_tokens)?;
        day.recorded_responses = Some(add(day.recorded_responses.unwrap_or(0), 1)?);
        let count = models.entry(record.model).or_default();
        *count = add(*count, record.total_tokens)?;
    }
    Ok(UsageSummary {
        usage,
        history: days.into_values().collect(),
        model_usage: models
            .into_iter()
            .map(|(model, total_tokens)| ModelUsage {
                model,
                total_tokens,
            })
            .collect(),
        model_history: model_days
            .into_iter()
            .map(
                |((date, model), (total_tokens, input, output, cached, creation, read))| {
                    ModelDayUsage {
                        date,
                        model,
                        total_tokens,
                        input_tokens: Some(input),
                        output_tokens: Some(output),
                        cached_tokens: Some(cached),
                        cache_creation_tokens: Some(creation),
                        cache_read_tokens: Some(read),
                    }
                },
            )
            .collect(),
        warnings: vec![],
    })
}

pub struct SessionFile {
    pub hint: String,
    pub path_hint: String,
    pub text: String,
}
pub struct SessionFiles {
    pub files: Vec<SessionFile>,
    pub warnings: Vec<String>,
}
const MAX_FILE: u64 = 16 * 1024 * 1024;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;

/// Parse a comma-separated environment variable of absolute or `~`-prefixed directories.
pub(crate) fn extra_roots_from_env(var: &str, home: &Path) -> Vec<PathBuf> {
    let Ok(raw) = std::env::var(var) else {
        return vec![];
    };
    let mut out = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = if trimmed == "~" {
            home.to_path_buf()
        } else if let Some(rest) = trimmed.strip_prefix("~/") {
            home.join(rest)
        } else {
            PathBuf::from(trimmed)
        };
        if path.is_absolute() && !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

/// Capability-relative filesystem access confines every read to the approved tree.
/// Directory and final-file symlinks are refused, including during path replacement.
/// Sources: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html
/// https://github.com/bytecodealliance/cap-std/tree/main/cap-fs-ext
pub fn read_session_files(home: &Path, provider: &str) -> Result<SessionFiles, String> {
    read_session_files_until(home, provider, &|| false)
}

pub(crate) fn read_session_files_until(
    home: &Path,
    provider: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<SessionFiles, String> {
    read_session_files_with_roots(home, &[], provider, cancelled)
}

pub(crate) fn read_session_files_with_roots(
    home: &Path,
    extra_roots: &[PathBuf],
    provider: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<SessionFiles, String> {
    if cancelled() {
        return Err("Local usage scan was cancelled.".into());
    }
    if !home.is_absolute() {
        return Err("Usage home directory must be absolute.".into());
    }
    let home_dir = Dir::open_ambient_dir(home, ambient_authority())
        .map_err(|_| "Usage home directory unavailable.")?;
    let mut root_dirs: Vec<(String, Dir)> = Vec::new();
    let mut warnings = vec![];
    if !extra_roots.is_empty() {
        for (idx, base) in extra_roots.iter().enumerate() {
            let candidate = if provider == "claude"
                && base.file_name().is_some_and(|n| n == "projects")
            {
                base.clone()
            } else if provider == "claude" {
                base.join("projects")
            } else {
                base.clone()
            };
            if let Ok(dir) = open_absolute_nofollow(&candidate) {
                root_dirs.push((format!("env{idx}"), dir));
            }
        }
    } else {
        let candidates: Vec<&[&str]> = match provider {
            "claude" => vec![&[".config", "claude", "projects"], &[".claude", "projects"]],
            "gemini" => vec![&[".gemini", "tmp"]],
            "codex" => vec![&[".codex"]],
            _ => return Err("Unsupported local usage integration.".into()),
        };
        for (idx, parts) in candidates.iter().enumerate() {
            if let Ok(dir) = open_relative_nofollow(&home_dir, parts) {
                root_dirs.push((format!("root{idx}"), dir));
            }
        }
    }
    if root_dirs.is_empty() {
        return Err(
            "Approved local session directory unavailable; linked directories are not followed."
                .into(),
        );
    }
    let mut files = vec![];
    let mut bytes = 0usize;
    let mut entries = 0usize;
    let mut codex_found = false;
    for (root_tag, root) in &root_dirs {
        let mut paths = BTreeSet::new();
        if provider == "codex" {
            for name in ["sessions", "archived_sessions"] {
                match root.open_dir_nofollow(name) {
                    Ok(subdir) => {
                        codex_found = true;
                        collect(
                            &subdir,
                            Path::new(name),
                            provider,
                            0,
                            &mut entries,
                            &mut paths,
                            &mut warnings,
                            cancelled,
                        )?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(_) => warnings
                        .push("A linked or unavailable Codex session directory was skipped.".into()),
                }
            }
        } else {
            collect(
                root,
                Path::new(""),
                provider,
                0,
                &mut entries,
                &mut paths,
                &mut warnings,
                cancelled,
            )?;
        }
        let mut ordered: Vec<_> = paths.iter().collect();
        ordered.sort_by_cached_key(|path| {
            std::cmp::Reverse(root.symlink_metadata(path).and_then(|m| m.modified()).ok())
        });
        for path in ordered {
            if cancelled() {
                return Err("Local usage scan was cancelled.".into());
            }
            if provider == "gemini"
                && path.extension().is_some_and(|s| s == "json")
                && paths.contains(&path.with_extension("jsonl"))
            {
                continue;
            }
            let mut options = OpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            #[cfg(unix)]
            {
                use cap_std::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NONBLOCK);
            }
            let Ok(file) = open_session(root, path, &options) else {
                warnings.push("An unreadable session file was skipped.".into());
                continue;
            };
            let Ok(metadata) = file.metadata() else {
                warnings.push("Session metadata unavailable.".into());
                continue;
            };
            if !metadata.is_file()
                || metadata.len() > MAX_FILE
                || bytes.saturating_add(metadata.len() as usize) > MAX_BYTES
            {
                warnings.push("Session scan limits reached; observed usage is incomplete.".into());
                continue;
            }
            let mut text = String::new();
            if file.take(MAX_FILE + 1).read_to_string(&mut text).is_err()
                || text.len() as u64 > MAX_FILE
                || bytes.saturating_add(text.len()) > MAX_BYTES
            {
                warnings.push("An oversized or unsupported session file was skipped.".into());
                continue;
            }
            bytes += text.len();
            // No source path or conversation text leaves the local parser boundary.
            let hint = if provider == "codex" {
                String::new()
            } else {
                session_hint(&text, path)
            };
            let path_hint = format!("{root_tag}/{}", path.to_string_lossy());
            files.push(SessionFile {
                hint,
                path_hint,
                text,
            });
        }
    }
    if provider == "codex" && !codex_found {
        return Err("No approved Codex rollout directories are available.".into());
    }
    warnings.sort();
    warnings.dedup();
    Ok(SessionFiles { files, warnings })
}

fn open_relative_nofollow(base: &Dir, parts: &[&str]) -> std::io::Result<Dir> {
    let mut cur = base.try_clone()?;
    for part in parts {
        cur = cur.open_dir_nofollow(part)?;
    }
    Ok(cur)
}

pub(crate) fn open_absolute_nofollow(path: &Path) -> std::io::Result<Dir> {
    if !path.is_absolute() {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let mut components = path.components();
    let mut base = PathBuf::new();
    for component in components.by_ref() {
        base.push(component.as_os_str());
        if component == std::path::Component::RootDir {
            break;
        }
    }
    let mut dir = Dir::open_ambient_dir(base, ambient_authority())?;
    for component in components {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        };
        dir = dir.open_dir_nofollow(name)?;
    }
    Ok(dir)
}

/// Open a local SQLite database strictly read-only, refusing symlinks on both
/// the database file and its `-wal`/`-shm`/`-journal` sidecars, and disabling
/// `trusted_schema` before any query executes.
pub(crate) fn open_sqlite_readonly_nofollow(path: &Path) -> Result<rusqlite::Connection, ()> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| ())?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Err(());
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        match std::fs::symlink_metadata(Path::new(&sidecar)) {
            Ok(m) if !m.file_type().is_symlink() && m.file_type().is_file() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(()),
        }
    }
    let flags =
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
    let conn = rusqlite::Connection::open_with_flags(path, flags).map_err(|_| ())?;
    conn.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|_| ())?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA trusted_schema = OFF;")
        .map_err(|_| ())?;
    Ok(conn)
}

fn open_session(
    root: &Dir,
    path: &Path,
    options: &OpenOptions,
) -> std::io::Result<cap_std::fs::File> {
    let mut parent = root.try_clone()?;
    for component in path.parent().unwrap_or(Path::new("")).components() {
        let std::path::Component::Normal(name) = component else {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        };
        parent = parent.open_dir_nofollow(name)?;
    }
    parent.open_with(
        path.file_name().ok_or(std::io::ErrorKind::InvalidInput)?,
        options,
    )
}

fn session_hint(text: &str, path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let metadata = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .or_else(|| serde_json::from_str(text.lines().next().unwrap_or("")).ok());
    let session = metadata
        .as_ref()
        .and_then(|v| v["sessionId"].as_str())
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control));
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    if let Some(session) = session {
        session.hash(&mut hash)
    } else {
        path.with_extension("").hash(&mut hash)
    }
    format!("session-{:016x}", hash.finish())
}

pub fn load_local_usage(
    home: &Path,
    provider: &str,
    now: DateTime<Utc>,
) -> Result<UsageSummary, String> {
    let files = read_session_files(home, provider)?;
    from_files(&files, provider, now)
}

pub(crate) fn from_files(
    files: &SessionFiles,
    provider: &str,
    now: DateTime<Utc>,
) -> Result<UsageSummary, String> {
    if provider == "codex" {
        return codex_records(files, now).map(|(summary, _)| summary);
    }
    let mut warnings = files.warnings.clone();
    let mut records = vec![];
    if provider == "claude" {
        let parsed = crate::claude_usage::parse_files(&files.files);
        records.extend(parsed.records);
        warnings.extend(parsed.warnings);
    } else if provider == "gemini" {
        for file in &files.files {
            match crate::gemini_usage::parse(&file.text, &file.hint) {
                Ok(parsed) => {
                    records.extend(parsed.records);
                    warnings.extend(parsed.warnings);
                }
                Err(_) => warnings.push(
                    "An unsupported session file was excluded; observed usage is incomplete."
                        .into(),
                ),
            }
        }
    } else {
        return Err("Unsupported usage integration.".into());
    }
    let source = if provider == "claude" {
        "Claude Code local transcripts (ccusage method)"
    } else {
        "Gemini CLI local transcripts (ccusage method)"
    };
    let mut result = aggregate_window(records, now, source, RETAINED_USAGE_DAYS)?;
    warnings.sort();
    warnings.dedup();
    result.warnings = warnings;
    Ok(result)
}

pub(crate) fn codex_records(
    files: &SessionFiles,
    now: DateTime<Utc>,
) -> Result<(UsageSummary, Vec<TokenRecord>), String> {
    let start = now.date_naive() - chrono::Days::new(RETAINED_USAGE_DAYS - 1);
    let mut unique = BTreeMap::<String, (DateTime<Utc>, Option<TokenRecord>)>::new();
    let mut warnings = files.warnings.clone();
    let (parsed, cumulative) = crate::codex_usage::parse_files(&files.files);
    warnings.extend(parsed.warnings);
    for row in parsed.records {
        if row.timestamp > now || row.timestamp.date_naive() < start {
            continue;
        }
        match unique.entry(row.id.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert((row.timestamp, Some(row)));
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let (timestamp, old) = slot.get_mut();
                if row.timestamp > *timestamp {
                    *timestamp = row.timestamp;
                    *old = Some(row);
                } else if row.timestamp == *timestamp && old.as_ref() != Some(&row) {
                    *old = None;
                }
            }
        }
        if unique.len() > MAX_CODEX_RECORDS {
            return Err(
                "Codex usage record limit reached; previous history was retained.".into(),
            );
        }
    }
    if unique.values().any(|(_, row)| row.is_none()) {
        warnings
            .push("Conflicting copies of a Codex response were excluded from this scan.".into());
    }
    let records: Vec<_> = unique.into_values().filter_map(|(_, row)| row).collect();
    if records.is_empty() {
        let reason = if cumulative {
            "Codex reported cumulative snapshots but no eligible per-response counters. Daily usage cannot be calculated from these snapshots."
        } else {
            "No supported Codex per-response counters were found in the local rollouts for the last 90 UTC days."
        };
        warnings.sort();
        warnings.dedup();
        warnings.insert(0, reason.into());
        return Err(warnings.join(" "));
    }
    let mut summary = aggregate_window(
        records.clone(),
        now,
        crate::codex_usage::SOURCE,
        RETAINED_USAGE_DAYS,
    )?;
    warnings.sort();
    warnings.dedup();
    summary.warnings = warnings;
    Ok((summary, records))
}

fn collect(
    root: &Dir,
    relative: &Path,
    provider: &str,
    depth: usize,
    visited: &mut usize,
    paths: &mut BTreeSet<PathBuf>,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    if depth > 6 || *visited >= MAX_ENTRIES {
        warnings.push("Session scan limits reached; observed usage is incomplete.".into());
        return Ok(());
    }
    let entries = root
        .entries()
        .map_err(|_| "Session directory listing unavailable.")?;
    for entry in entries {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        if *visited >= MAX_ENTRIES {
            warnings.push("Session scan limits reached; observed usage is incomplete.".into());
            break;
        }
        *visited += 1;
        let Ok(entry) = entry else {
            warnings.push("An unreadable session entry was skipped.".into());
            continue;
        };
        let name = entry.file_name();
        let path = relative.join(&name);
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            warnings.push("Linked session entries were skipped.".into());
            continue;
        }
        if kind.is_dir() {
            // Open each directory component without following a link; recursion uses its handle.
            let Ok(child) = root.open_dir_nofollow(&name) else {
                warnings.push("A linked or unavailable session directory was skipped.".into());
                continue;
            };
            collect(
                &child,
                &path,
                provider,
                depth + 1,
                visited,
                paths,
                warnings,
                cancelled,
            )?;
        } else if kind.is_file()
            && path
                .extension()
                .is_some_and(|e| e == "jsonl" || (provider == "gemini" && e == "json"))
            && match provider {
                "claude" | "codex" | "gemini" => true,
                _ => false,
            }
        {
            paths.insert(path);
        }
    }
    Ok(())
}
