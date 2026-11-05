-- One row per call to a paid API: chat replies, hand-off notes, indexing batches and search queries.
-- Tokens come from the provider's response, or are guessed from text length (estimated = 1).
-- cost_usd is estimated when the call is made and stays as it was if prices change later.
CREATE TABLE api_usage (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    kind TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens INTEGER NOT NULL DEFAULT 0,
    estimated INTEGER NOT NULL DEFAULT 0,
    latency_ms INTEGER NOT NULL DEFAULT 0,
    ok INTEGER NOT NULL DEFAULT 1,
    cost_usd REAL
);
CREATE INDEX api_usage_at ON api_usage(at);
