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
    PRIMARY KEY (document_id, page_no, ordinal)
) WITHOUT ROWID;

-- `verified` is NULL until a second reader has compared the cell (Apple Vision, later).
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
    PRIMARY KEY (document_id, page_no, table_ordinal, row, col),
    FOREIGN KEY (document_id, page_no, table_ordinal) REFERENCES tables(document_id, page_no, ordinal) ON DELETE CASCADE
) WITHOUT ROWID;
