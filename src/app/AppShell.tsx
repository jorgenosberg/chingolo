import { useEffect } from "react";
import {
	Group,
	Panel,
	Separator,
	useDefaultLayout,
} from "react-resizable-panels";
import { SidebarRightIcon } from "../components/icons";
import { Button, IconButton } from "../components/ui";
import { ChatView } from "../features/chat/ChatView";
import { Sidebar } from "../features/projects/Sidebar";
import { SettingsDialog } from "../features/settings/SettingsDialog";
import { useConversations } from "../lib/queries";
import { useNav } from "./nav";

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
				</Button>
				<p className="text-xs text-muted-foreground">
					⌘N starts a conversation, ⌘K searches.
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
				<IconButton label="Hide context pane" onPress={toggleContext}>
					<SidebarRightIcon />
				</IconButton>
			</div>
			<p className="px-4 text-sm text-muted-foreground">
				Sources and the PDF reader will appear here once you import papers.
			</p>
		</aside>
	);
}

export function AppShell() {
	const nav = useNav();
	const conversations = useConversations();
	const layout = useDefaultLayout({
		id: "chingui-main",
		storage: layoutStorage(),
	});

	// Keyboard shortcuts.
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (!e.metaKey || e.altKey || e.ctrlKey) return;
			const current = conversations.data?.find(
				(c) => c.id === nav.conversationId,
			);
			if (e.key === "n") {
				e.preventDefault();
				nav.newChat(current?.projectId ?? null);
			} else if (e.key === "k") {
				e.preventDefault();
				document.getElementById("sidebar-search")?.focus();
			} else if (e.key === ",") {
				e.preventDefault();
				nav.openSettings();
			} else if (e.key === "\\") {
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
						{nav.conversationId ? (
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
			<SettingsDialog
				isOpen={nav.settingsOpen}
				onOpenChange={nav.setSettingsOpen}
			/>
		</div>
	);
}
