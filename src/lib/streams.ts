import { useSyncExternalStore } from "react";

/**
 * Live text of replies that are still streaming, keyed by conversation.
 * Lives outside React so a reply keeps filling in while the user looks at another chat.
 */
export type LiveReply = { messageId: string | null; text: string };

const replies = new Map<string, LiveReply>();
const listeners = new Set<() => void>();

function emit() {
	for (const l of listeners) l();
}

export function appendDelta(conversationId: string, text: string) {
	const cur = replies.get(conversationId) ?? { messageId: null, text: "" };
	replies.set(conversationId, { ...cur, text: cur.text + text });
	emit();
}

/** Deltas can arrive before the send command returns, so keep any text already received. */
export function bindMessage(conversationId: string, messageId: string) {
	const cur = replies.get(conversationId) ?? { messageId: null, text: "" };
	replies.set(conversationId, { ...cur, messageId });
	emit();
}

export function endStream(conversationId: string) {
	if (replies.delete(conversationId)) emit();
}

export function useLiveReply(
	conversationId: string | null,
): LiveReply | undefined {
	return useSyncExternalStore(
		(cb) => {
			listeners.add(cb);
			return () => listeners.delete(cb);
		},
		() => (conversationId ? replies.get(conversationId) : undefined),
	);
}
