import { useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense, useCallback, useEffect, useRef } from "react";
import {
	Group,
	Panel,
	Separator,
	useDefaultLayout,
} from "react-resizable-panels";
import { SidebarRightIcon } from "../components/icons";
import { Button, IconButton, Kbd, SHORTCUTS, Spinner } from "../components/ui";
import { ChatView } from "../features/chat/ChatView";
import { LibraryView } from "../features/library/LibraryView";
import { Sidebar } from "../features/projects/Sidebar";
import { SettingsView } from "../features/settings/SettingsView";
import { api } from "../lib/api";
import { useDropActive, useLibraryEvents } from "../lib/library";
import { useConversations, useProjects } from "../lib/queries";
import { useNav } from "./nav";

// The reader pulls in PDF.js, so it loads the first time a document is opened.
const ReaderView = lazy(() => import("../features/reader/ReaderView"));

function layoutStorage() {
	// localStorage can throw in restricted contexts; fall back to no persistence.
	try {
		return window.localStorage;
	} catch {
		return undefined;
	}
}

function Divider() {
	return (
		<Separator className="relative w-px bg-border outline-none transition-colors data-[separator=hover]:bg-primary/50 data-[separator=active]:bg-primary data-[separator=focus]:bg-primary">
			{/* Wider invisible grab area. */}
			<span className="absolute inset-y-0 -left-1.5 -right-1.5" />
		</Separator>
	);
}

function EmptyWorkspace() {
	const { newChat } = useNav();
	return (
		<div className="flex h-full flex-col">
			<div data-tauri-drag-region className="h-12 shrink-0" />
			<div className="flex flex-1 flex-col items-center justify-center gap-4 px-8 text-center">
				<div>
					<h1 className="text-xl font-semibold tracking-tight">Chingui</h1>
					<p className="mt-1 text-muted-foreground">
						A little help for big ideas.
					</p>
				</div>
				<Button variant="primary" onPress={() => newChat()}>
					New conversation
					<Kbd
						keys={SHORTCUTS.newChat}
						className="text-primary-foreground/70"
					/>
				</Button>
				<p className="text-xs text-muted-foreground">
					<Kbd keys={SHORTCUTS.search} /> searches,{" "}
					<Kbd keys={SHORTCUTS.context} /> toggles the context pane.
				</p>
			</div>
		</div>
	);
}

function ContextPane() {
	const { toggleContext } = useNav();
	return (
		<aside aria-label="Context" className="flex h-full flex-col bg-sidebar">
			<div
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center justify-between px-4"
			>
				<h2 className="text-[13px] font-medium">Context</h2>
				<IconButton
					label="Hide context pane"
					shortcut={SHORTCUTS.context}
					onPress={toggleContext}
				>
					<SidebarRightIcon />
				</IconButton>
			</div>
			<p className="px-4 text-sm text-muted-foreground">
				Sources cited in a conversation will open here.
			</p>
		</aside>
	);
}

function DropOverlay({ targetName }: { targetName: string | null }) {
	const active = useDropActive();
	if (!active) return null;
	return (
		<div
			aria-hidden="true"
			className="pointer-events-none fixed inset-2 z-40 flex items-center justify-center rounded-2xl border-2 border-dashed border-primary bg-background/80"
		>
			<p className="text-sm font-medium">
				Drop to add to {targetName ?? "the library"}
			</p>
		</div>
	);
}

export function AppShell() {
	const nav = useNav();
	const qc = useQueryClient();
	const conversations = useConversations();
	const projects = useProjects();
	const layout = useDefaultLayout({
		id: "chingui-main",
		storage: layoutStorage(),
	});

	// Dropped files go to the project in view: the library's scope, or the open chat's project.
	const currentChat = conversations.data?.find(
		(c) => c.id === nav.conversationId,
	);
	const importTarget =
		nav.view === "library" || nav.view === "reader"
			? nav.libraryScope
			: (currentChat?.projectId ?? null);
	useEffect(() => {
		api.setImportTarget(importTarget).catch(() => {});
	}, [importTarget]);

	// A drop opens the library on the project it went to.
	const targetRef = useRef(importTarget);
	targetRef.current = importTarget;
	const { openLibrary } = nav;
	const onSummary = useCallback(
		() => openLibrary(targetRef.current),
		[openLibrary],
	);
	useLibraryEvents(qc, onSummary);

	// Keyboard shortcuts.
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (!e.metaKey || e.altKey || e.ctrlKey) return;
			const current = conversations.data?.find(
				(c) => c.id === nav.conversationId,
			);
			if (e.key === SHORTCUTS.newChat) {
				e.preventDefault();
				nav.newChat(current?.projectId ?? null);
			} else if (e.key === SHORTCUTS.search) {
				e.preventDefault();
				document.getElementById("sidebar-search")?.focus();
			} else if (e.key === SHORTCUTS.settings) {
				e.preventDefault();
				nav.openSettings();
			} else if (e.key === SHORTCUTS.library) {
				e.preventDefault();
				nav.openLibrary();
			} else if (e.key === SHORTCUTS.context) {
				e.preventDefault();
				nav.toggleContext();
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [nav, conversations.data]);

	return (
		<div className="h-full">
			<Group
				orientation="horizontal"
				defaultLayout={layout.defaultLayout}
				onLayoutChanged={layout.onLayoutChanged}
				className="h-full"
			>
				<Panel id="sidebar" defaultSize="260px" minSize="200px" maxSize="420px">
					<Sidebar />
				</Panel>
				<Divider />
				<Panel id="main" minSize="420px">
					<main className="relative h-full bg-background">
						{nav.view === "settings" ? (
							<SettingsView />
						) : nav.view === "reader" ? (
							<Suspense
								fallback={
									<div className="flex h-full items-center justify-center">
										<Spinner />
									</div>
								}
							>
								<ReaderView />
							</Suspense>
						) : nav.view === "library" ? (
							<LibraryView />
						) : nav.conversationId ? (
							<ChatView
								key={nav.conversationId}
								conversationId={nav.conversationId}
							/>
						) : (
							<EmptyWorkspace />
						)}
					</main>
				</Panel>
				<Divider />
				<Panel
					id="context"
					panelRef={nav.contextPanel}
					collapsible
					collapsedSize="0px"
					defaultSize="300px"
					minSize="240px"
					maxSize="560px"
				>
					<ContextPane />
				</Panel>
			</Group>
			<DropOverlay
				targetName={
					projects.data?.find((p) => p.id === importTarget)?.name ?? null
				}
			/>
		</div>
	);
}
