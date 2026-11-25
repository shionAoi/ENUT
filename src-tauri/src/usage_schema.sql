CREATE TABLE usage_tools (
    tool TEXT PRIMARY KEY,
    allowed INTEGER NOT NULL CHECK (allowed IN (0, 1)),
    period_json TEXT,
    period_verified INTEGER NOT NULL CHECK (period_verified IN (0, 1)),
    partial_json TEXT,
    models_json TEXT NOT NULL,
    updated_at TEXT
) STRICT;

CREATE TABLE usage_daily (
    tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
    date TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cached_tokens INTEGER NOT NULL,
    cache_creation_tokens INTEGER,
    cache_read_tokens INTEGER,
    total_tokens INTEGER NOT NULL,
    recorded_responses INTEGER,
    PRIMARY KEY (tool, date)
) STRICT;

CREATE TABLE usage_model_daily (
    tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
    date TEXT NOT NULL,
    model TEXT NOT NULL,
    total_tokens INTEGER NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cached_tokens INTEGER,
    cache_creation_tokens INTEGER,
    cache_read_tokens INTEGER,
    PRIMARY KEY (tool, date, model)
) STRICT;

CREATE INDEX usage_daily_by_date ON usage_daily(date, tool);
CREATE INDEX usage_model_daily_by_date ON usage_model_daily(date, tool);

CREATE TABLE usage_events (
    tool TEXT NOT NULL REFERENCES usage_tools(tool) ON DELETE CASCADE,
    id TEXT NOT NULL,
    timestamp_utc TEXT NOT NULL,
    model TEXT,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cached_tokens INTEGER NOT NULL,
    cache_creation_tokens INTEGER,
    cache_read_tokens INTEGER,
    total_tokens INTEGER NOT NULL,
    PRIMARY KEY (tool, id)
) STRICT;
