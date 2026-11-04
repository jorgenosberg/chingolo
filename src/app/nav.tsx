import { useQueryClient } from "@tanstack/react-query";
import {
	createContext,
	type ReactNode,
	type RefObject,
	useCallback,
	useContext,
	useMemo,
	useState,
} from "react";
import {
	type PanelImperativeHandle,
	usePanelRef,
} from "react-resizable-panels";
import { api } from "../lib/api";
import { keys } from "../lib/queries";

const STORAGE_KEY = "chingui.conversation";

type Nav = {
	conversationId: string | null;
	select: (id: string | null) => void;
	/** Creates a conversation, optionally inside a project or chapter, and opens it. */
	newChat: (
		projectId?: string | null,
		chapterId?: string | null,
	) => Promise<void>;
	settingsOpen: boolean;
	setSettingsOpen: (open: boolean) => void;
	openSettings: () => void;
	contextPanel: RefObject<PanelImperativeHandle | null>;
	toggleContext: () => void;
};

const Ctx = createContext<Nav | null>(null);

export function useNav(): Nav {
	const v = useContext(Ctx);
	if (!v) throw new Error("useNav needs <NavProvider>");
	return v;
}

function readSaved(): string | null {
	try {
		return localStorage.getItem(STORAGE_KEY);
	} catch {
		return null;
	}
}

export function NavProvider({ children }: { children: ReactNode }) {
	const qc = useQueryClient();
	const [conversationId, setConversationId] = useState<string | null>(
		readSaved,
	);
	const [settingsOpen, setSettingsOpen] = useState(false);
	const contextPanel = usePanelRef();

	const select = useCallback((id: string | null) => {
		setConversationId(id);
		try {
			if (id) localStorage.setItem(STORAGE_KEY, id);
			else localStorage.removeItem(STORAGE_KEY);
		} catch {
			// remembering the selection is a convenience only
		}
	}, []);

	const newChat = useCallback(
		async (
			projectId: string | null = null,
			chapterId: string | null = null,
		) => {
			const c = await api.createConversation(projectId, chapterId);
			await qc.invalidateQueries({ queryKey: keys.conversations });
			select(c.id);
		},
		[qc, select],
	);

	const toggleContext = useCallback(() => {
		const p = contextPanel.current;
		if (!p) return;
		if (p.isCollapsed()) p.expand();
		else p.collapse();
	}, [contextPanel]);

	const value = useMemo<Nav>(
		() => ({
			conversationId,
			select,
			newChat,
			settingsOpen,
			setSettingsOpen,
			openSettings: () => setSettingsOpen(true),
			contextPanel,
			toggleContext,
		}),
		[
			conversationId,
			select,
			newChat,
			settingsOpen,
			contextPanel,
			toggleContext,
		],
	);

	return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
