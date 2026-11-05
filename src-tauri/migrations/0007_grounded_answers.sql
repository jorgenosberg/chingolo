-- All turns of a reply that used tools (JSON array of Turn), replayed to the model as stored.
-- Null for plain replies. `content` stays the visible text.
ALTER TABLE messages ADD COLUMN turns TEXT;
-- JSON: the app's check of each [S<n>] handle in the reply.
ALTER TABLE messages ADD COLUMN citations TEXT;

-- Passages shown to the model in a conversation, numbered [S1], [S2], ... The model only ever
-- sees the handle. Title, page and text are a snapshot, so a citation still resolves and its
-- quotes can still be checked after the document is re-read (new passage ids) or deleted.
-- No foreign keys to passages or documents, for the same reason.
CREATE TABLE conversation_sources (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    handle INTEGER NOT NULL,
    passage_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    title TEXT NOT NULL,
    -- first page of the passage, 0 for DOCX
    page INTEGER NOT NULL,
    text TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, handle),
    UNIQUE (conversation_id, passage_id)
) WITHOUT ROWID;
