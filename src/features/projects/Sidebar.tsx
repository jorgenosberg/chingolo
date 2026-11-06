import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { Button as RacButton } from "react-aria-components";
import { useNav } from "../../app/nav";
import {
	type Action,
	ActionMenu as RowMenu,
} from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import {
	ChatIcon,
	ChevronIcon,
	FolderIcon,
	LibraryIcon,
	PlusIcon,
	SearchIcon,
	SettingsIcon,
} from "../../components/icons";
import { cx, focus, IconButton, Kbd, SHORTCUTS } from "../../components/ui";
import { api } from "../../lib/api";
import type { Conversation, Project } from "../../lib/bindings";
import {
	keys,
	useChapters,
	useConversations,
	useDocuments,
	useProjects,
} from "../../lib/queries";
import { useConversationFlows } from "../chat/flows";

function Row({
	depth = 0,
	selected,
	onPress,
	icon,
	label,
	menu,
	expanded,
	shortcut,
}: {
	shortcut?: string;
	depth?: number;
	selected?: boolean;
	onPress: () => void;
	icon: ReactNode;
	label: string;
	menu?: ReactNode;
	expanded?: boolean;
}) {
	return (
		<div className="group relative">
			<RacButton
				onPress={onPress}
				aria-current={selected ? "true" : undefined}
				aria-expanded={expanded}
				style={{ paddingLeft: 8 + depth * 14 }}
				className={cx(
					"flex h-8 w-full items-center gap-2 rounded-lg pr-8 text-left text-[13px] transition-colors",
					"data-[hovered]:bg-muted",
					selected && "bg-border/60 font-medium",
					focus,
				)}
			>
				<span className="shrink-0 text-muted-foreground">{icon}</span>
				<span className="truncate">{label}</span>
				{shortcut && <Kbd keys={shortcut} className="ml-auto" />}
			</RacButton>
			{menu && (
				<div className="absolute right-1 top-1 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100 has-[[data-pressed]]:opacity-100">
					{menu}
				</div>
			)}
		</div>
	);
}

function SectionHeader({
	title,
	onAdd,
	addLabel,
}: {
	title: string;
	onAdd?: () => void;
	addLabel?: string;
}) {
	return (
		<div className="flex items-center justify-between px-2 pb-1 pt-4">
			<h2 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
				{title}
			</h2>
			{onAdd && (
				<IconButton
					label={addLabel ?? `Add ${title}`}
					onPress={onAdd}
					className="size-6"
				>
					<PlusIcon />
				</IconButton>
			)}
		</div>
	);
}

function useConversationActions() {
	const qc = useQueryClient();
	const { select, conversationId } = useNav();
	const { prompt, confirm } = useDialogs();
	const { fork, handoff } = useConversationFlows();
	const refresh = () => qc.invalidateQueries({ queryKey: keys.conversations });

	return (c: Conversation): Action[] => [
		{
			id: "rename",
			label: "Rename",
			run: async () => {
				const title = await prompt({
					title: "Rename conversation",
					label: "Title",
					initial: c.title,
				});
				if (title) {
					await api.renameConversation(c.id, title);
					refresh();
				}
			},
		},
		{ id: "fork", label: "Fork", run: () => fork(c) },
		{
			id: "handoff",
			label: "Hand off to a new conversation",
			run: () => handoff(c),
		},
		{
			id: "archive",
			label: c.archivedAt ? "Unarchive" : "Archive",
			run: async () => {
				await api.archiveConversation(c.id, !c.archivedAt);
				refresh();
			},
		},
		{
			id: "delete",
			label: "Delete…",
			danger: true,
			run: async () => {
				const ok = await confirm({
					title: "Delete conversation?",
					message: `"${c.title}" and all its messages will be deleted. This can't be undone.`,
					confirmLabel: "Delete",
					danger: true,
				});
				if (!ok) return;
				await api.deleteConversation(c.id);
				if (conversationId === c.id) select(null);
				refresh();
			},
		},
	];
}

function ConversationRow({ c, depth }: { c: Conversation; depth: number }) {
	const { conversationId, select } = useNav();
	const actionsFor = useConversationActions();
	return (
		<Row
			depth={depth}
			icon={<ChatIcon className="size-3.5" />}
			label={c.title}
			selected={conversationId === c.id}
			onPress={() => select(c.id)}
			menu={
				<RowMenu label={`Actions for ${c.title}`} actions={actionsFor(c)} />
			}
		/>
	);
}

function ProjectItem({
	project,
	conversations,
}: {
	project: Project;
	conversations: Conversation[];
}) {
	const qc = useQueryClient();
	const { newChat, select, conversationId } = useNav();
	const { prompt, confirm } = useDialogs();
	const chapters = useChapters(project.id);
	const [open, setOpen] = useState(true);
	const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
	const toggleChapter = (id: string) =>
		setCollapsed((s) => {
			const next = new Set(s);
			if (!next.delete(id)) next.add(id);
			return next;
		});

	const inProject = conversations.filter((c) => c.projectId === project.id);
	const loose = inProject.filter((c) => !c.chapterId);

	const refreshChapters = () =>
		qc.invalidateQueries({ queryKey: keys.chapters(project.id) });

	const actions: Action[] = [
		{
			id: "chat",
			label: "New conversation",
			run: () => {
				setOpen(true);
				newChat(project.id);
			},
		},
		{
			id: "chapter",
			label: "Add folder…",
			run: async () => {
				const title = await prompt({
					title: "Add folder",
					label: "Folder name",
					submitLabel: "Add",
				});
				if (title) {
					await api.createChapter(project.id, title);
					setOpen(true);
					refreshChapters();
				}
			},
		},
		{
			id: "rename",
			label: "Rename…",
			run: async () => {
				const name = await prompt({
					title: "Rename project",
					label: "Name",
					initial: project.name,
				});
				if (name) {
					await api.renameProject(project.id, name);
					qc.invalidateQueries({ queryKey: keys.projects });
				}
			},
		},
		{
			id: "delete",
			label: "Delete…",
			danger: true,
			run: async () => {
				const ok = await confirm({
					title: "Delete project?",
					message: `"${project.name}", its folders and its ${inProject.length} conversation${inProject.length === 1 ? "" : "s"} will be deleted. This can't be undone.`,
					confirmLabel: "Delete project",
					danger: true,
				});
				if (!ok) return;
				await api.deleteProject(project.id);
				if (inProject.some((c) => c.id === conversationId)) select(null);
				qc.invalidateQueries({ queryKey: keys.projects });
				qc.invalidateQueries({ queryKey: keys.conversations });
			},
		},
	];

	return (
		<div>
			<Row
				icon={
					<ChevronIcon
						className={cx("size-3.5 transition-transform", open && "rotate-90")}
					/>
				}
				label={project.name}
				expanded={open}
				onPress={() => setOpen((o) => !o)}
				menu={
					<RowMenu label={`Actions for ${project.name}`} actions={actions} />
				}
			/>
			{open && (
				<div>
					{(chapters.data ?? []).map((ch) => (
						<div key={ch.id}>
							<Row
								depth={1}
								icon={<FolderIcon className="size-3.5" />}
								label={ch.title}
								expanded={!collapsed.has(ch.id)}
								onPress={() => toggleChapter(ch.id)}
								menu={
									<RowMenu
										label={`Actions for ${ch.title}`}
										actions={[
											{
												id: "chat",
												label: "New conversation",
												run: () => newChat(project.id, ch.id),
											},
											{
												id: "rename",
												label: "Rename…",
												run: async () => {
													const title = await prompt({
														title: "Rename folder",
														label: "Folder name",
														initial: ch.title,
													});
													if (title) {
														await api.renameChapter(ch.id, title);
														refreshChapters();
													}
												},
											},
											{
												id: "delete",
												label: "Delete…",
												danger: true,
												run: async () => {
													const ok = await confirm({
														title: "Delete folder?",
														message: `"${ch.title}" is removed. Its conversations stay in the project.`,
														confirmLabel: "Delete folder",
														danger: true,
													});
													if (!ok) return;
													await api.deleteChapter(ch.id);
													refreshChapters();
													qc.invalidateQueries({
														queryKey: keys.conversations,
													});
												},
											},
										]}
									/>
								}
							/>
							{!collapsed.has(ch.id) &&
								inProject
									.filter((c) => c.chapterId === ch.id)
									.map((c) => <ConversationRow key={c.id} c={c} depth={2} />)}
						</div>
					))}
					{loose.map((c) => (
						<ConversationRow key={c.id} c={c} depth={1} />
					))}
					{!chapters.data?.length && !loose.length && (
						<p className="py-1 pl-8 text-xs text-muted-foreground">Empty</p>
					)}
				</div>
			)}
		</div>
	);
}

export function Sidebar() {
	const { newChat, openSettings, view, openLibrary, setSearchOpen } = useNav();
	const documents = useDocuments(null);
	const { prompt } = useDialogs();
	const qc = useQueryClient();
	const projects = useProjects();
	const conversations = useConversations();
	const [showArchived, setShowArchived] = useState(false);

	const all = conversations.data ?? [];
	const active = all.filter((c) => !c.archivedAt);
	const archived = all.filter((c) => c.archivedAt);
	const unassigned = active.filter((c) => !c.projectId);

	const addProject = async () => {
		const name = await prompt({
			title: "New project",
			label: "Project name",
			placeholder: "Master's thesis",
			submitLabel: "Create",
		});
		if (name) {
			await api.createProject(name);
			qc.invalidateQueries({ queryKey: keys.projects });
		}
	};

	return (
		<nav aria-label="Library" className="flex h-full flex-col bg-sidebar">
			{/* Room for the window buttons; also drags the window. */}
			<div
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center justify-end px-3"
			>
				<IconButton
					label="Search"
					shortcut={SHORTCUTS.search}
					onPress={() => setSearchOpen(true)}
				>
					<SearchIcon />
				</IconButton>
			</div>

			<div className="px-3 pb-2">
				<RacButton
					onPress={() => newChat()}
					className={cx(
						"flex h-9 w-full items-center gap-2 rounded-lg bg-primary px-3 text-[13px] font-medium text-primary-foreground transition-opacity data-[hovered]:opacity-90",
						focus,
					)}
				>
					<PlusIcon />
					New conversation
					<Kbd
						keys={SHORTCUTS.newChat}
						className="ml-auto text-primary-foreground/70"
					/>
				</RacButton>
			</div>

			<div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
				<div className="pt-2">
					<Row
						icon={<LibraryIcon className="size-3.5" />}
						label={
							documents.data?.length
								? `Library (${documents.data.length})`
								: "Library"
						}
						selected={view === "library"}
						onPress={() => openLibrary()}
						shortcut={SHORTCUTS.library}
					/>
				</div>
				<SectionHeader
					title="Projects"
					onAdd={addProject}
					addLabel="New project"
				/>
				{projects.data?.map((p) => (
					<ProjectItem key={p.id} project={p} conversations={active} />
				))}
				{projects.isSuccess && projects.data.length === 0 && (
					<p className="px-2 text-xs text-muted-foreground">
						Projects group your chats by thesis or course.
					</p>
				)}

				<SectionHeader
					title="Conversations"
					onAdd={() => newChat()}
					addLabel="New conversation"
				/>
				{unassigned.map((c) => (
					<ConversationRow key={c.id} c={c} depth={0} />
				))}
				{conversations.isSuccess && unassigned.length === 0 && (
					<p className="px-2 text-xs text-muted-foreground">
						No conversations yet.
					</p>
				)}

				{archived.length > 0 && (
					<>
						<div className="pt-4">
							<Row
								icon={
									<ChevronIcon
										className={cx(
											"size-3.5 transition-transform",
											showArchived && "rotate-90",
										)}
									/>
								}
								label={`Archived (${archived.length})`}
								expanded={showArchived}
								onPress={() => setShowArchived((s) => !s)}
							/>
						</div>
						{showArchived &&
							archived.map((c) => (
								<ConversationRow key={c.id} c={c} depth={1} />
							))}
					</>
				)}
			</div>

			<footer className="shrink-0 border-t border-border px-3 py-2">
				<RacButton
					onPress={openSettings}
					className={cx(
						"flex h-8 w-full items-center gap-2 rounded-lg px-2 text-[13px] text-muted-foreground data-[hovered]:bg-muted data-[hovered]:text-foreground",
						focus,
					)}
				>
					<SettingsIcon /> Settings
					<Kbd keys={SHORTCUTS.settings} className="ml-auto" />
				</RacButton>
			</footer>
		</nav>
	);
}
