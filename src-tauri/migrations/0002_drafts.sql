-- Text typed in a conversation's composer and not sent yet.
ALTER TABLE conversations ADD COLUMN draft TEXT NOT NULL DEFAULT '';
