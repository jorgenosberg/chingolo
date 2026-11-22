-- A saved item can have several notes, like comments under a post. Each is written at its own
-- time. The old single `saved_items.note` column stays but is no longer read.
CREATE TABLE saved_notes (
    id TEXT PRIMARY KEY,
    saved_item_id TEXT NOT NULL REFERENCES saved_items(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX saved_notes_item ON saved_notes(saved_item_id, created_at);

-- Each existing note becomes the first note of its item.
INSERT INTO saved_notes (id, saved_item_id, body, created_at, updated_at)
SELECT lower(hex(randomblob(16))), id, trim(note), updated_at, updated_at
  FROM saved_items
 WHERE trim(note) <> '';
