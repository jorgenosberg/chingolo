import type { QueryClient } from "@tanstack/react-query";
import { useEffect, useSyncExternalStore } from "react";
import {
	type Document,
	events,
	type ImportSummary,
	type Progress,
} from "./bindings";
import { keys } from "./queries";

/**
 * Live import state that doesn't belong in the query cache: page progress, the drop
 * highlight and the last import summary. Lives outside React like `streams.ts`.
 */
type State = {
	progress: ReadonlyMap<string, Progress>;
	dropActive: boolean;
	summary: ImportSummary | null;
};

let state: State = { progress: new Map(), dropActive: false, summary: null };
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
export const useDropActive = () => useStore((s) => s.dropActive);
export const useImportSummary = () => useStore((s) => s.summary);
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
				if (document.status === "ready") {
					qc.invalidateQueries({ queryKey: ["library-search"] });
				}
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

		return () => {
			disposed = true;
			for (const fn of unlisten) fn();
		};
	}, [qc, onSummary]);
}
