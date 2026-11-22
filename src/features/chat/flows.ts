import { useQueryClient } from "@tanstack/react-query";
import { useNav } from "../../app/nav";
import { useDialogs } from "../../components/dialogs";
import { api } from "../../lib/api";
import type { Conversation } from "../../lib/bindings";
import { keys } from "../../lib/queries";
import { setDraft } from "./drafts";

const HANDOFF_INTRO =
	"Notes from my earlier conversation, to use as background:\n\n";
const HANDOFF_OUTRO = "\n\nNext: ";

/** Fork and handoff, shared by the sidebar menu, the chat header and message actions. */
export function useConversationFlows() {
	const qc = useQueryClient();
	const { select } = useNav();
	const { busy, alert } = useDialogs();

	const fail = (title: string, e: unknown) =>
		alert({ title, message: e instanceof Error ? e.message : String(e) });

	const open = async (id: string) => {
		await qc.invalidateQueries({ queryKey: keys.conversations });
		select(id);
	};

	/** Copies the conversation, up to and including one message if given, into a new one. */
	const fork = async (c: Conversation, fromMessageId: string | null = null) => {
		try {
			await open((await api.forkConversation(c.id, fromMessageId)).id);
		} catch (e) {
			await fail("Couldn't fork the conversation", e);
		}
	};

	/**
	 * The conversation's own model writes a short note, which becomes an editable draft in a new
	 * conversation. The user reads it before anything is sent to a model.
	 */
	const handoff = async (c: Conversation) => {
		try {
			const note = await busy("Writing a handoff note…", () =>
				api.summarizeForHandoff(c.id),
			);
			const next = await api.createConversation(c.projectId, c.chapterId);
			setDraft(next.id, HANDOFF_INTRO + note + HANDOFF_OUTRO);
			await open(next.id);
		} catch (e) {
			await fail("Couldn't write the handoff note", e);
		}
	};

	return { fork, handoff };
}
