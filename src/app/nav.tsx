import { useQueryClient } from "@tanstack/react-query";
import {
	createContext,
	type ReactNode,
	type RefObject,
	useCallback,
	useContext,
	useMemo,
	useRef,
	useState,
} from "react";
import {
	type PanelImperativeHandle,
	usePanelRef,
} from "react-resizable-panels";
import { api } from "../lib/api";
import { keys } from "../lib/queries";

const STORAGE_KEY = "chingui.conversation";

/** What the reader shows: a document, a page, and optionally a passage to highlight. */
type View = "chat" | "library" | "reader" | "settings";

export type ReaderTarget = {
	documentId: string;
	page: number;
	passageId: string | null;
};

type Nav = {
	reader: ReaderTarget | null;
	/** Opens a document in the main pane. `closeReader` goes back to where it came from. */
	openReader: (target: ReaderTarget) => void;
	closeReader: () => void;
	view: View;
	/** Project the library view is scoped to, or null for every document. */
	libraryScope: string | null;
	openLibrary: (scope?: string | null) => void;
	setLibraryScope: (scope: string | null) => void;
	conversationId: string | null;
	select: (id: string | null) => void;
	/** Creates a conversation, optionally inside a project or chapter, and opens it. */
	newChat: (
		projectId?: string | null,
		chapterId?: string | null,
	) => Promise<void>;
	/** Settings fill the main pane. `closeSettings` goes back to the view it was opened from. */
	openSettings: () => void;
	closeSettings: () => void;
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
	const [view, setView] = useState<View>("chat");
	const [libraryScope, setLibraryScope] = useState<string | null>(null);
	const [reader, setReader] = useState<ReaderTarget | null>(null);
	const contextPanel = usePanelRef();

	const returnView = useRef<View>("library");
	const openReader = useCallback((target: ReaderTarget) => {
		setView((v) => {
			if (v !== "reader") returnView.current = v;
			return "reader";
		});
		setReader(target);
	}, []);
	const closeReader = useCallback(() => {
		setReader(null);
		setView(returnView.current);
	}, []);

	const settingsReturn = useRef<View>("chat");
	const openSettings = useCallback(() => {
		setView((v) => {
			if (v !== "settings") settingsReturn.current = v;
			return "settings";
		});
	}, []);
	const closeSettings = useCallback(() => setView(settingsReturn.current), []);

	const openLibrary = useCallback((scope?: string | null) => {
		if (scope !== undefined) setLibraryScope(scope);
		setView("library");
	}, []);

	const select = useCallback((id: string | null) => {
		setConversationId(id);
		setView("chat");
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
			reader,
			openReader,
			closeReader,
			view,
			libraryScope,
			openLibrary,
			setLibraryScope,
			conversationId,
			select,
			newChat,
			openSettings,
			closeSettings,
			contextPanel,
			toggleContext,
		}),
		[
			reader,
			openReader,
			closeReader,
			view,
			libraryScope,
			openLibrary,
			conversationId,
			select,
			newChat,
			openSettings,
			closeSettings,
			contextPanel,
			toggleContext,
		],
	);

	return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
