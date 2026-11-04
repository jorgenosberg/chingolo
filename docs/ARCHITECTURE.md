# Architecture

One local Tauri 2 app. No server, no account. Rust owns everything privileged. The React frontend only calls typed commands.

## Layout

```text
src/                     React frontend
  app/                   shell, panes, navigation state
  components/            shared UI
  features/{projects,chat,settings,...}
  lib/                   bindings.ts (generated), helpers
  styles/                tokens.css, Tailwind entry
src-tauri/
  src/
    commands/            thin Tauri command wrappers
    domain/              plain types and validation, no IO
    services/ai/         Provider trait + gemini.rs (+ openai.rs in M2)
    services/{ingestion,retrieval,references,export}/   (M2+)
    persistence/         SQLite access, migrations runner
    security/            Keychain access
  migrations/            numbered .sql files
```

## IPC boundary

- Commands and events are typed with tauri-specta, which generates `src/lib/bindings.ts`.
- The webview has no filesystem, shell or SQL access. Capabilities are limited to core defaults and the dialog plugin. CSP is strict.
- Streaming uses a Tauri `Channel` per generation, with a cancel command.

## Data

- SQLite via rusqlite (bundled, FTS5 on). WAL and foreign keys on. Migrations run at startup. IDs are UUIDv7 text.
- M1 tables: `projects`, `chapters`, `conversations` (with nullable `source_scope` JSON), `messages`, `messages_fts`, `settings`.
- Original files go in a managed library folder, named by content hash.

## AI providers

- `Provider` trait: `list_models`, `generate` (stream of events), `generate_structured`, `capabilities` (context window, caching, vision, tools).
- Features are gated on capabilities, not on provider names.
- API keys live in macOS Keychain and never reach the frontend, the DB, logs or exports.
- Streaming messages are written as `streaming`, flushed periodically, then finalized. Leftover `streaming` rows are marked interrupted on startup.

## Retrieval (M2)

Two modes, chosen per request from the provider's context window and the size of the selected sources:

1. **Whole-source mode.** Selected documents are sent as passages tagged with app-assigned IDs. Provider caching keeps repeat questions cheap.
2. **Tool mode.** The model gets `search_library`, `read_section` and `get_page` over a local hybrid index: FTS5 (unicode61, remove diacritics, for æøå) plus multilingual embeddings in sqlite-vec, merged by rank fusion, with neighbor expansion.

In both modes the model cites passage IDs. The app checks each ID against the records it actually supplied, resolves the page, text and coordinates, and drops or flags anything it cannot match. Quotes are compared against the stored text before they are shown as verbatim. Document text is treated as untrusted data, never as instructions.

## Ingestion (M2)

Import, hash and dedupe, parse (pdfium for PDF with per-page char boxes, zip + quick-xml for DOCX), extract metadata, segment into passages keeping page and offsets, index, embed. Jobs are cancellable and resumable, and report progress through events. Pages with missing or garbled text are flagged. Tables and equations are not guessed at.

## Privacy

- Local by default. Only the context needed for a request goes to the chosen provider, and the UI shows which one.
- A one-time notice appears before the first cloud request.
- No telemetry with content.
