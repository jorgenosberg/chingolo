import { api } from "../../lib/api";
import type { Conversation } from "../../lib/bindings";

/** Unsent text per conversation. The backend keeps a copy so it survives a restart. */
const live = new Map<string, string>();
const timers = new Map<string, ReturnType<typeof setTimeout>>();
const SAVE_DELAY_MS = 500;

/** A conversation nobody has written in yet that still has unsent text. */
export const isDraft = (c: Conversation) =>
	!c.hasMessages && c.draft.trim() !== "";

/** The text for a composer: what was typed this session, else what the backend saved. */
export function getDraft(id: string, saved: string) {
	return live.get(id) ?? saved;
}

function save(id: string) {
	const text = live.get(id) ?? "";
	// Blank text is stored as empty, so a conversation with only spaces is not kept.
	return api.setConversationDraft(id, text.trim() ? text : "").catch(() => {});
}

/** Sets a conversation's unsent text and saves it shortly after the last change. */
export function setDraft(id: string, text: string) {
	live.set(id, text);
	clearTimeout(timers.get(id));
	timers.set(
		id,
		setTimeout(() => {
			timers.delete(id);
			save(id);
		}, SAVE_DELAY_MS),
	);
}

/** Saves a change that is still waiting, for when the user leaves the conversation. */
export async function flushDraft(id: string) {
	const timer = timers.get(id);
	if (timer === undefined) return;
	clearTimeout(timer);
	timers.delete(id);
	await save(id);
}
