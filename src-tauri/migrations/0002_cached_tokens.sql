-- Input tokens served from the provider's prompt cache, to measure caching and model-switch cost
ALTER TABLE messages ADD COLUMN cached_input_tokens INTEGER;
