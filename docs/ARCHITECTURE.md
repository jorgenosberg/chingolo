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
    services/docfile.rs  chingolo-doc:// scheme that serves stored files to the reader
    services/tools.rs    library tools for the model (search_library, read_section, get_page, list_documents)
    services/citations.rs  checks [S<n>] handles in a finished reply
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
- M1 tables: `projects`, `chapters`, `conversations` (with nullable `source_scope` JSON, see Source scope), `messages` (plus `turns` and `citations` JSON, migration 0007), `messages_fts`, `settings`. `conversation_sources` holds the handles of a conversation.
- Message status is `streaming`, `complete`, `error`, `cancelled` or `interrupted`. Replies are flushed about twice a second while streaming, so a crash keeps most of the text.
- Each message records its provider, model and token counts. A conversation also reports `switch_cost_tokens`, computed from its latest reply (see Conversations and models).
- Original files go in a managed library folder, named by content hash.

## AI providers

- `Provider` trait: `list_models`, `generate` (stream of events) and `capabilities` (tools, implicit caching, vision). `generate_structured` is not built. Context windows come from `ModelInfo.input_token_limit` (the provider's list, or the price table for OpenAI).
- A conversation is a list of `Turn`s made of `Part`s (text, tool call, tool result). Providers emit `GenerationEvent::ToolCall` (complete) and a `Replay` record of the finished model turn (Gemini parts with thought signatures, OpenAI output items with encrypted reasoning). The record goes back untouched to the same provider and is ignored by another. Both are serde types so they can be stored with messages.
- Gemini uses `generateContent`. OpenAI uses the Responses API with `store: false`, so nothing is kept on their side.
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

macOS Keychain "Always Allow" is tied to the app's code signature. Ad-hoc dev builds get a new signature on every rebuild, so macOS asks again. `src-tauri/.cargo/config.toml` sets `scripts/dev-run.sh` as the cargo runner, which signs the debug binary with a local Apple Development certificate (if one is installed) using the identifier `app.chingolo.desktop`. Release builds use a proper identity (M4).

## Retrieval (M2)

Two modes, chosen per request from the provider's context window and the size of the selected sources:

1. **Whole-source mode.** Selected documents are sent as passages tagged with app-assigned IDs. Provider caching keeps repeat questions cheap.
2. **Tool mode.** The model gets `search_library`, `read_section` and `get_page` over a local hybrid index: FTS5 (unicode61, remove diacritics, for æøå) plus EmbeddingGemma vectors in a plain SQLite table scored in Rust (`persistence/vectors.rs`), merged by scaled score fusion (`services/retrieval.rs`), with neighbor expansion.

Built so far: tool mode only (whole-source mode is not built, deferred until the slice 9 benchmark). A conversation gets the four tools when the provider reports `capabilities().tools` and its scope has a ready document.

- **Source scope.** `conversations.source_scope` holds a `Scope` (`domain/library.rs`): `project` (the conversation's own project, no id stored), `library`, or `documents` with chosen ids. `NULL` is the default: the project if it has ready documents, else the library. `tools::scope_for` resolves it to a `DocFilter` (`All`, `Project`, `Documents`). A chosen scope never falls back to the library: with nothing ready it gives no tools. `persistence/library.rs::scope_clause` is the one place that turns a filter into SQL, used by keyword search, `vectors::scan`, `ready_document_count` and `list_documents_in`, so they can't disagree. `set_conversation_scope` (`persistence/chat.rs::change_scope`) validates: `project` needs a project, document lists are deduplicated, non-empty, at most 200 and must exist. The composer's `ScopePopover` sets it.
- **`list_documents`** returns title, year, pages and a plain status for every document in the scope (ready or not), with no ids or handles. `GROUNDED_PROMPT` tells the model to call it first for questions about each or all papers.

- **Handles.** The model cites `[S<n>]`, never a passage id. `persistence/sources.rs` numbers passages per conversation the first time a tool shows them, in the same transaction that builds the tool result, and keeps a snapshot of title, page and text (`conversation_sources`, no foreign keys, so it survives a re-read or a delete).
- **Loop.** `run_generation` (`services/chat.rs`) asks the model, runs its tool calls, sends the results back and repeats up to 4 tool rounds, then asks once more with no tools. The turns it produced (with provider replay records) are saved in `messages.turns` and rebuilt by `history()`. Every request is recorded in `api_usage` and logged. A tool error goes back to the model as a result.
- **Check.** When a reply finishes, `services/citations.rs` removes handles the app never gave out, checks quotes of four words or more against the stored text, and flags claims that share few words with the source (`ok`, `weak`, `quote_mismatch`). The result is saved in `messages.citations` and sent as `Message.citations`. The word-overlap test can't judge meaning, so it flags and never removes.
- Document text is treated as untrusted data, never as instructions (the prompt says so, and the tools return it as data).

## Ingestion (M2)

Import, hash and dedupe, parse (pdfium for PDF with per-page char boxes, zip + quick-xml for DOCX), extract metadata, segment into passages keeping page and offsets, index, embed. Jobs are cancellable and resumable, and report progress through events. Pages with no text layer are rendered with pdfium and read with PP-OCRv6 through `oar-ocr` (`ingestion/ocr.rs`, models downloaded on first use, `pages.ocr` marks them), then put back into PDF user space so highlights work. `oar-ocr` is vendored in `src-tauri/third_party` to share the `ort` rc.15 runtime with `fastembed`, and sessions must use `AutoDevicePolicy::PreferCPU` (the automatic device gives wrong text). Pages with garbled text are flagged. Layout, tables and figures are planned through the same engine (see STATUS).

Details (title, authors, year, DOI, journal, series, ISBN, ISSN) come from `ingestion/detect.rs` (rules over the first 4 pages with type sizes, plus file properties; only the edition year rule reads up to page 6). `persistence::library::apply_metadata` is the only writer and skips fields listed in `documents.meta_edited`. A `Job::Details` refreshes them without reading or indexing the file again. `ingestion/enrich.rs` runs after a document is ready, one at a time: an opt-in lookup by identifier (`lookup.rs`: DOI through Crossref then doi.org, arXiv ids through doi.org, PubMed ids through NCBI, ISBNs through the National Library of Norway and Open Library, each validated against the document's text by `crossref::title_in_text`), then an opt-in model reading of the first pages (`details_ai.rs`, validated the same way, usage kind `metadata`; a report with no ISBN or publisher also sends excerpts from later pages). Both only add to empty fields, and a fresh read never changes what an online record gave or clears what a model found. A third opt-in step, `title_step`, searches Crossref by title and first author, then DataCite by title, for documents no identifier matched (`crossref::search_hit_fits` decides). Each pass is recorded on the document (`enriched_at`, `enrich_error`, migration `0012`): a failed one is retried once after 45 s, and `Enricher::resume` queues unfinished documents on start and when a lookup is turned on. `ingestion/activity.rs` reports which documents have a job in flight (`DetailsActivity` event). The library derives "needs attention" in `src/lib/library.ts::attentionFor` from the fields, `weak_pages` and `enrich_error`.

### Embedding

- `services/embedding/engine.rs`: one worker thread (`embed`) with its own database connection. A passage of a `ready` document with no row in `passage_vectors` for the current model and revision is pending, so a restart, a model change and a re-read of a file all need no extra bookkeeping. The ingest worker wakes it when a document becomes ready. Batches are 8 passages so a search never waits long for the model.
- Keyword search never depends on this. `ready` means searchable by keyword. Embedding progress is reported separately (`EmbeddingStatus` event, `Document.embeddedCount`).
- Two backends (`Backend` in `services/embedding/mod.rs`), both 768 dimensions: `gemini` (default, `services/embedding/cloud.rs`, `gemini-embedding-2` through `batchEmbedContents`, key read from the Keychain per call, retries 429/500/503) and `local`. Each has its own model name and revision stored on every vector, and the worker drops vectors that don't match the current backend before it starts, so a switch re-indexes. Gemini starts only after the user's yes (settings key `embedding_cloud_accepted`). The choice is the settings key `embedding_backend`; without one, Gemini, except for users who already accepted the local download.
- `services/embedding/model.rs` (local backend): the pinned model files (revision and SHA-256 in the manifest), downloaded by Rust with `.part` files and Range resume, checksum verified before a file gets its real name. Files live in `<app data>/models/embeddinggemma-300m-q4/<revision>/`. The download starts only after the user agrees (settings key `embedding_download_accepted`) and resumes at startup.
- `services/embedding/mod.rs`: loads the model from those files through fastembed's user-defined model API (weights passed from memory), on CPU only. Documents get the prefix `title: none | text: `, queries `task: search result | query: `.
- Search (`search_library`): with a model and a query of 3 or more characters it embeds the query, then merges BM25 (any term, common words dropped) and the vector scan by scaled score fusion (each list scaled to 0..1, half the weight each). With the Sharpen search results setting on, the top 20 go to the conversation's model, which puts the relevant ones first (`services/rerank.rs`, usage kind `rerank`). Otherwise it is the old keyword search.

## Frontend shell

- Navigation state is in `src/app/nav.tsx` (`useNav`): the selected conversation, the main view (chat, library, reader, settings), the right pane (`citation`, `contextOpen`), the sidebar (`sidebarCollapsed`) and the search modal (`searchOpen`). `AppShell.tsx` owns the resizable panes (react-resizable-panels) and the keyboard shortcuts (`SHORTCUTS` in `components/ui.tsx`: ⌘N, ⌘K, ⌘, ⌘J, ⌘L, ⌘B).
- The left sidebar collapses to a 52 px icon rail (`Sidebar.tsx`). The right pane shows a cited source (the reader in `compact` mode) or an empty placeholder, and closing a source also collapses it.
- ⌘K opens `features/search/SearchModal.tsx`, a centered search with modes (conversations, projects). A new kind of result is a new mode.
- Replies render with Streamdown. `features/chat/citations.tsx` turns `[S12]` into chips with tooltips and builds the "Sources" list under a reply.

## Reader (M2)

- A document opens in the main pane (`features/reader`), later also as a split beside a chat. PDF.js renders it. Files reach the viewer through the `chingolo-doc://localhost/<document id>` URI scheme registered in Rust (`services/docfile.rs`). The id is looked up in the database and the managed copy is read, so the webview never supplies a path. Single byte ranges are supported.
- `get_passage` returns a passage with its line boxes (PDF user space). The reader converts two opposite corners through the page viewport and draws the boxes in percent of the page, which survives zoom. A passage without boxes opens the page with a note.
- PDF.js assets (wasm, CMaps, fonts, ICC profiles) are served from `/pdfjs/` by a small Vite plugin and copied into the build. The CSP allows `chingolo-doc:`, `wasm-unsafe-eval` and blob workers.
- Pinch zoom is a CSS transform preview with one PDF.js rescale at the end (`features/reader/pinch.ts`).
- Tailwind's `border-box` default must not apply to `.pdfViewer .page`, or the canvas and text layer disagree on size (see `index.css`).

## Logging and usage

- Logs: `tauri-plugin-log`, console plus a file in `~/Library/Logs/app.chingolo.desktop/` (2 MB, 5 kept). Our code logs at debug in dev builds and info in release, dependencies at info. Lines carry timings, counts, model ids and errors, never document text, messages or keys. Settings has a "Show log folder" button (`show_logs`, opens the app's own log folder).
- Usage: table `api_usage` (migration 0006, `conversation_id` added in 0008), one row per paid call, kinds `chat`, `handoff`, `embed_index`, `embed_query`, `rerank`, `metadata`. Chat and hand-off rows carry their conversation, so `get_conversation_cost` can total one conversation (indexing and search queries have none). Written by `services/chat.rs` and the embedding engine (`persistence/usage.rs`). The Gemini encoder reports prompt tokens from the response (`last_usage`), or an estimate from text length flagged `estimated`. `get_usage` groups by provider, model and kind for 24 hours, 7 days or all time.
- Cost: `services/pricing.rs` prices a call when it is recorded. The table is a trimmed models.dev copy: `src-tauri/data/prices.json` is bundled, and `keep_fresh` fetches a new one at startup and then daily when automatic updates are on (settings key `prices_auto_update`, default on) and the last success is over 7 days old (`prices_checked_at`). The Usage tab has the switch and an "Update prices now" button that fetches whatever the switch says, saving it as `prices.json` in the app data folder. The newer of the two wins, a feed that looks wrong is refused, and a failure only logs. Models without a price show tokens only. To refresh the bundled copy for a new build: `cargo test update_bundled_prices -- --ignored` in `src-tauri`.

## Privacy

- Local by default. Only the context needed for a request goes to the chosen provider, and the UI shows which one.
- A one-time notice appears before the first cloud request.
- Three opt-in settings send more. The online lookup sends only an identifier (DOI, arXiv id, PubMed id or ISBN) to Crossref, doi.org, NCBI, the National Library of Norway or Open Library. The title search sends a title and first author's surname to Crossref, and the title to DataCite. The details model sends the first pages of papers with missing details to the chosen provider.
- No telemetry with content.
