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
    updated_at INTEGER NOT NULL
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
