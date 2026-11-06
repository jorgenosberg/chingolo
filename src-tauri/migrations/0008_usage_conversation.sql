-- Which conversation a call belonged to, so a conversation can show what it cost. Calls made
-- before this column existed, and calls with no conversation (indexing), stay NULL.
ALTER TABLE api_usage ADD COLUMN conversation_id TEXT;
CREATE INDEX api_usage_conversation ON api_usage(conversation_id);
