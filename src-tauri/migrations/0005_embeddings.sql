-- One vector per passage. Deleting a passage (a document delete, or a re-read of the file)
-- removes its vector through the cascade. A passage with no row for the current model is pending.
CREATE TABLE passage_vectors (
    passage_id TEXT PRIMARY KEY REFERENCES passages(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    revision TEXT NOT NULL,
    -- little-endian f32, L2-normalized
    vec BLOB NOT NULL
) WITHOUT ROWID;
