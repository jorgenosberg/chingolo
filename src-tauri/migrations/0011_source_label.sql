-- The short author-year label ("Boyd et al. 2004") a source had when it was first shown. Empty for
-- sources saved before this column, and for documents with no authors or publisher.
ALTER TABLE conversation_sources ADD COLUMN label TEXT NOT NULL DEFAULT '';
