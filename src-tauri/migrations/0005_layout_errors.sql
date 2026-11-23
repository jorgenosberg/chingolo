-- A document whose page layout fails is tried twice, then left alone until the user retries it.
-- Both columns are cleared when layout succeeds or the user presses Retry.
ALTER TABLE documents ADD COLUMN layout_error TEXT;
ALTER TABLE documents ADD COLUMN layout_attempts INTEGER NOT NULL DEFAULT 0;
