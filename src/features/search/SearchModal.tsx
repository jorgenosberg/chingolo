import {
	ChatCircleIcon,
	FolderIcon,
	MagnifyingGlassIcon,
} from "@phosphor-icons/react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
	Dialog,
	Modal,
	ModalOverlay,
	Radio,
	RadioGroup,
} from "react-aria-components";
import { useNav } from "../../app/nav";
import { cx, focus } from "../../components/ui";
import {
	useConversationSearch,
	useConversations,
	useMessageSearch,
	useProjects,
} from "../../lib/queries";

type Mode = "conversations" | "projects";

const MODES: { id: Mode; label: string; placeholder: string }[] = [
	{
		id: "conversations",
		label: "Conversations",
		placeholder: "Search conversations",
	},
	{ id: "projects", label: "Projects", placeholder: "Search projects" },
];

type Result = {
	id: string;
	label: string;
	detail: string;
	icon: "chat" | "project";
	/** Matching message text, with the matched words between U+0002 and U+0003. */
	snippet?: string;
	run: () => void;
};

/** The snippet with its matched words in `<mark>`. Built as elements, never as HTML. */
function Snippet({ text }: { text: string }) {
	// End markers become start markers, so the pieces alternate: text, match, text, match, ...
	const parts = text
		.replace(/\s+/g, " ")
		.split("\u0003")
		.join("\u0002")
		.split("\u0002");
	return (
		<>
			{parts.map((p, i) =>
				// Odd parts sit between a start and an end marker.
				// biome-ignore lint/suspicious/noArrayIndexKey: parts never reorder
				i % 2 === 1 ? <mark key={i}>{p}</mark> : <span key={i}>{p}</span>,
			)}
		</>
	);
}

/**
 * A centered search over conversations and projects. Written so more kinds of results (documents,
 * for one) can be added as another mode.
 */
export function SearchModal() {
	const { searchOpen, setSearchOpen } = useNav();
	return (
		<ModalOverlay
			isOpen={searchOpen}
			onOpenChange={setSearchOpen}
			isDismissable
			className="fixed inset-0 z-50 flex items-start justify-center bg-black/30 p-4 pt-[14vh] backdrop-blur-[2px]"
		>
			<Modal className="w-full max-w-xl">
				<Dialog
					aria-label="Search"
					className="overflow-hidden rounded-2xl border border-border bg-card shadow-xl outline-none"
				>
					<SearchPanel close={() => setSearchOpen(false)} />
				</Dialog>
			</Modal>
		</ModalOverlay>
	);
}

// Mounted each time the modal opens, so the query and mode start fresh.
function SearchPanel({ close }: { close: () => void }) {
	const { select, openLibrary } = useNav();
	const [mode, setMode] = useState<Mode>("conversations");
	const [query, setQuery] = useState("");
	const [active, setActive] = useState(0);
	const listRef = useRef<HTMLDivElement>(null);

	const conversations = useConversations();
	const projects = useProjects();
	const found = useConversationSearch(mode === "conversations" ? query : "");
	const hits = useMessageSearch(mode === "conversations" ? query : "");
	const q = query.trim();

	const results = useMemo<Result[]>(() => {
		const projectName = new Map(
			(projects.data ?? []).map((p) => [p.id, p.name]),
		);
		if (mode === "projects") {
			const needle = q.toLowerCase();
			const all = conversations.data ?? [];
			return (projects.data ?? [])
				.filter((p) => !needle || p.name.toLowerCase().includes(needle))
				.map((p) => {
					const n = all.filter(
						(c) => c.projectId === p.id && !c.archivedAt,
					).length;
					return {
						id: p.id,
						label: p.name,
						detail: `${n} ${n === 1 ? "conversation" : "conversations"}`,
						icon: "project" as const,
						run: () => openLibrary(p.id),
					};
				});
		}
		const detailOf = (projectId: string | null, archived: boolean) =>
			[
				projectId ? projectName.get(projectId) : null,
				archived ? "Archived" : null,
			]
				.filter(Boolean)
				.join(" · ");
		// The conversation search also returns chats that only match in a message. Those show up
		// below as message hits, so only title matches are listed here.
		const needle = q.toLowerCase();
		const list = q
			? (found.data ?? []).filter((c) => c.title.toLowerCase().includes(needle))
			: [...(conversations.data ?? [])]
					.filter((c) => !c.archivedAt)
					.sort((a, b) => b.updatedAt - a.updatedAt)
					.slice(0, 12);
		const titles: Result[] = list.map((c) => ({
			id: c.id,
			label: c.title,
			detail: detailOf(c.projectId, !!c.archivedAt),
			icon: "chat" as const,
			run: () => select(c.id),
		}));
		const messages: Result[] = q
			? (hits.data ?? []).map((h) => ({
					id: `message:${h.messageId}`,
					label: h.conversationTitle,
					detail: detailOf(h.projectId, h.archived),
					icon: "chat" as const,
					snippet: h.snippet,
					run: () => select(h.conversationId, { messageId: h.messageId }),
				}))
			: [];
		return [...titles, ...messages];
	}, [
		mode,
		q,
		found.data,
		hits.data,
		conversations.data,
		projects.data,
		select,
		openLibrary,
	]);

	// A new query or mode starts at the top of the list.
	// biome-ignore lint/correctness/useExhaustiveDependencies: reset on these only
	useEffect(() => setActive(0), [mode, q]);
	useEffect(() => {
		listRef.current
			?.querySelector(`[data-index="${active}"]`)
			?.scrollIntoView({ block: "nearest" });
	}, [active]);

	const choose = (r: Result | undefined) => {
		if (!r) return;
		close();
		r.run();
	};

	const searching =
		mode === "conversations" &&
		q.length > 0 &&
		(found.isPending || hits.isPending);
	const meta = MODES.find((m) => m.id === mode) ?? MODES[0];
	const empty = !searching && results.length === 0;

	return (
		<div>
			<div className="flex items-center gap-2.5 border-b border-border px-4">
				<MagnifyingGlassIcon className="size-5 shrink-0 text-muted-foreground" />
				<input
					// biome-ignore lint/a11y/noAutofocus: the modal exists to type in
					autoFocus
					role="combobox"
					aria-expanded
					aria-controls="search-results"
					aria-activedescendant={
						results[active] ? `search-result-${active}` : undefined
					}
					aria-label={meta.placeholder}
					placeholder={meta.placeholder}
					value={query}
					onChange={(e) => setQuery(e.target.value)}
					onKeyDown={(e) => {
						if (e.nativeEvent.isComposing) return;
						if (e.key === "ArrowDown") {
							e.preventDefault();
							setActive((i) => Math.min(i + 1, results.length - 1));
						} else if (e.key === "ArrowUp") {
							e.preventDefault();
							setActive((i) => Math.max(i - 1, 0));
						} else if (e.key === "Enter") {
							e.preventDefault();
							choose(results[active]);
						}
					}}
					className="h-12 min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
				/>
			</div>

			<RadioGroup
				aria-label="Search in"
				orientation="horizontal"
				value={mode}
				onChange={(v) => setMode(v as Mode)}
				className="flex gap-1 px-3 pt-2"
			>
				{MODES.map((m) => (
					<Radio
						key={m.id}
						value={m.id}
						className={cx(
							"cursor-default rounded-md px-2.5 py-1 text-xs text-muted-foreground",
							"data-[hovered]:text-foreground data-[selected]:bg-muted data-[selected]:font-medium data-[selected]:text-foreground",
							focus,
						)}
					>
						{m.label}
					</Radio>
				))}
			</RadioGroup>

			<div
				ref={listRef}
				id="search-results"
				role="listbox"
				aria-label="Results"
				className="h-80 overflow-y-auto p-2"
			>
				{results.map((r, i) => (
					// biome-ignore lint/a11y/useKeyWithClickEvents: keys are handled on the input
					<div
						key={r.id}
						tabIndex={-1}
						id={`search-result-${i}`}
						data-index={i}
						role="option"
						aria-selected={i === active}
						onClick={() => choose(r)}
						onMouseMove={() => active !== i && setActive(i)}
						className={cx(
							"flex cursor-default items-center gap-2.5 rounded-lg px-2.5 py-2 text-[13px]",
							i === active && "bg-muted",
						)}
					>
						<span className="shrink-0 text-muted-foreground">
							{r.icon === "chat" ? (
								<ChatCircleIcon className="size-[17px]" />
							) : (
								<FolderIcon className="size-[17px]" />
							)}
						</span>
						<span className="min-w-0 flex-1">
							<span className="block truncate">{r.label}</span>
							{r.snippet && (
								<span className="block truncate text-xs text-muted-foreground [&_mark]:rounded-sm [&_mark]:bg-[oklch(0.88_0.15_90/0.6)] [&_mark]:text-foreground">
									<Snippet text={r.snippet} />
								</span>
							)}
						</span>
						{r.detail && (
							<span className="shrink-0 text-xs text-muted-foreground">
								{r.detail}
							</span>
						)}
					</div>
				))}
				{searching && (
					<p className="px-2.5 py-2 text-xs text-muted-foreground">
						Searching…
					</p>
				)}
				{empty && (
					<p className="px-2.5 py-2 text-xs text-muted-foreground">
						{q
							? "No matches."
							: mode === "projects"
								? "No projects yet."
								: "No conversations yet."}
					</p>
				)}
				{mode === "conversations" && (found.isError || hits.isError) && (
					<p role="alert" className="px-2.5 py-2 text-xs text-problem">
						{(found.error ?? hits.error)?.message}
					</p>
				)}
			</div>
		</div>
	);
}
