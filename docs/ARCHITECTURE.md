# Architecture

One local Tauri 2 app. No server, no account. Rust owns everything privileged. The React frontend only calls typed commands.

## Layout

```text
src/                     React frontend
  app/                   shell, panes, navigation state
  components/            shared UI
  features/{projects,chat,settings,library,reader,...}
  lib/                   bindings.ts (generated), api wrapper, react-query hooks, live-reply store
  styles/                index.css (design tokens, Tailwind entry)
src-tauri/
  src/
    commands/            thin Tauri command wrappers
    domain/              plain types and validation, no IO
    services/ai/         Provider trait, SSE parser, gemini.rs (+ openai.rs in M2)
    services/chat.rs     send, stream and persist a reply; handoff summary
    services/docfile.rs  chingui-doc:// scheme that serves stored files to the reader
    services/{ingestion,retrieval,references,export}/   (M2+)
    persistence/         SQLite access, migrations runner
    security/            Keychain access
  migrations/            numbered .sql files
  scripts/dev-run.sh     cargo runner that signs dev builds (see Dev signing)
```

## IPC boundary

- Commands and events are typed with tauri-specta, which generates `src/lib/bindings.ts`.
- The webview has no filesystem, shell or SQL access. Capabilities are core defaults plus window dragging. CSP is strict.
- Streaming uses a Tauri `Channel` per generation, with a cancel command. The send command returns right away with the saved user message and the empty reply; text arrives over the channel, and a final event carries the finished message.
- The frontend keeps live reply text in a store outside React, so a reply keeps filling while the user views another chat. Deltas can arrive before the send command returns, so the store merges them.

## Data

- SQLite via rusqlite (bundled, FTS5 on). WAL and foreign keys on. Migrations run at startup. IDs are UUIDv7 text.
- M1 tables: `projects`, `chapters`, `conversations` (with nullable `source_scope` JSON), `messages`, `messages_fts`, `settings`.
- Message status is `streaming`, `complete`, `error`, `cancelled` or `interrupted`. Replies are flushed about twice a second while streaming, so a crash keeps most of the text.
- Each message records its provider, model and token counts. A conversation also reports `switch_cost_tokens`, computed from its latest reply (see Conversations and models).
- Original files go in a managed library folder, named by content hash.

## AI providers

- `Provider` trait: `list_models`, `generate` (stream of events), `generate_structured`, `capabilities` (context window, caching, vision, tools).
- Features are gated on capabilities, not on provider names.
- API keys live in macOS Keychain and never reach the frontend, the DB, logs or exports.
- Streaming messages are written as `streaming`, flushed periodically, then finalized. Leftover `streaming` rows are marked interrupted on startup.
- Providers receive the API key per call and never store it. Model ids are listed from the provider at runtime and filtered to chat models.
- Errors from the provider are shown with their cause. Keys are sent as headers only, never in URLs.

## Conversations and models

- A conversation has a provider and a model. The model can change at any time. A different model can't reuse the old model's prompt cache, so after a successful reply the UI warns with the size of the history the new model must re-read (`switch_cost_tokens`, the input tokens of the latest reply). After a failed reply there is no warning.
- Fork copies finished messages, up to a chosen message, into a new conversation in the same project and chapter. Token counts are not copied.
- Handoff asks the conversation's own model for a short note and opens it as an editable draft in a new conversation. Nothing is stored or sent until the user sends it.

## Dev signing

macOS Keychain "Always Allow" is tied to the app's code signature. Ad-hoc dev builds get a new signature on every rebuild, so macOS asks again. `src-tauri/.cargo/config.toml` sets `scripts/dev-run.sh` as the cargo runner, which signs the debug binary with a local Apple Development certificate (if one is installed) using the identifier `app.chingui.desktop`. Release builds use a proper identity (M4).

## Retrieval (M2)

Two modes, chosen per request from the provider's context window and the size of the selected sources:

1. **Whole-source mode.** Selected documents are sent as passages tagged with app-assigned IDs. Provider caching keeps repeat questions cheap.
2. **Tool mode.** The model gets `search_library`, `read_section` and `get_page` over a local hybrid index: FTS5 (unicode61, remove diacritics, for æøå) plus multilingual embeddings in sqlite-vec, merged by rank fusion, with neighbor expansion.

In both modes the model cites passage IDs. The app checks each ID against the records it actually supplied, resolves the page, text and coordinates, and drops or flags anything it cannot match. Quotes are compared against the stored text before they are shown as verbatim. Document text is treated as untrusted data, never as instructions.

## Ingestion (M2)

Import, hash and dedupe, parse (pdfium for PDF with per-page char boxes, zip + quick-xml for DOCX), extract metadata, segment into passages keeping page and offsets, index, embed. Jobs are cancellable and resumable, and report progress through events. Pages with missing or garbled text are flagged. Tables and equations are not guessed at.

## Reader (M2)

- A document opens in the main pane (`features/reader`), later also as a split beside a chat. PDF.js renders it. Files reach the viewer through the `chingui-doc://localhost/<document id>` URI scheme registered in Rust (`services/docfile.rs`). The id is looked up in the database and the managed copy is read, so the webview never supplies a path. Single byte ranges are supported.
- `get_passage` returns a passage with its line boxes (PDF user space). The reader converts two opposite corners through the page viewport and draws the boxes in percent of the page, which survives zoom. A passage without boxes opens the page with a note.
- PDF.js assets (wasm, CMaps, fonts, ICC profiles) are served from `/pdfjs/` by a small Vite plugin and copied into the build. The CSP allows `chingui-doc:`, `wasm-unsafe-eval` and blob workers.
- Pinch zoom is a CSS transform preview with one PDF.js rescale at the end (`features/reader/pinch.ts`).
- Tailwind's `border-box` default must not apply to `.pdfViewer .page`, or the canvas and text layer disagree on size (see `index.css`).

## Privacy

- Local by default. Only the context needed for a request goes to the chosen provider, and the UI shows which one.
- A one-time notice appears before the first cloud request.
- No telemetry with content.
