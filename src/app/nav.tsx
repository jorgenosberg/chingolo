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
import { flushDraft } from "../features/chat/drafts";
import { api } from "../lib/api";
import { keys, type SourceRef } from "../lib/queries";

const STORAGE_KEY = "chingolo.conversation";

/** What the reader shows: a document, a page, and optionally a passage to highlight. */
type View = "chat" | "library" | "reader" | "saved" | "settings" | "activity";

export type ReaderTarget = {
	documentId: string;
	page: number;
	passageId: string | null;
	/** Set by a citation or a saved item: the saved copy to show if the document or passage is gone. */
	source?: SourceRef;
};

/** Which saved items the Saved page lists: all, one project's, or those with no project. */
export type SavedScope = "all" | "none" | { projectId: string };

/** `nonce` changes on every request, so picking the same message twice still jumps. */
export type MessageTarget = {
	conversationId: string;
	messageId: string;
	nonce: number;
};

/** Width the split reader opens at when a citation is clicked. */
const CITATION_PANE_PX = 560;

type Nav = {
	/** A cited source, open beside the chat in the right pane. */
	citation: ReaderTarget | null;
	openCitation: (target: ReaderTarget) => void;
	closeCitation: () => void;
	reader: ReaderTarget | null;
	/** Opens a document in the main pane. `closeReader` goes back to where it came from. */
	openReader: (target: ReaderTarget) => void;
	closeReader: () => void;
	view: View;
	/** Project the library view is scoped to, or null for every document. */
	libraryScope: string | null;
	openLibrary: (scope?: string | null) => void;
	setLibraryScope: (scope: string | null) => void;
	savedScope: SavedScope;
	openSaved: (scope?: SavedScope) => void;
	/** Everything running in the background, one row per kind of work. */
	openActivity: () => void;
	/** One saved item on its own page. `openSaved` goes back to the list. */
	savedItemId: string | null;
	/** `compose` puts the cursor in the new-note box. */
	openSavedItem: (id: string, opts?: { compose?: boolean }) => void;
	savedCompose: boolean;
	setSavedScope: (scope: SavedScope) => void;
	conversationId: string | null;
	/** Opens a conversation, optionally at one of its messages (a search hit). */
	select: (id: string | null, opts?: { messageId?: string }) => void;
	/** A message the open chat should scroll to. The chat clears it once it has jumped. */
	messageTarget: MessageTarget | null;
	clearMessageTarget: () => void;
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
	/** Whether the right pane is showing. Kept in step with its size by `AppShell`. */
	contextOpen: boolean;
	setContextOpen: (open: boolean) => void;
	/** The centered search over conversations and projects. */
	searchOpen: boolean;
	setSearchOpen: (open: boolean) => void;
	/** The left sidebar shrinks to a narrow icon rail instead of hiding. */
	sidebarPanel: RefObject<PanelImperativeHandle | null>;
	sidebarCollapsed: boolean;
	setSidebarCollapsed: (collapsed: boolean) => void;
	toggleSidebar: () => void;
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
	const [messageTarget, setMessageTarget] = useState<MessageTarget | null>(
		null,
	);
	const targetNonce = useRef(0);
	const [view, setView] = useState<View>("chat");
	const [libraryScope, setLibraryScope] = useState<string | null>(null);
	const [savedScope, setSavedScope] = useState<SavedScope>("all");
	const [savedItemId, setSavedItemId] = useState<string | null>(null);
	const [savedCompose, setSavedCompose] = useState(false);
	const [reader, setReader] = useState<ReaderTarget | null>(null);
	const [citation, setCitation] = useState<ReaderTarget | null>(null);
	const contextPanel = usePanelRef();
	const [contextOpen, setContextOpen] = useState(true);
	const [searchOpen, setSearchOpen] = useState(false);
	const sidebarPanel = usePanelRef();
	const [sidebarCollapsed, setSidebarCollapsed] = useState(false);

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

	const openActivity = useCallback(() => setView("activity"), []);

	const openSaved = useCallback((scope?: SavedScope) => {
		if (scope !== undefined) setSavedScope(scope);
		setSavedItemId(null);
		setSavedCompose(false);
		setView("saved");
	}, []);
	const openSavedItem = useCallback(
		(id: string, opts?: { compose?: boolean }) => {
			setSavedItemId(id);
			setSavedCompose(opts?.compose ?? false);
			setView("saved");
		},
		[],
	);

	// A conversation left without a message or a draft is not kept.
	const openId = useRef(conversationId);
	const leave = useCallback(
		async (id: string) => {
			try {
				await flushDraft(id);
				await api.discardEmptyConversation(id);
			} catch {
				// leaving must not fail over a save
			}
			qc.invalidateQueries({ queryKey: keys.conversations });
		},
		[qc],
	);
	const select = useCallback(
		(id: string | null, opts?: { messageId?: string }) => {
			const left = openId.current;
			openId.current = id;
			if (left && left !== id) leave(left);
			setConversationId(id);
			setMessageTarget(
				id && opts?.messageId
					? {
							conversationId: id,
							messageId: opts.messageId,
							nonce: ++targetNonce.current,
						}
					: null,
			);
			setCitation(null);
			setView("chat");
			try {
				if (id) localStorage.setItem(STORAGE_KEY, id);
				else localStorage.removeItem(STORAGE_KEY);
			} catch {
				// remembering the selection is a convenience only
			}
		},
		[leave],
	);
	const clearMessageTarget = useCallback(() => setMessageTarget(null), []);

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

	const openCitation = useCallback(
		(target: ReaderTarget) => {
			setCitation(target);
			const p = contextPanel.current;
			if (!p) return;
			if (p.isCollapsed()) p.expand();
			if (p.getSize().inPixels < CITATION_PANE_PX)
				p.resize(`${CITATION_PANE_PX}px`);
		},
		[contextPanel],
	);
	// Closing the source closes the pane too, so it isn't left open and empty.
	const closeCitation = useCallback(() => {
		setCitation(null);
		contextPanel.current?.collapse();
	}, [contextPanel]);

	const toggleContext = useCallback(() => {
		const p = contextPanel.current;
		if (!p) return;
		if (p.isCollapsed()) p.expand();
		else p.collapse();
	}, [contextPanel]);

	const toggleSidebar = useCallback(() => {
		const p = sidebarPanel.current;
		if (!p) return;
		if (p.isCollapsed()) p.expand();
		else p.collapse();
	}, [sidebarPanel]);

	const value = useMemo<Nav>(
		() => ({
			citation,
			openCitation,
			closeCitation,
			reader,
			openReader,
			closeReader,
			view,
			libraryScope,
			openLibrary,
			setLibraryScope,
			savedScope,
			openSaved,
			openActivity,
			savedItemId,
			openSavedItem,
			savedCompose,
			setSavedScope,
			conversationId,
			select,
			messageTarget,
			clearMessageTarget,
			newChat,
			openSettings,
			closeSettings,
			contextPanel,
			toggleContext,
			contextOpen,
			setContextOpen,
			searchOpen,
			setSearchOpen,
			sidebarPanel,
			sidebarCollapsed,
			setSidebarCollapsed,
			toggleSidebar,
		}),
		[
			citation,
			openCitation,
			closeCitation,
			reader,
			openReader,
			closeReader,
			view,
			libraryScope,
			openLibrary,
			savedScope,
			openSaved,
			openActivity,
			savedItemId,
			openSavedItem,
			savedCompose,
			conversationId,
			select,
			messageTarget,
			clearMessageTarget,
			newChat,
			openSettings,
			closeSettings,
			contextPanel,
			toggleContext,
			contextOpen,
			searchOpen,
			sidebarPanel,
			sidebarCollapsed,
			toggleSidebar,
		],
	);

	return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}
