# Changelog

## 0.1.2

Copying a reply now gives clean text, and scans are read once the text recognition model is ready.

- Copying a reply, with the copy button or by selecting text, no longer includes markdown marks or citation handles like [S2]. Bold, italics, headings, lists and tables stay formatted when you paste into Word or Google Docs.
- Scans imported before the text recognition model was downloaded are read again when the download finishes.
- Icon buttons show a tooltip, and menus, popovers and tooltips share one look. Menu rows highlight and show a pointer cursor on hover.
- A new conversation you leave without writing anything is no longer kept in the list.

## 0.1.1

Search now takes you to the exact message, and you can find text inside a chat.

- Search (⌘K) now finds single messages and shows the matching text. Clicking a result opens the chat at that message.
- ⌘F finds text in the open chat, with a match count and next and previous buttons.
- After an update, Chingolo shows what changed.

## 0.1.0

First release. macOS on Apple Silicon only, not notarized.

- Projects and chats with Gemini or OpenAI, using your own API key (kept in the macOS Keychain). Per-conversation cost, forks and hand-off notes.
- A research library. Import PDF and Word files. Titles, authors and years are read from the file and checked against Crossref and similar sources. Scanned pages are read with local text recognition.
- Search that combines keywords and meaning, with a local layout model that drops headers and footers and keeps tables and footnotes apart from body text.
- Tables are read into grids from the PDF's own text. A second reader (Apple Vision) marks numbers and signs it cannot confirm.
- Grounded answers. The model searches the library with tools and cites numbered sources. The app checks each citation against the passages it supplied, and a click opens the PDF at the passage.
- Updates are checked at start and installed from a signed release.
