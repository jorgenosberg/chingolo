-- Saved items get a folder (the chat's folder, changeable), a "used in my text" mark and tags.
ALTER TABLE saved_items ADD COLUMN chapter_id TEXT REFERENCES chapters(id) ON DELETE SET NULL;
ALTER TABLE saved_items ADD COLUMN used_at INTEGER;
CREATE INDEX saved_items_chapter ON saved_items(chapter_id);

-- Only when the item is still in its chat's project: items could already be moved elsewhere, and
-- a folder from another project would break the rule that a folder belongs to the item's project.
UPDATE saved_items SET chapter_id = (
    SELECT c.chapter_id FROM conversations c
     WHERE c.id = saved_items.conversation_id AND c.project_id IS saved_items.project_id
)
WHERE conversation_id IS NOT NULL;

-- Tags are shared by all projects. `key` is the lowercased name, made in Rust because SQLite's
-- NOCASE only folds ASCII (Ørret and ørret must be one tag).
CREATE TABLE saved_tags (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    key TEXT NOT NULL UNIQUE,
    color TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE saved_item_tags (
    saved_item_id TEXT NOT NULL REFERENCES saved_items(id) ON DELETE CASCADE,
    tag_id TEXT NOT NULL REFERENCES saved_tags(id) ON DELETE CASCADE,
    PRIMARY KEY (saved_item_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX saved_item_tags_tag ON saved_item_tags(tag_id);
