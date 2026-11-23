-- Opt-in usage statistics (see .context/REPORTING.md). Counts are cumulative per day and are
-- only written while the user has usage statistics turned on.
CREATE TABLE usage_counts (
    day TEXT NOT NULL,
    event TEXT NOT NULL,
    provider TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    bucket TEXT NOT NULL DEFAULT '',
    count INTEGER NOT NULL,
    PRIMARY KEY (day, event, provider, model, bucket)
);

-- Things counted once per conversation, such as passing 80% of the context window.
CREATE TABLE conversation_flags (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    flag TEXT NOT NULL,
    PRIMARY KEY (conversation_id, flag)
);
