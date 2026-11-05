-- A fork's copied messages end at this message. The UI collapses everything up to it.
ALTER TABLE conversations ADD COLUMN forked_from_title TEXT;
ALTER TABLE conversations ADD COLUMN fork_boundary_message_id TEXT;
