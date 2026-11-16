-- Page layout from the layout model: where body text, headers, footers, tables and figures are.
-- Boxes are in PDF user space like passage boxes. `ordinal` is the model's reading order.
-- `pages.layout_at` says layout ran on that page, so an interrupted run resumes.
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

ALTER TABLE pages ADD COLUMN layout_at INTEGER;
