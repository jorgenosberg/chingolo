-- Pages whose text came from text recognition (OCR) rather than from the file's own text layer.
ALTER TABLE pages ADD COLUMN ocr INTEGER NOT NULL DEFAULT 0;
