import type { QueryClient } from "@tanstack/react-query";
import { useEffect, useSyncExternalStore } from "react";
import { api } from "./api";
import {
	type DocStatus,
	type Document,
	type EmbeddingStatus,
	events,
	type ImportSummary,
	type Progress,
} from "./bindings";
import { keys } from "./queries";

/**
 * Live import state that doesn't belong in the query cache: page progress, the drop
 * highlight, the last import summary and the search model's state. Lives outside React
 * like `streams.ts`.
 */
type State = {
	progress: ReadonlyMap<string, Progress>;
	/** Documents whose details are being refreshed or looked up. */
	details: ReadonlySet<string>;
	dropActive: boolean;
	summary: ImportSummary | null;
	embedding: EmbeddingStatus | null;
};

let state: State = {
	progress: new Map(),
	details: new Set(),
	dropActive: false,
	summary: null,
	embedding: null,
};
const listeners = new Set<() => void>();

function set(patch: Partial<State>) {
	state = { ...state, ...patch };
	for (const l of listeners) l();
}

function subscribe(cb: () => void) {
	listeners.add(cb);
	return () => listeners.delete(cb);
}

function useStore<T>(pick: (s: State) => T): T {
	return useSyncExternalStore(subscribe, () => pick(state));
}

export const useProgress = (id: string) =>
	useStore((s) => s.progress.get(id) ?? null);
export const useDetailsBusy = (id: string) =>
	useStore((s) => s.details.has(id));
export const useDetailsIds = () => useStore((s) => s.details);
export const useDropActive = () => useStore((s) => s.dropActive);
export const useImportSummary = () => useStore((s) => s.summary);
export const useEmbeddingStatus = () => useStore((s) => s.embedding);
export const setEmbedding = (embedding: EmbeddingStatus) => set({ embedding });
export const setImportSummary = (summary: ImportSummary | null) =>
	set({ summary });

function setProgress(id: string, p: Progress | null) {
	const next = new Map(state.progress);
	if (p) next.set(id, p);
	else if (!next.delete(id)) return;
	set({ progress: next });
}

/** Puts a changed document into every cached list that already holds it. */
function patchLists(qc: QueryClient, doc: Document) {
	let found = false;
	qc.setQueriesData<Document[]>({ queryKey: keys.documents }, (old) => {
		if (!old) return old;
		if (!old.some((d) => d.id === doc.id)) return old;
		found = true;
		return old.map((d) => (d.id === doc.id ? doc : d));
	});
	// A new document isn't in any list yet. The server knows which scopes it belongs to.
	if (!found) qc.invalidateQueries({ queryKey: keys.documents });
}

const lastStatus = new Map<string, DocStatus>();

/** Search results change when the model becomes ready and when indexing finishes. */
function embeddingFinishedSomething(
	before: EmbeddingStatus | null,
	after: EmbeddingStatus,
) {
	if (!before) return false;
	const becameReady =
		before.model.kind !== "ready" && after.model.kind === "ready";
	const allIndexed =
		before.indexed < before.total &&
		after.total > 0 &&
		after.indexed === after.total;
	return becameReady || allIndexed;
}

/** Listens for library events for the life of the component. Mount once. */
export function useLibraryEvents(qc: QueryClient, onSummary: () => void) {
	useEffect(() => {
		let disposed = false;
		const unlisten: (() => void)[] = [];
		const keep = (p: Promise<() => void>) =>
			p.then((fn) => (disposed ? fn() : unlisten.push(fn)));

		keep(
			events.documentChanged.listen(({ payload }) => {
				const { document, progress } = payload;
				patchLists(qc, document);
				setProgress(
					document.id,
					document.status === "processing" ? progress : null,
				);
				// The embedding worker sends ready documents again as vectors arrive.
				// Only a change to ready makes new text searchable.
				const before = lastStatus.get(document.id);
				lastStatus.set(document.id, document.status);
				if (document.status === "ready" && before !== "ready") {
					qc.invalidateQueries({ queryKey: ["library-search"] });
				}
			}),
		);
		keep(
			events.detailsActivity.listen(({ payload }) =>
				set({ details: new Set(payload.ids) }),
			),
		);
		keep(
			events.embeddingStatus.listen(({ payload }) => {
				if (embeddingFinishedSomething(state.embedding, payload)) {
					qc.invalidateQueries({ queryKey: ["library-search"] });
				}
				set({ embedding: payload });
			}),
		);
		keep(
			events.importSummary.listen(({ payload }) => {
				set({ summary: payload });
				// A drop can attach existing documents, which no change event reports.
				qc.invalidateQueries({ queryKey: keys.documents });
				onSummary();
			}),
		);
		keep(
			events.dropHover.listen(({ payload }) =>
				set({ dropActive: payload.active }),
			),
		);

		// Events may have arrived first, and they are newer.
		api.embeddingStatus().then((s) => {
			if (!disposed && !state.embedding) set({ embedding: s });
		});

		return () => {
			disposed = true;
			for (const fn of unlisten) fn();
		};
	}, [qc, onSummary]);
}

/**
 * What is wrong with a ready document, in words for the library row. Empty when all is well.
 * A field the user has edited counts as handled, even if it's empty. A report with a
 * publisher but no authors is taken as having a corporate author.
 */
export function missingFields(doc: Document) {
	if (doc.status !== "ready") return [];
	const free = (f: string) => !doc.metaEdited.includes(f);
	const missing: ("title" | "authors" | "year")[] = [];
	if (!doc.title && free("title")) missing.push("title");
	if (doc.authors.length === 0 && !doc.publisher && free("authors"))
		missing.push("authors");
	if (!doc.year && free("year")) missing.push("year");
	return missing;
}

export function attentionFor(doc: Document): string[] {
	if (doc.status !== "ready") return [];
	const issues = missingFields(doc).map((f) => `No ${f}`);
	// A blank cover or a lone figure page is normal, so only a real share counts.
	if (doc.weakPages >= 3 && doc.weakPages * 20 >= (doc.pageCount ?? 0))
		issues.push(
			`${doc.weakPages} ${doc.weakPages === 1 ? "page" : "pages"} hard to read`,
		);
	if (doc.enrichError) issues.push(doc.enrichError);
	return issues;
}
