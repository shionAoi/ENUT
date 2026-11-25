//! Monitor-owned normalized snapshots only; provider credentials remain CLI-owned.
use crate::usage::{aggregate_window, TokenRecord, MAX_CODEX_RECORDS, RETAINED_USAGE_DAYS};
use crate::{Agent, DailyUsage, ModelDayUsage, ModelUsage, PartialUsage, Usage};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use chrono::{NaiveDate, Utc};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path},
};

const LEGACY_FILE: &str = "monitor-state.json";
const DATABASE_FILE: &str = "monitor-usage.sqlite3";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SAFE: u64 = 9_007_199_254_740_991;
pub(crate) const STORAGE_ERROR: &str = "Local history storage unavailable. Saved permissions/history could not be confirmed; check local storage and retry. Revoke failures may leave saved data on disk.";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedTool {
    pub allowed: bool,
    usage: Option<Usage>,
    #[serde(skip)]
    period_verified: bool,
    history: Vec<DailyUsage>,
    models: Vec<ModelUsage>,
    model_history: Vec<ModelDayUsage>,
    partial: Option<PartialUsage>,
    updated_at: Option<String>,
    #[serde(skip)]
    codex_records: BTreeMap<String, TokenRecord>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedState {
    version: u8,
    pub tools: BTreeMap<String, SavedTool>,
}
impl Default for SavedState {
    fn default() -> Self {
        Self {
            version: 1,
            tools: BTreeMap::new(),
        }
    }
}
fn text(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
}
fn timestamp(s: &str) -> bool {
    s.len() <= 40 && chrono::DateTime::parse_from_rfc3339(s).is_ok()
}
fn date(s: &str) -> bool {
    s.len() == 10 && NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}
fn sum(mut values: impl Iterator<Item = u64>) -> Result<u64, String> {
    values.try_fold(0u64, |a, b| {
        a.checked_add(b)
            .filter(|n| *n <= MAX_SAFE)
            .ok_or(STORAGE_ERROR.into())
    })
}
impl SavedTool {
    pub fn apply(&self, agent: &mut Agent) {
        agent.usage = self.usage.clone();
        agent.saved_period_usage = (agent.id == "opencode" && self.period_verified)
            .then(|| self.usage.clone())
            .flatten();
        agent.usage_history = self.history.clone();
        agent.model_usage = self.models.clone();
        agent.model_history = self.model_history.clone();
        agent.partial_usage = self.partial.clone();
        agent.updated_at = self.updated_at.clone();
        agent.history_cached = self.usage.is_some()
            || self.partial.is_some()
            || !self.history.is_empty()
            || !self.model_history.is_empty();
    }
    pub fn merge(&mut self, agent: &Agent, codex_records: Vec<TokenRecord>) -> Result<(), String> {
        if agent.id == "codex" && !codex_records.is_empty() {
            // An incomplete reconciliation scan must not erase earlier observations.
            for record in codex_records {
                self.codex_records
                    .entry(record.id.clone())
                    .and_modify(|previous| {
                        let upgrade_model = record.timestamp == previous.timestamp
                            && previous.model == crate::codex_usage::MODEL
                            && record.model != crate::codex_usage::MODEL;
                        if record.timestamp > previous.timestamp || upgrade_model {
                            *previous = record.clone();
                        }
                    })
                    .or_insert(record);
            }
            let now = Utc::now();
            self.prune(now.date_naive())?;
            let summary = aggregate_window(
                self.codex_records.values().cloned().collect(),
                now,
                crate::codex_usage::SOURCE,
                RETAINED_USAGE_DAYS,
            )?;
            self.usage = Some(summary.usage);
            self.history = summary.history;
            self.models = summary.model_usage;
            self.model_history = summary.model_history;
            self.updated_at = agent.updated_at.clone();
            return self.validate();
        }
        if let Some(usage) = agent
            .reported_period_usage
            .as_ref()
            .or(agent.usage.as_ref())
        {
            self.usage = Some(usage.clone());
            self.period_verified = agent.reported_period_usage.is_some();
            self.models = agent.model_usage.clone();
        }
        if agent.partial_usage.is_some() {
            self.partial = agent.partial_usage.clone();
        }
        // Absolute buckets replace together; absent dates are unknown, not deletions.
        let dates: BTreeSet<_> = agent
            .usage_history
            .iter()
            .map(|row| &row.date)
            .chain(agent.model_history.iter().map(|row| &row.date))
            .collect();
        for date in &dates {
            self.history.retain(|old| old.date != **date);
            self.model_history.retain(|old| old.date != **date);
            self.history.extend(
                agent
                    .usage_history
                    .iter()
                    .filter(|row| row.date == **date)
                    .cloned(),
            );
            self.model_history.extend(
                agent
                    .model_history
                    .iter()
                    .filter(|row| row.date == **date)
                    .cloned(),
            );
        }
        if agent.usage.is_some() || agent.partial_usage.is_some() || !dates.is_empty() {
            self.updated_at = agent.updated_at.clone();
        }
        self.prune(Utc::now().date_naive())?;
        if !dates.is_empty() {
            if self
                .model_history
                .iter()
                .any(|row| !self.history.iter().any(|day| day.date == row.date))
            {
                // Model-only evidence cannot supply complete input/output daily totals.
                self.usage = None;
            }
            self.recount()?;
        }
        self.validate()
    }
    fn prune(&mut self, today: NaiveDate) -> Result<bool, String> {
        let start = today - chrono::Days::new(RETAINED_USAGE_DAYS - 1);
        let records_before = self.codex_records.len();
        self.codex_records.retain(|_, row| {
            let date = row.timestamp.date_naive();
            date >= start && date <= today
        });
        let previous = (self.history.len(), self.model_history.len());
        self.history.retain(|r| {
            NaiveDate::parse_from_str(&r.date, "%Y-%m-%d").is_ok_and(|d| d >= start && d <= today)
        });
        self.model_history.retain(|r| {
            NaiveDate::parse_from_str(&r.date, "%Y-%m-%d").is_ok_and(|d| d >= start && d <= today)
        });
        self.history.sort_by(|a, b| a.date.cmp(&b.date));
        self.model_history
            .sort_by(|a, b| (&a.date, &a.model).cmp(&(&b.date, &b.model)));
        let dated_expired = previous != (self.history.len(), self.model_history.len());
        if dated_expired {
            if previous.0 != 0 && self.history.is_empty() {
                self.usage = None;
            }
            self.recount()?;
        }
        let partial_expired = self.partial.as_ref().is_some_and(|p| {
            chrono::DateTime::parse_from_rfc3339(&p.period_end)
                .is_ok_and(|end| end.with_timezone(&Utc).date_naive() < start)
        });
        if partial_expired {
            self.partial = None;
        }
        Ok(dated_expired || partial_expired || self.codex_records.len() != records_before)
    }
    fn recount(&mut self) -> Result<(), String> {
        if let Some(usage) = &mut self.usage {
            usage.input_tokens = sum(self.history.iter().map(|r| r.input_tokens))?;
            usage.output_tokens = sum(self.history.iter().map(|r| r.output_tokens))?;
            usage.cached_tokens = sum(self.history.iter().map(|r| r.cached_tokens))?;
            usage.cache_creation_tokens = if !self.history.is_empty()
                && self.history.iter().all(|r| r.cache_creation_tokens.is_some())
            {
                Some(sum(
                    self.history.iter().filter_map(|r| r.cache_creation_tokens),
                )?)
            } else {
                None
            };
            usage.cache_read_tokens = if !self.history.is_empty()
                && self.history.iter().all(|r| r.cache_read_tokens.is_some())
            {
                Some(sum(
                    self.history.iter().filter_map(|r| r.cache_read_tokens),
                )?)
            } else {
                None
            };
            usage.total_tokens = sum(self.history.iter().map(|r| r.total_tokens))?;
            usage.period_start = self
                .history
                .first()
                .map(|r| format!("{}T00:00:00Z", r.date));
            usage.spend_usd = None;
        }
        let mut models: BTreeMap<String, u64> = BTreeMap::new();
        for row in &self.model_history {
            let n = models.entry(row.model.clone()).or_default();
            *n = sum([*n, row.total_tokens].into_iter())?;
        }
        self.models = models
            .into_iter()
            .map(|(model, total_tokens)| ModelUsage {
                model,
                total_tokens,
            })
            .collect();
        Ok(())
    }
    fn validate(&self) -> Result<(), String> {
        let mut days = BTreeSet::new();
        let mut models = BTreeSet::new();
        if self.history.len() > RETAINED_USAGE_DAYS as usize
            || self.codex_records.len() > MAX_CODEX_RECORDS
            || self.model_history.len() > 4096
            || self.models.len() > 256
            || self.updated_at.as_deref().is_some_and(|s| !timestamp(s))
        {
            return Err(STORAGE_ERROR.into());
        }
        for (id, r) in &self.codex_records {
            let valid_cache_split = r.cache_creation_tokens <= MAX_SAFE
                && r.cache_read_tokens <= MAX_SAFE
                && r.cache_creation_tokens.checked_add(r.cache_read_tokens)
                    == Some(r.cached_tokens);
            let valid_codex_totals = r
                .input_tokens
                .checked_add(r.output_tokens)
                .and_then(|v| v.checked_add(r.cached_tokens))
                .is_some_and(|s| s <= r.total_tokens)
                || (r.cached_tokens <= r.input_tokens
                    && r.input_tokens.checked_add(r.output_tokens) == Some(r.total_tokens));
            if id.len() != 64
                || !id.bytes().all(|b| b.is_ascii_hexdigit())
                || !text(&r.model)
                || r.total_tokens > MAX_SAFE
                || !valid_cache_split
                || !valid_codex_totals
            {
                return Err(STORAGE_ERROR.into());
            }
        }
        for r in &self.history {
            let valid_cache_split = match (r.cache_creation_tokens, r.cache_read_tokens) {
                (Some(cc), Some(cr)) => {
                    cc <= MAX_SAFE && cr <= MAX_SAFE && cc.checked_add(cr) == Some(r.cached_tokens)
                }
                (None, None) => true,
                _ => false,
            };
            let valid_totals = r
                .input_tokens
                .checked_add(r.output_tokens)
                .and_then(|v| v.checked_add(r.cached_tokens))
                .is_some_and(|s| s <= r.total_tokens)
                || (r.cached_tokens <= r.input_tokens
                    && r.input_tokens
                        .checked_add(r.output_tokens)
                        .is_some_and(|s| s <= r.total_tokens));
            if !date(&r.date)
                || !days.insert(&r.date)
                || !valid_cache_split
                || !valid_totals
                || [
                    r.input_tokens,
                    r.output_tokens,
                    r.cached_tokens,
                    r.total_tokens,
                ]
                .iter()
                .any(|n| *n > MAX_SAFE)
                || r.recorded_responses.is_some_and(|n| n > MAX_SAFE)
            {
                return Err(STORAGE_ERROR.into());
            }
        }
        for r in &self.model_history {
            if !text(&r.model)
                || !date(&r.date)
                || !models.insert((&r.date, &r.model))
                || r.total_tokens > MAX_SAFE
                || r.input_tokens.is_some_and(|n| n > MAX_SAFE)
                || r.output_tokens.is_some_and(|n| n > MAX_SAFE)
                || r.cached_tokens.is_some_and(|n| n > MAX_SAFE)
                || r.cache_creation_tokens.is_some_and(|n| n > MAX_SAFE)
                || r.cache_read_tokens.is_some_and(|n| n > MAX_SAFE)
            {
                return Err(STORAGE_ERROR.into());
            }
        }
        for day in &self.history {
            let total = sum(self
                .model_history
                .iter()
                .filter(|r| r.date == day.date)
                .map(|r| r.total_tokens))?;
            if self.model_history.iter().any(|r| r.date == day.date) && total != day.total_tokens {
                return Err(STORAGE_ERROR.into());
            }
        }
        if self
            .models
            .iter()
            .any(|r| !text(&r.model) || r.total_tokens > MAX_SAFE)
        {
            return Err(STORAGE_ERROR.into());
        }
        if let Some(u) = &self.usage {
            let valid_cache_split = match (u.cache_creation_tokens, u.cache_read_tokens) {
                (Some(cc), Some(cr)) => {
                    cc <= MAX_SAFE && cr <= MAX_SAFE && cc.checked_add(cr) == Some(u.cached_tokens)
                }
                (None, None) => true,
                _ => false,
            };
            if !text(&u.source)
                || !matches!(u.scope.as_str(), "device" | "account")
                || !timestamp(&u.as_of)
                || u.period_start.as_deref().is_some_and(|s| !timestamp(s))
                || u.period_end.as_deref().is_some_and(|s| !timestamp(s))
                || !valid_cache_split
                || [
                    u.input_tokens,
                    u.output_tokens,
                    u.cached_tokens,
                    u.total_tokens,
                ]
                .iter()
                .any(|n| *n > MAX_SAFE)
                || u.spend_usd.is_some()
            {
                return Err(STORAGE_ERROR.into());
            }
        }
        if let Some(p) = &self.partial {
            if !text(&p.source)
                || !text(&p.note)
                || !timestamp(&p.as_of)
                || !timestamp(&p.period_start)
                || !timestamp(&p.period_end)
                || p.input_tokens > MAX_SAFE
                || p.cached_tokens > MAX_SAFE
            {
                return Err(STORAGE_ERROR.into());
            }
        }
        Ok(())
    }
}
impl SavedState {
    pub fn prune(&mut self, today: NaiveDate) -> Result<BTreeSet<String>, String> {
        let mut changed = BTreeSet::new();
        for (id, tool) in &mut self.tools {
            if tool.prune(today)? {
                changed.insert(id.clone());
            }
        }
        Ok(changed)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.tools.len() > crate::service::AGENTS.len() {
            return Err(STORAGE_ERROR.into());
        }
        for (id, tool) in &self.tools {
            if !crate::service::known_agent(id)
                || (id != "codex" && !tool.codex_records.is_empty())
            {
                return Err(STORAGE_ERROR.into());
            }
            tool.validate()?;
        }
        Ok(())
    }
}
pub(crate) struct HistoryStore {
    dir: Dir,
    database: Connection,
    _lock: std::fs::File,
}
impl HistoryStore {
    pub fn load(path: &Path) -> Result<(Self, SavedState), String> {
        let (dir, lock) = Self::open_directory(path)?;
        let check = |name| match dir.symlink_metadata(name) {
            Ok(meta) => verify_local_file(&meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(STORAGE_ERROR.to_string()),
        };
        for name in [
            DATABASE_FILE,
            "monitor-usage.sqlite3-wal",
            "monitor-usage.sqlite3-shm",
            "monitor-usage.sqlite3-journal",
        ] {
            check(name)?;
        }
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match dir.open_with(DATABASE_FILE, &options) {
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(STORAGE_ERROR.into()),
        }
        let database = Connection::open_with_flags(
            path.join(DATABASE_FILE),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| STORAGE_ERROR)?;
        database
            .busy_timeout(std::time::Duration::from_millis(500))
            .map_err(|_| STORAGE_ERROR)?;
        database
            .execute_batch(
                "PRAGMA trusted_schema = OFF; PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;",
            )
            .map_err(|_| STORAGE_ERROR)?;
        let version: i64 = database
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|_| STORAGE_ERROR)?;
        let store = Self {
            dir,
            database,
            _lock: lock,
        };
        let has_legacy = store.has_local_legacy()?;
        const MIGRATE_V1_TO_V2: &str =
            "ALTER TABLE usage_daily ADD COLUMN cache_creation_tokens INTEGER;
             ALTER TABLE usage_daily ADD COLUMN cache_read_tokens INTEGER;
             ALTER TABLE usage_model_daily ADD COLUMN cached_tokens INTEGER;
             ALTER TABLE usage_model_daily ADD COLUMN cache_creation_tokens INTEGER;
             ALTER TABLE usage_model_daily ADD COLUMN cache_read_tokens INTEGER;
             ALTER TABLE usage_events ADD COLUMN model TEXT;
             ALTER TABLE usage_events ADD COLUMN cache_creation_tokens INTEGER;
             ALTER TABLE usage_events ADD COLUMN cache_read_tokens INTEGER;
             UPDATE usage_events
                SET input_tokens = input_tokens - cached_tokens,
                    cache_creation_tokens = 0,
                    cache_read_tokens = cached_tokens
              WHERE cached_tokens > 0
                AND cached_tokens <= input_tokens
                AND input_tokens + output_tokens = total_tokens;
             UPDATE usage_events
                SET cache_creation_tokens = 0,
                    cache_read_tokens = cached_tokens
              WHERE cache_creation_tokens IS NULL OR cache_read_tokens IS NULL;
             PRAGMA user_version = 2;";
        const MIGRATE_V2_TO_V3: &str =
            "INSERT OR IGNORE INTO usage_tools(tool, allowed, period_json, period_verified, partial_json, models_json, updated_at)
             SELECT 'codex', allowed, period_json, period_verified, partial_json, models_json, updated_at
               FROM usage_tools WHERE tool = 'codex-local';
             UPDATE usage_tools
                SET allowed = MAX(allowed, COALESCE((SELECT allowed FROM usage_tools WHERE tool = 'codex-local'), 0)),
                    period_json = COALESCE(period_json, (SELECT period_json FROM usage_tools WHERE tool = 'codex-local')),
                    period_verified = MAX(period_verified, COALESCE((SELECT period_verified FROM usage_tools WHERE tool = 'codex-local'), 0)),
                    partial_json = COALESCE(partial_json, (SELECT partial_json FROM usage_tools WHERE tool = 'codex-local')),
                    models_json = CASE WHEN models_json = '[]' THEN COALESCE((SELECT models_json FROM usage_tools WHERE tool = 'codex-local'), '[]') ELSE models_json END,
                    updated_at = COALESCE(updated_at, (SELECT updated_at FROM usage_tools WHERE tool = 'codex-local'))
              WHERE tool = 'codex'
                AND EXISTS (SELECT 1 FROM usage_tools WHERE tool = 'codex-local');
             INSERT OR IGNORE INTO usage_model_daily(tool, date, model, total_tokens, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens)
             SELECT 'codex', date, model, total_tokens, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens
               FROM usage_model_daily
              WHERE tool = 'codex-local'
                AND NOT EXISTS (SELECT 1 FROM usage_daily d WHERE d.tool = 'codex' AND d.date = usage_model_daily.date)
                AND NOT EXISTS (SELECT 1 FROM usage_model_daily m WHERE m.tool = 'codex' AND m.date = usage_model_daily.date);
             INSERT OR IGNORE INTO usage_daily(tool, date, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens, recorded_responses)
             SELECT 'codex', date, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens, recorded_responses
               FROM usage_daily WHERE tool = 'codex-local';
             INSERT OR IGNORE INTO usage_events(tool, id, timestamp_utc, model, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens)
             SELECT 'codex', id, timestamp_utc, model, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens
               FROM usage_events WHERE tool = 'codex-local';
             DELETE FROM usage_tools WHERE tool = 'codex-local';
             PRAGMA user_version = 3;";
        let mut state = match version {
            0 => {
                let tables: i64 = store
                    .database
                    .query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))
                    .map_err(|_| STORAGE_ERROR)?;
                if tables != 0 {
                    return Err(STORAGE_ERROR.into());
                }
                store.read_legacy()?
            }
            1 | 2 => {
                let tx = store
                    .database
                    .unchecked_transaction()
                    .map_err(|_| STORAGE_ERROR)?;
                if has_legacy {
                    Self::verify_foreign_keys(&tx)?;
                }
                if version == 1 {
                    tx.execute_batch(MIGRATE_V1_TO_V2)
                        .map_err(|_| STORAGE_ERROR)?;
                }
                tx.execute_batch(MIGRATE_V2_TO_V3)
                    .map_err(|_| STORAGE_ERROR)?;
                let state = Self::read_database(&tx).map_err(|_| STORAGE_ERROR)?;
                state.validate()?;
                tx.commit().map_err(|_| STORAGE_ERROR)?;
                state
            }
            3 => {
                if has_legacy {
                    Self::verify_foreign_keys(&store.database)?;
                }
                Self::read_database(&store.database).map_err(|_| STORAGE_ERROR)?
            }
            _ => return Err(STORAGE_ERROR.into()),
        };
        state.validate()?;
        let mode: String = store
            .database
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(|_| STORAGE_ERROR)?;
        if mode != "wal" {
            return Err(STORAGE_ERROR.into());
        }
        store
            .database
            .execute_batch("PRAGMA synchronous = FULL;")
            .map_err(|_| STORAGE_ERROR)?;
        let pruned = state.prune(Utc::now().date_naive())?;
        if version == 0 {
            let tx = store
                .database
                .unchecked_transaction()
                .map_err(|_| STORAGE_ERROR)?;
            tx.execute_batch(include_str!("usage_schema.sql"))
                .map_err(|_| STORAGE_ERROR)?;
            Self::write_database(&tx, &state).map_err(|_| STORAGE_ERROR)?;
            tx.execute_batch("PRAGMA user_version = 3;")
                .map_err(|_| STORAGE_ERROR)?;
            tx.commit().map_err(|_| STORAGE_ERROR)?;
        } else if !pruned.is_empty() {
            store.save(&state)?;
        }
        // SQLite is authoritative now. A cleanup error leaves it intact for the next startup.
        if has_legacy {
            store.remove_legacy()?;
        }
        Ok((store, state))
    }

    fn verify_foreign_keys(database: &Connection) -> Result<(), String> {
        // Enabling foreign keys does not detect orphaned history already on disk.
        let mut check = database
            .prepare("PRAGMA foreign_key_check")
            .map_err(|_| STORAGE_ERROR)?;
        if check.exists([]).map_err(|_| STORAGE_ERROR)? {
            return Err(STORAGE_ERROR.into());
        }
        Ok(())
    }

    fn has_local_legacy(&self) -> Result<bool, String> {
        match self.dir.symlink_metadata(LEGACY_FILE) {
            Ok(metadata) => {
                verify_local_file(&metadata)?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(STORAGE_ERROR.into()),
        }
    }

    fn remove_legacy(&self) -> Result<(), String> {
        if self.has_local_legacy()? {
            self.dir
                .remove_file(LEGACY_FILE)
                .map_err(|_| STORAGE_ERROR)?;
        }
        Ok(())
    }

    fn read_legacy(&self) -> Result<SavedState, String> {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = match self.dir.open_with(LEGACY_FILE, &options) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SavedState::default()),
            Err(_) => return Err(STORAGE_ERROR.into()),
        };
        let metadata = file.metadata().map_err(|_| STORAGE_ERROR)?;
        verify_local_file(&metadata)?;
        if metadata.len() > MAX_BYTES {
            return Err(STORAGE_ERROR.into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| STORAGE_ERROR)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(STORAGE_ERROR.into());
        }
        let state: SavedState = serde_json::from_slice(&bytes).map_err(|_| STORAGE_ERROR)?;
        if state.tools.contains_key("codex-local") {
            return Err(STORAGE_ERROR.into());
        }
        for tool in state.tools.values() {
            for r in &tool.history {
                if r.cached_tokens > r.input_tokens {
                    return Err(STORAGE_ERROR.into());
                }
            }
        }
        Ok(state)
    }
    fn open_directory(path: &Path) -> Result<(Dir, std::fs::File), String> {
        if !path.is_absolute() {
            return Err(STORAGE_ERROR.into());
        }
        let mut components = path.components();
        let mut base = std::path::PathBuf::new();
        for component in components.by_ref() {
            base.push(component.as_os_str());
            if component == Component::RootDir {
                break;
            }
        }
        let mut dir =
            Dir::open_ambient_dir(base, ambient_authority()).map_err(|_| STORAGE_ERROR)?;
        for component in components {
            let Component::Normal(name) = component else {
                return Err(STORAGE_ERROR.into());
            };
            match dir.create_dir(name) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(_) => return Err(STORAGE_ERROR.into()),
            }
            dir = dir.open_dir_nofollow(name).map_err(|_| STORAGE_ERROR)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let handle = dir.open(".").map_err(|_| STORAGE_ERROR)?.into_std();
            if handle.metadata().map_err(|_| STORAGE_ERROR)?.uid() != unsafe { libc::geteuid() } {
                return Err(STORAGE_ERROR.into());
            }
            handle
                .set_permissions(std::fs::Permissions::from_mode(0o700))
                .map_err(|_| STORAGE_ERROR)?;
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        let lock = dir
            .open_with(".monitor-lock", &options)
            .map_err(|_| STORAGE_ERROR)?
            .into_std();
        if !lock.metadata().map_err(|_| STORAGE_ERROR)?.is_file() {
            return Err(STORAGE_ERROR.into());
        }
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
            let meta = lock.metadata().map_err(|_| STORAGE_ERROR)?;
            if meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o077 != 0
                || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
            {
                return Err(STORAGE_ERROR.into());
            }
        }
        Ok((dir, lock))
    }
    pub fn save(&self, state: &SavedState) -> Result<(), String> {
        state.validate()?;
        let meta = self
            .dir
            .symlink_metadata(DATABASE_FILE)
            .map_err(|_| STORAGE_ERROR)?;
        verify_local_file(&meta)?;
        let tx = self
            .database
            .unchecked_transaction()
            .map_err(|_| STORAGE_ERROR)?;
        Self::write_database(&tx, state).map_err(|_| STORAGE_ERROR)?;
        tx.commit().map_err(|_| STORAGE_ERROR.into())
    }

    fn read_database(conn: &Connection) -> rusqlite::Result<SavedState> {
        let mut state = SavedState::default();
        let mut tools = conn.prepare("SELECT tool, allowed, period_json, partial_json, models_json, updated_at, period_verified FROM usage_tools LIMIT ?1")?;
        let mut daily = conn.prepare("SELECT date, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens, recorded_responses FROM usage_daily WHERE tool=?1 ORDER BY date LIMIT 91")?;
        let mut models = conn.prepare("SELECT date, model, total_tokens, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens FROM usage_model_daily WHERE tool=?1 ORDER BY date, model LIMIT 4097")?;
        let mut events = conn.prepare("SELECT id, timestamp_utc, model, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens FROM usage_events WHERE tool=?1 LIMIT ?2")?;
        let mut rows = tools.query([crate::service::AGENTS.len() as i64 + 1])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let tool = SavedTool {
                allowed: row.get(1)?,
                usage: decode(row, 2)?,
                period_verified: row.get(6)?,
                partial: decode(row, 3)?,
                models: decode(row, 4)?,
                updated_at: row.get(5)?,
                codex_records: events
                    .query_map(params![id, MAX_CODEX_RECORDS as i64 + 1], |r| {
                        let id: String = r.get(0)?;
                        let time: String = r.get(1)?;
                        let timestamp = chrono::DateTime::parse_from_rfc3339(&time)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    1,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .with_timezone(&Utc);
                        let model: String = r
                            .get::<_, Option<String>>(2)?
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| crate::codex_usage::MODEL.into());
                        let cached_tokens = r.get::<_, i64>(5)? as u64;
                        let cache_creation_tokens =
                            r.get::<_, Option<i64>>(6)?.unwrap_or(0) as u64;
                        let cache_read_tokens = r
                            .get::<_, Option<i64>>(7)?
                            .map(|n| n as u64)
                            .unwrap_or_else(|| cached_tokens.saturating_sub(cache_creation_tokens));
                        Ok((
                            id.clone(),
                            TokenRecord {
                                id,
                                timestamp,
                                model,
                                input_tokens: r.get::<_, i64>(3)? as u64,
                                output_tokens: r.get::<_, i64>(4)? as u64,
                                cached_tokens,
                                cache_creation_tokens,
                                cache_read_tokens,
                                total_tokens: r.get::<_, i64>(8)? as u64,
                            },
                        ))
                    })?
                    .collect::<Result<_, _>>()?,
                history: daily
                    .query_map([&id], |r| {
                        Ok(DailyUsage {
                            date: r.get(0)?,
                            input_tokens: r.get::<_, i64>(1)? as u64,
                            output_tokens: r.get::<_, i64>(2)? as u64,
                            cached_tokens: r.get::<_, i64>(3)? as u64,
                            cache_creation_tokens: r.get::<_, Option<i64>>(4)?.map(|n| n as u64),
                            cache_read_tokens: r.get::<_, Option<i64>>(5)?.map(|n| n as u64),
                            total_tokens: r.get::<_, i64>(6)? as u64,
                            recorded_responses: r.get::<_, Option<i64>>(7)?.map(|n| n as u64),
                        })
                    })?
                    .collect::<Result<_, _>>()?,
                model_history: models
                    .query_map([&id], |r| {
                        Ok(ModelDayUsage {
                            date: r.get(0)?,
                            model: r.get(1)?,
                            total_tokens: r.get::<_, i64>(2)? as u64,
                            input_tokens: r.get::<_, Option<i64>>(3)?.map(|n| n as u64),
                            output_tokens: r.get::<_, Option<i64>>(4)?.map(|n| n as u64),
                            cached_tokens: r.get::<_, Option<i64>>(5)?.map(|n| n as u64),
                            cache_creation_tokens: r.get::<_, Option<i64>>(6)?.map(|n| n as u64),
                            cache_read_tokens: r.get::<_, Option<i64>>(7)?.map(|n| n as u64),
                        })
                    })?
                    .collect::<Result<_, _>>()?,
            };
            state.tools.insert(id, tool);
        }
        Ok(state)
    }

    fn write_database(tx: &Connection, state: &SavedState) -> rusqlite::Result<()> {
        tx.execute("DELETE FROM usage_tools", [])?;
        let mut tool_row = tx.prepare("INSERT INTO usage_tools(tool, allowed, period_json, period_verified, partial_json, models_json, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        let mut daily = tx.prepare("INSERT INTO usage_daily(tool, date, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens, recorded_responses) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")?;
        let mut model = tx.prepare("INSERT INTO usage_model_daily(tool, date, model, total_tokens, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)")?;
        let mut event = tx.prepare("INSERT INTO usage_events(tool, id, timestamp_utc, model, input_tokens, output_tokens, cached_tokens, cache_creation_tokens, cache_read_tokens, total_tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)")?;
        for (id, tool) in &state.tools {
            tool_row.execute(params![
                id,
                tool.allowed,
                tool.usage.as_ref().map(encode).transpose()?,
                tool.period_verified,
                tool.partial.as_ref().map(encode).transpose()?,
                encode(&tool.models)?,
                tool.updated_at
            ])?;
            for r in &tool.history {
                daily.execute(params![
                    id,
                    r.date,
                    r.input_tokens as i64,
                    r.output_tokens as i64,
                    r.cached_tokens as i64,
                    r.cache_creation_tokens.map(|n| n as i64),
                    r.cache_read_tokens.map(|n| n as i64),
                    r.total_tokens as i64,
                    r.recorded_responses.map(|n| n as i64)
                ])?;
            }
            for r in &tool.model_history {
                model.execute(params![
                    id,
                    r.date,
                    r.model,
                    r.total_tokens as i64,
                    r.input_tokens.map(|n| n as i64),
                    r.output_tokens.map(|n| n as i64),
                    r.cached_tokens.map(|n| n as i64),
                    r.cache_creation_tokens.map(|n| n as i64),
                    r.cache_read_tokens.map(|n| n as i64)
                ])?;
            }
            for r in tool.codex_records.values() {
                event.execute(params![
                    id,
                    r.id,
                    r.timestamp.to_rfc3339(),
                    r.model,
                    r.input_tokens as i64,
                    r.output_tokens as i64,
                    r.cached_tokens as i64,
                    Some(r.cache_creation_tokens as i64),
                    Some(r.cache_read_tokens as i64),
                    r.total_tokens as i64
                ])?;
            }
        }
        Ok(())
    }
}

fn verify_local_file(meta: &cap_std::fs::Metadata) -> Result<(), String> {
    if !meta.is_file() {
        return Err(STORAGE_ERROR.into());
    }
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 || meta.nlink() != 1
        {
            return Err(STORAGE_ERROR.into());
        }
    }
    Ok(())
}

fn encode(value: &impl Serialize) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn decode<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    use rusqlite::types::{Type, ValueRef};
    let empty = |error| rusqlite::Error::FromSqlConversionFailure(index, Type::Text, error);
    let bytes = match row.get_ref(index)? {
        ValueRef::Null => b"null".as_slice(),
        ValueRef::Text(bytes) if bytes.len() <= MAX_BYTES as usize => bytes,
        _ => {
            return Err(rusqlite::Error::InvalidColumnType(
                index,
                "usage metadata".into(),
                Type::Text,
            ))
        }
    };
    serde_json::from_slice(bytes).map_err(|e| empty(Box::new(e)))
}
