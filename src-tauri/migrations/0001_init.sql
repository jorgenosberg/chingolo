-- Chingolo schema, version 1.

-- Projects, chapters and chat ---------------------------------------------------------------

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE chapters (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX chapters_project ON chapters(project_id, position);

CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    chapter_id TEXT REFERENCES chapters(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    provider TEXT NOT NULL,
    model_id TEXT,
    -- JSON describing which documents or collections the chat may use
    source_scope TEXT,
    archived_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- A fork's copied messages end at fork_boundary_message_id. The UI collapses everything up to it.
    forked_from_title TEXT,
    fork_boundary_message_id TEXT
);
CREATE INDEX conversations_project ON conversations(project_id, updated_at);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL CHECK (status IN ('streaming', 'complete', 'error', 'cancelled', 'interrupted')),
    provider TEXT,
    model_id TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    -- error text, or a note such as a non-STOP finish reason
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    -- input tokens served from the provider's prompt cache
    cached_input_tokens INTEGER,
    -- All turns of a reply that used tools (JSON array of Turn), replayed to the model as stored.
    -- Null for plain replies. `content` stays the visible text.
    turns TEXT,
    -- JSON: the app's check of each [S<n>] handle in the reply.
    citations TEXT
);
-- ids are UUIDv7, so ordering by id is ordering by creation time
CREATE INDEX messages_conversation ON messages(conversation_id, id);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    content,
    content='messages',
    content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER messages_au AFTER UPDATE OF content ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO messages_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Research library. Documents are global and shared by projects; indexed content is stored once.

CREATE TABLE documents (
    id TEXT PRIMARY KEY,
    -- sha256 of the original file. The managed copy is named by it.
    content_hash TEXT NOT NULL UNIQUE,
    original_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('pdf', 'docx')),
    title TEXT,
    -- JSON array of strings
    authors TEXT NOT NULL DEFAULT '[]',
    year INTEGER,
    doi TEXT,
    page_count INTEGER,
    status TEXT NOT NULL CHECK (status IN ('queued', 'processing', 'ready', 'no_text', 'error', 'cancelled')),
    -- why the document is in error, or a note such as "3 pages could not be read"
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    journal TEXT,
    publisher TEXT,
    volume TEXT,
    issue TEXT,
    pages TEXT,
    series TEXT,
    isbn TEXT,
    issn TEXT,
    -- JSON list of field names the user changed. A refresh never touches those fields.
    meta_edited TEXT NOT NULL DEFAULT '[]',
    -- JSON map of field name to where its value came from: "file", "crossref" or "ai"
    meta_source TEXT NOT NULL DEFAULT '{}',
    -- When the details lookups (Crossref, model) finished, so an interrupted run is picked up
    -- again. `enrich_error` says why the last one failed.
    enriched_at INTEGER,
    enrich_error TEXT
);
CREATE INDEX documents_status ON documents(status);

CREATE TABLE project_documents (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, document_id)
);
CREATE INDEX project_documents_document ON project_documents(document_id);

-- Page geometry as the viewer shows it (points). Not used for DOCX.
CREATE TABLE pages (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    page_no INTEGER NOT NULL,
    width REAL NOT NULL,
    height REAL NOT NULL,
    quality TEXT NOT NULL CHECK (quality IN ('ok', 'low', 'none')),
    -- 1 when the text came from text recognition (OCR) instead of the file's text layer
    ocr INTEGER NOT NULL DEFAULT 0,
    -- when layout ran on this page, so an interrupted run resumes
    layout_at INTEGER,
    PRIMARY KEY (document_id, page_no)
) WITHOUT ROWID;

CREATE TABLE passages (
    id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    -- PDF pages (1-based). Both 0 for DOCX, where ordinal is the only location.
    page_start INTEGER NOT NULL,
    page_end INTEGER NOT NULL,
    text TEXT NOT NULL,
    -- lowercased with æ/ø/å folded, what the full-text index sees
    text_folded TEXT NOT NULL,
    -- JSON: [{"page": 3, "rects": [[left, bottom, right, top], ...]}] in PDF user space. Null for DOCX.
    boxes TEXT,
    UNIQUE (document_id, ordinal)
);

CREATE VIRTUAL TABLE passages_fts USING fts5(
    text_folded,
    content='passages',
    content_rowid='rowid',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER passages_ai AFTER INSERT ON passages BEGIN
    INSERT INTO passages_fts(rowid, text_folded) VALUES (new.rowid, new.text_folded);
END;
CREATE TRIGGER passages_ad AFTER DELETE ON passages BEGIN
    INSERT INTO passages_fts(passages_fts, rowid, text_folded) VALUES ('delete', old.rowid, old.text_folded);
END;

-- One vector per passage. Deleting a passage (a document delete, or a re-read of the file)
-- removes its vector through the cascade. A passage with no row for the current model is pending.
CREATE TABLE passage_vectors (
    passage_id TEXT PRIMARY KEY REFERENCES passages(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    revision TEXT NOT NULL,
    -- little-endian f32, L2-normalized
    vec BLOB NOT NULL
) WITHOUT ROWID;

-- Page layout from the layout model: where body text, headers, footers, tables and figures are.
-- Boxes are in PDF user space like passage boxes. `ordinal` is the model's reading order.
CREATE TABLE page_regions (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    page_no INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    label TEXT NOT NULL,
    score REAL NOT NULL,
    left REAL NOT NULL,
    bottom REAL NOT NULL,
    right REAL NOT NULL,
    top REAL NOT NULL,
    PRIMARY KEY (document_id, page_no, ordinal)
) WITHOUT ROWID;

-- Tables the table model found inside `table` layout regions. One row per region that was read,
-- whatever the outcome, so an interrupted run resumes and a failed table is not retried.
-- `status`: ok (grid used for passages), unsure (characters or numbers did not add up, plain
-- lines kept), failed (the model errored, `note` says why). Boxes are PDF user space.
CREATE TABLE tables (
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    page_no INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    status TEXT NOT NULL,
    note TEXT,
    rows INTEGER NOT NULL DEFAULT 0,
    cols INTEGER NOT NULL DEFAULT 0,
    left REAL NOT NULL,
    bottom REAL NOT NULL,
    right REAL NOT NULL,
    top REAL NOT NULL,
    -- text inside the region but outside the grid (footnotes, sources), kept with the table
    footer TEXT,
    PRIMARY KEY (document_id, page_no, ordinal)
) WITHOUT ROWID;

-- `verified` is NULL until a second reader (Apple Vision) has compared the cell. `guessed` marks
-- cells that got words by position only (the words sit outside the cell's box).
CREATE TABLE table_cells (
    document_id TEXT NOT NULL,
    page_no INTEGER NOT NULL,
    table_ordinal INTEGER NOT NULL,
    row INTEGER NOT NULL,
    col INTEGER NOT NULL,
    row_span INTEGER NOT NULL,
    col_span INTEGER NOT NULL,
    text TEXT NOT NULL,
    left REAL NOT NULL,
    bottom REAL NOT NULL,
    right REAL NOT NULL,
    top REAL NOT NULL,
    verified INTEGER,
    guessed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (document_id, page_no, table_ordinal, row, col),
    FOREIGN KEY (document_id, page_no, table_ordinal) REFERENCES tables(document_id, page_no, ordinal) ON DELETE CASCADE
) WITHOUT ROWID;

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
    -- the short author-year label ("Boyd et al. 2004") the source had when first shown
    label TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (conversation_id, handle),
    UNIQUE (conversation_id, passage_id)
) WITHOUT ROWID;

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
    cost_usd REAL,
    -- the conversation the call belonged to, NULL for calls with none (indexing)
    conversation_id TEXT
);
CREATE INDEX api_usage_at ON api_usage(at);
CREATE INDEX api_usage_conversation ON api_usage(conversation_id);
