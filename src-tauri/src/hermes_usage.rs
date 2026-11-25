//! Local read-only Hermes SQLite usage adapter (ccusage method).
use crate::usage::{
    aggregate_window, open_absolute_nofollow, TokenRecord, UsageSummary, RETAINED_USAGE_DAYS,
};
use chrono::{DateTime, Utc};
use rusqlite::{types::ValueRef, Connection};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

const SOURCE: &str = "Hermes local state database (ccusage method)";
const MAX_SAFE: u64 = 9_007_199_254_740_991;
const MAX_UNIX_SECS: f64 = 253_402_300_799.0; // 9999-12-31T23:59:59Z

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
    load_from_roots(&[home.join(".hermes")], now, cancelled)
}

pub fn load_from_roots(
    roots: &[PathBuf],
    now: DateTime<Utc>,
    cancelled: &dyn Fn() -> bool,
) -> Result<UsageSummary, String> {
    if cancelled() {
        return Err("Local usage scan was cancelled.".into());
    }
    let mut seen_dbs: HashSet<PathBuf> = HashSet::new();
    let mut seen_sessions: HashSet<String> = HashSet::new();
    let mut records: Vec<TokenRecord> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut accessible_roots = 0usize;

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
                warnings.push("Linked or unavailable Hermes directory was skipped.".into());
                continue;
            }
        };

        let meta = match root_dir.symlink_metadata("state.db") {
            Ok(meta) => meta,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(_) => {
                warnings.push("An unreadable Hermes state database was skipped.".into());
                continue;
            }
        };
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            warnings.push("Linked or unavailable Hermes state database was skipped.".into());
            continue;
        }

        let db_path = root.join("state.db");
        let canonical = std::fs::canonicalize(&db_path).unwrap_or_else(|_| db_path.clone());
        if !seen_dbs.insert(canonical) {
            continue;
        }

        load_state_db(
            &db_path,
            &mut seen_sessions,
            &mut records,
            &mut warnings,
            cancelled,
        )?;
    }

    if accessible_roots == 0 {
        return Err(
            "Approved local Hermes directory unavailable; linked directories are not followed."
                .into(),
        );
    }

    let mut summary = aggregate_window(records, now, SOURCE, RETAINED_USAGE_DAYS)?;
    warnings.sort();
    warnings.dedup();
    summary.warnings = warnings;
    Ok(summary)
}

fn open_sqlite_readonly(path: &Path) -> Result<Connection, ()> {
    crate::usage::open_sqlite_readonly_nofollow(path)
}

fn load_state_db(
    db_path: &Path,
    seen_sessions: &mut HashSet<String>,
    records: &mut Vec<TokenRecord>,
    warnings: &mut Vec<String>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    let Ok(conn) = open_sqlite_readonly(db_path) else {
        warnings.push("An unreadable Hermes state database was skipped.".into());
        return Ok(());
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT
            id,
            model,
            billing_provider,
            started_at,
            message_count,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens
        FROM sessions
        WHERE model IS NOT NULL
            AND TRIM(model) != ''",
    ) else {
        warnings.push("An unreadable Hermes state database was skipped.".into());
        return Ok(());
    };
    let Ok(mut rows) = stmt.query([]) else {
        warnings.push("An unreadable Hermes state database was skipped.".into());
        return Ok(());
    };

    while let Ok(Some(row)) = rows.next() {
        if cancelled() {
            return Err("Local usage scan was cancelled.".into());
        }
        let input_tokens = read_count(row, 5);
        let output_tokens = read_count(row, 6);
        let cache_read_tokens = read_count(row, 7);
        let cache_creation_tokens = read_count(row, 8);
        // reasoning_tokens (col 9) is a subset of output_tokens in Hermes — never add it again.
        let Some(cached_tokens) = cache_creation_tokens.checked_add(cache_read_tokens) else {
            warnings.push("A Hermes usage row exceeded supported token limits.".into());
            continue;
        };
        let Some(total_tokens) = input_tokens
            .checked_add(output_tokens)
            .and_then(|n| n.checked_add(cached_tokens))
            .filter(|n| *n <= MAX_SAFE)
        else {
            warnings.push("A Hermes usage row exceeded supported token limits.".into());
            continue;
        };
        if total_tokens == 0 {
            continue;
        }

        let Ok(session_id) = row.get::<_, String>(0) else {
            warnings.push("A Hermes usage row with invalid metadata was skipped.".into());
            continue;
        };
        let Ok(raw_model) = row.get::<_, String>(1) else {
            warnings.push("A Hermes usage row with invalid metadata was skipped.".into());
            continue;
        };
        let model = raw_model.trim().to_string();
        if session_id.is_empty()
            || model.is_empty()
            || model.len() > 512
            || model.chars().any(char::is_control)
        {
            warnings.push("A Hermes usage row with invalid metadata was skipped.".into());
            continue;
        }

        let Some(timestamp) = read_f64(row, 3).and_then(timestamp_from_secs) else {
            warnings.push("A Hermes usage row with an invalid timestamp was skipped.".into());
            continue;
        };

        // First-wins deduplication across roots.
        if !seen_sessions.insert(session_id.clone()) {
            continue;
        }

        records.push(TokenRecord {
            id: bounded_id("hermes", &session_id),
            timestamp,
            model,
            input_tokens,
            output_tokens,
            cached_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            total_tokens,
        });
    }
    Ok(())
}

fn read_count(row: &rusqlite::Row<'_>, idx: usize) -> u64 {
    match row.get_ref(idx) {
        Ok(ValueRef::Integer(n)) if n >= 0 => n as u64,
        Ok(ValueRef::Real(f)) if f.is_finite() && f >= 0.0 && f <= MAX_SAFE as f64 => f as u64,
        _ => 0,
    }
}

fn read_f64(row: &rusqlite::Row<'_>, idx: usize) -> Option<f64> {
    match row.get_ref(idx).ok()? {
        ValueRef::Real(f) if f.is_finite() => Some(f),
        ValueRef::Integer(n) => Some(n as f64),
        _ => None,
    }
}

fn timestamp_from_secs(secs: f64) -> Option<DateTime<Utc>> {
    if !secs.is_finite() || secs <= 0.0 || secs > MAX_UNIX_SECS {
        return None;
    }
    let dur = Duration::try_from_secs_f64(secs).ok()?;
    let millis = i64::try_from(dur.as_millis()).ok()?;
    DateTime::from_timestamp_millis(millis)
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
