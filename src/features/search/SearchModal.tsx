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
	run: () => void;
};

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
		const list = q
			? (found.data ?? [])
			: [...(conversations.data ?? [])]
					.filter((c) => !c.archivedAt)
					.sort((a, b) => b.updatedAt - a.updatedAt)
					.slice(0, 12);
		return list.map((c) => ({
			id: c.id,
			label: c.title,
			detail: [
				c.projectId ? projectName.get(c.projectId) : null,
				c.archivedAt ? "Archived" : null,
			]
				.filter(Boolean)
				.join(" · "),
			icon: "chat" as const,
			run: () => select(c.id),
		}));
	}, [
		mode,
		q,
		found.data,
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

	const searching = mode === "conversations" && q.length > 0 && found.isPending;
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
						<span className="min-w-0 flex-1 truncate">{r.label}</span>
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
				{mode === "conversations" && found.isError && (
					<p role="alert" className="px-2.5 py-2 text-xs text-problem">
						{found.error.message}
					</p>
				)}
			</div>
		</div>
	);
}
