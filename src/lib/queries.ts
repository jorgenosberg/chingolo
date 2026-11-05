import {
	type QueryClient,
	useMutation,
	useQuery,
	useQueryClient,
} from "@tanstack/react-query";
import { api, Channel, sendMessageRaw } from "./api";
import type { ChatEvent, Message } from "./bindings";
import { appendDelta, bindMessage, endStream } from "./streams";

export const keys = {
	settings: ["settings"] as const,
	models: ["models"] as const,
	projects: ["projects"] as const,
	chapters: (projectId: string) => ["chapters", projectId] as const,
	conversations: ["conversations"] as const,
	search: (q: string) => ["conversations", "search", q] as const,
	messages: (conversationId: string) => ["messages", conversationId] as const,
	/** Prefix of every document list, whatever the scope. */
	documents: ["documents"] as const,
	documentsIn: (scope: string | null) => ["documents", scope] as const,
	passage: (id: string) => ["passage", id] as const,
	librarySearch: (q: string, scope: string | null) =>
		["library-search", scope, q] as const,
};

export function useSettings() {
	return useQuery({ queryKey: keys.settings, queryFn: api.getSettings });
}

export function useModels(enabled: boolean) {
	return useQuery({
		queryKey: keys.models,
		queryFn: api.listAllModels,
		enabled,
		staleTime: 10 * 60_000,
	});
}

export function useProjects() {
	return useQuery({ queryKey: keys.projects, queryFn: api.listProjects });
}

export function useChapters(projectId: string) {
	return useQuery({
		queryKey: keys.chapters(projectId),
		queryFn: () => api.listChapters(projectId),
	});
}

/** Every conversation, archived included. The sidebar groups them on the client. */
export function useConversations() {
	return useQuery({
		queryKey: keys.conversations,
		queryFn: () => api.listConversations({ includeArchived: true }),
	});
}

export function useConversationSearch(query: string) {
	const q = query.trim();
	return useQuery({
		queryKey: keys.search(q),
		queryFn: () => api.listConversations({ includeArchived: true, query: q }),
		enabled: q.length > 0,
	});
}

export function useMessages(conversationId: string | null) {
	return useQuery({
		queryKey: keys.messages(conversationId ?? ""),
		queryFn: () => api.listMessages(conversationId as string),
		enabled: conversationId !== null,
	});
}

/** Documents in a project, or the whole library when `scope` is null. */
export function useDocuments(scope: string | null) {
	return useQuery({
		queryKey: keys.documentsIn(scope),
		queryFn: () => api.listDocuments(scope),
	});
}

/** Keyword search over document text. Pass an already deferred query. */
export function useLibrarySearch(query: string, scope: string | null) {
	const q = query.trim();
	return useQuery({
		queryKey: keys.librarySearch(q, scope),
		queryFn: () => api.searchLibrary(q, scope),
		enabled: q.length > 0,
	});
}

/** A stored passage with its line boxes. Passages never change, so it is cached for good. */
export function usePassage(passageId: string | null) {
	return useQuery({
		queryKey: keys.passage(passageId ?? ""),
		queryFn: () => api.getPassage(passageId as string),
		enabled: passageId !== null,
		staleTime: Number.POSITIVE_INFINITY,
	});
}

/** Mutation that refreshes the given query keys when it succeeds. */
export function useApiMutation<A, R>(
	fn: (arg: A) => Promise<R>,
	invalidate: readonly (readonly unknown[])[],
) {
	const qc = useQueryClient();
	return useMutation({
		mutationFn: fn,
		onSuccess: () => {
			for (const queryKey of invalidate) qc.invalidateQueries({ queryKey });
		},
	});
}

/**
 * Sends a message and wires up streaming. The reply's text goes to the live store, and the
 * final message replaces the placeholder in the query cache when the stream ends.
 */
export async function sendMessage(
	qc: QueryClient,
	conversationId: string,
	content: string,
) {
	let pending = "";
	let frame = 0;
	let finishedEarly: Message | null = null;
	let started = false;

	const settle = (message: Message) => {
		endStream(conversationId);
		qc.setQueryData<Message[]>(keys.messages(conversationId), (old) =>
			old?.map((m) => (m.id === message.id ? message : m)),
		);
		qc.invalidateQueries({ queryKey: keys.conversations });
	};

	const channel = new Channel<ChatEvent>();
	channel.onmessage = (ev) => {
		if (ev.type === "delta") {
			pending += ev.text;
			if (!frame) {
				// Batch to one render per frame.
				frame = requestAnimationFrame(() => {
					frame = 0;
					appendDelta(conversationId, pending);
					pending = "";
				});
			}
			return;
		}
		cancelAnimationFrame(frame);
		frame = 0;
		pending = "";
		if (started) settle(ev.message);
		else finishedEarly = ev.message;
	};

	const result = await sendMessageRaw(conversationId, content, channel);
	started = true;
	const assistant = finishedEarly ?? result.assistantMessage;
	qc.setQueryData<Message[]>(keys.messages(conversationId), (old) => [
		...(old ?? []),
		result.userMessage,
		assistant,
	]);
	qc.invalidateQueries({ queryKey: keys.conversations });
	if (finishedEarly) endStream(conversationId);
	else bindMessage(conversationId, result.assistantMessage.id);
}
