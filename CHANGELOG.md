# Changelog

## 0.1.0

First release. macOS on Apple Silicon only, not notarized.

- Projects and chats with Gemini or OpenAI, using your own API key (kept in the macOS Keychain). Per-conversation cost, forks and hand-off notes.
- A research library. Import PDF and Word files. Titles, authors and years are read from the file and checked against Crossref and similar sources. Scanned pages are read with local text recognition.
- Search that combines keywords and meaning, with a local layout model that drops headers and footers and keeps tables and footnotes apart from body text.
- Tables are read into grids from the PDF's own text. A second reader (Apple Vision) marks numbers and signs it cannot confirm.
- Grounded answers. The model searches the library with tools and cites numbered sources. The app checks each citation against the passages it supplied, and a click opens the PDF at the passage.
- Updates are checked at start and installed from a signed release.
