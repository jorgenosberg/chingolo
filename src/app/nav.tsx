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

const STORAGE_KEY = "chingolo.conversation";

/** What the reader shows: a document, a page, and optionally a passage to highlight. */
type View = "chat" | "library" | "reader" | "settings";

export type ReaderTarget = {
	documentId: string;
	page: number;
	passageId: string | null;
	/** Set by a citation: the saved copy to show if the document or passage is gone. */
	source?: { conversationId: string; handle: number };
};

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

	// A conversation left without a message is not kept.
	const openId = useRef(conversationId);
	const select = useCallback(
		(id: string | null, opts?: { messageId?: string }) => {
			const left = openId.current;
			openId.current = id;
			if (left && left !== id) {
				api
					.discardEmptyConversation(left)
					.then((gone) => {
						if (gone) qc.invalidateQueries({ queryKey: keys.conversations });
					})
					.catch(() => {});
			}
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
		[qc],
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
