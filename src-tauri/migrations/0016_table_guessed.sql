-- Cells that got words by position only (the words sit outside the cell's box).
ALTER TABLE table_cells ADD COLUMN guessed INTEGER NOT NULL DEFAULT 0;

-- The strict rule left these tables unsure. Read them again with the placement rule.
DELETE FROM tables WHERE status = 'unsure';
