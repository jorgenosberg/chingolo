-- More details per document, and a record of which fields the user changed.
-- `meta_edited` is a JSON list of field names. A refresh never touches those fields.
-- `meta_source` maps a field name to where its value came from: "file", "crossref" or "ai".
ALTER TABLE documents ADD COLUMN journal TEXT;
ALTER TABLE documents ADD COLUMN publisher TEXT;
ALTER TABLE documents ADD COLUMN volume TEXT;
ALTER TABLE documents ADD COLUMN issue TEXT;
ALTER TABLE documents ADD COLUMN pages TEXT;
ALTER TABLE documents ADD COLUMN series TEXT;
ALTER TABLE documents ADD COLUMN isbn TEXT;
ALTER TABLE documents ADD COLUMN issn TEXT;
ALTER TABLE documents ADD COLUMN meta_edited TEXT NOT NULL DEFAULT '[]';
ALTER TABLE documents ADD COLUMN meta_source TEXT NOT NULL DEFAULT '{}';
