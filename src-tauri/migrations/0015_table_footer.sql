-- Text inside a table region but outside its grid (footnotes, sources), kept with the table.
ALTER TABLE tables ADD COLUMN footer TEXT;

-- The first rule rejected tables over such text. Read them again with the new one: dropping the
-- unsure rows makes the table job pick them up (their cells go with them).
DELETE FROM tables WHERE status = 'unsure';
