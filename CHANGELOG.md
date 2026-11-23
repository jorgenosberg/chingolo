# Changelog

## 0.3.1

See how full a chat is, and get a warning before it gets more expensive.

- A small ring in the chat header shows how much of the model's context window the conversation uses. Hover it for the token count and how much of the last reply's input was cached. It turns red at 80%.
- Some models charge more for every message once a chat passes a certain size (for example double above 200k or 272k tokens). A warning next to the ring appears as a chat gets close to that size. It offers to hand off to a new conversation, or you can fork from an earlier message, or keep going.
- Handing off a conversation now reuses the model's cached input, so the note usually costs less to write.

## 0.3.0

See everything Chingolo is working on in one place, and pause, retry or cancel it.

- A line at the bottom of the sidebar shows when Chingolo is reading, indexing or downloading, when something is paused, or when something needs attention. Click it, or the status line on the library page, to open the new Activity page.
- The Activity page lists reading, page layout and tables, paper details, indexing and the model downloads. Each shows its progress, the document it is on, and anything that failed. Filter by running, waiting, paused or needs attention.
- Pause reading, page layout, paper details or indexing. The document being worked on finishes first, and the pause is kept after a restart. Cancel the document being read, skip one in page layout, retry anything that failed, and cancel or restart a model download, also from Settings.
- New imports are read before the page layout backlog, so a new folder no longer waits behind it.
- A chat with text you typed but didn't send shows a pencil before its title in the sidebar. The text is kept, also after a restart.
- A document whose page layout fails is tried twice, then left alone until you press Retry, and the Activity page shows why. Before, it was tried again at every start and the reason was only in the log.
- A quote saved from a document now finds its passage more often when the selection runs over a line that ends in a hyphen, so it opens with the highlight.
- If the macOS Keychain can't be read (locked or missing), Chingolo still opens. Settings says what went wrong, and saved API keys can't be used until it is unlocked.

## 0.2.0

Save replies, quotes and passages from your documents, and keep notes on them.

- Bookmark a reply with the button under it. Or select text in a reply or in a document and choose Save. A saved item keeps its text and the sources it cites, so it still opens after the chat or the document is deleted.
- The new Saved page (in the sidebar, and under each project) shows what you saved as a list or as a small or large grid. Filter by kind, source or notes, search the text, and move items between projects.
- Open a saved item to see the full text, every source with a button to open the document, a link back to the message in its chat, and your notes. An item can have several notes, each with its own date.
- Copying a saved item adds one plain reference line per source (author and year, title, page, DOI), ready to match against EndNote in Word.

## 0.1.2

Copying a reply now gives clean text, and scans are read once the text recognition model is ready.

- Copying a reply, with the copy button or by selecting text, no longer includes markdown marks or citation handles like [S2]. Bold, italics, headings, lists and tables stay formatted when you paste into Word or Google Docs.
- Scans imported before the text recognition model was downloaded are read again when the download finishes.
- Icon buttons show a tooltip, and menus, popovers and tooltips share one look. Menu rows highlight and show a pointer cursor on hover.
- A new conversation you leave without writing anything is no longer kept in the list. If you had started typing, it is kept as a draft under "Drafts" in the sidebar, and opening it brings back your text, also after a restart.

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
