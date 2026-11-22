-- Saved items: replies, text selections and PDF passages the user keeps, with a note.
-- An item copies what it needs (text and the sources it cites), so it stays readable and its
-- citations keep opening after the conversation, project or document is gone.
CREATE TABLE saved_items (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('reply', 'selection', 'passage')),
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    conversation_id TEXT REFERENCES conversations(id) ON DELETE SET NULL,
    message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    -- the conversation's title when the item was saved
    conversation_title TEXT NOT NULL DEFAULT '',
    -- markdown with [S<n>] handles for a reply, plain text for a selection or a passage
    body TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX saved_items_project ON saved_items(project_id, created_at);

-- Same columns as conversation_sources, plus the DOI for reference lines. A passage item has
-- one row, handle 1, for the document it was selected in.
CREATE TABLE saved_item_sources (
    saved_item_id TEXT NOT NULL REFERENCES saved_items(id) ON DELETE CASCADE,
    handle INTEGER NOT NULL,
    -- empty when a selection in the reader matched no stored passage
    passage_id TEXT NOT NULL,
    document_id TEXT NOT NULL,
    title TEXT NOT NULL,
    label TEXT NOT NULL DEFAULT '',
    page INTEGER NOT NULL,
    text TEXT NOT NULL,
    doi TEXT,
    PRIMARY KEY (saved_item_id, handle)
) WITHOUT ROWID;
