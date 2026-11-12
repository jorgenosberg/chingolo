-- Whether the details lookups (Crossref, model) finished for a document, so an interrupted
-- run is picked up again. `enrich_error` says why the last one failed, for the library to show.
ALTER TABLE documents ADD COLUMN enriched_at INTEGER;
ALTER TABLE documents ADD COLUMN enrich_error TEXT;
