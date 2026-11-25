import {
	ArrowsOutSimpleIcon,
	BookmarkSimpleIcon,
	CheckCircleIcon,
	CheckIcon,
	CopyIcon,
	FolderIcon,
	FolderSimpleIcon,
	GridNineIcon,
	ListIcon,
	MagnifyingGlassIcon,
	NoteIcon,
	PencilSimpleIcon,
	RowsIcon,
	SortAscendingIcon,
	SquaresFourIcon,
	TagIcon,
	TrashIcon,
} from "@phosphor-icons/react";
import { math } from "@streamdown/math";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Button as RacButton } from "react-aria-components";
import { Streamdown } from "streamdown";
import { type SavedScope, useNav } from "../../app/nav";
import { type Action, ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import { type Choice, FilterSelect } from "../../components/FilterSelect";
import { RowCheckbox } from "../../components/RowCheckbox";
import { TagChip } from "../../components/TagChip";
import { Button, cx, focus, IconButton, Spinner } from "../../components/ui";
import { api } from "../../lib/api";
import type {
	Chapter,
	Project,
	SavedFilter,
	SavedItem,
	SavedSource,
} from "../../lib/bindings";
import {
	keys,
	useAllChapters,
	useProjects,
	useSaved,
	useSavedTags,
} from "../../lib/queries";
import { useSelection } from "../../lib/useSelection";
import {
	CiteProvider,
	citeComponents,
	citePlugins,
	citeTags,
	savedAsCitations,
	where,
} from "../chat/citations";
import { cleanCopy, writeRich } from "../chat/copy";
import { MoveDialog } from "./MoveDialog";
import { TagFilter } from "./TagFilter";
import { TagsDialog } from "./TagsDialog";

export const plugins = { math };
const ALL = "__all__";
const NONE = "__none__";

function filterFor(scope: SavedScope): SavedFilter {
	return scope === "all"
		? { kind: "all" }
		: scope === "none"
			? { kind: "unassigned" }
			: { kind: "project", id: scope.projectId };
}

function ScopeSelect({
	value,
	onChange,
}: {
	value: SavedScope;
	onChange: (scope: SavedScope) => void;
}) {
	const projects = useProjects();
	return (
		<FilterSelect
			label="Show saved items from"
			value={value === "all" ? ALL : value === "none" ? NONE : value.projectId}
			items={[
				{ id: ALL, name: "All saved" },
				{ id: NONE, name: "No project" },
				...(projects.data ?? []).map((p) => ({ id: p.id, name: p.name })),
			]}
			onChange={(k) =>
				onChange(k === ALL ? "all" : k === NONE ? "none" : { projectId: k })
			}
		/>
	);
}

const esc = (s: string) =>
	s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** One line per document: `Boyd et al. 2004, "Title", pp. 3, 5, doi:10.1000/x`. For Word and EndNote. */
export function referenceLines(sources: SavedSource[]): string[] {
	const docs = new Map<string, SavedSource[]>();
	for (const s of sources)
		docs.set(s.documentId, [...(docs.get(s.documentId) ?? []), s]);
	return [...docs.values()].map((group) => {
		const first = group[0];
		const pages = [
			...new Set(group.map((s) => s.page).filter((p) => p > 0)),
		].sort((a, b) => a - b);
		const doi = group.find((s) => s.doi)?.doi;
		return [
			first.label || null,
			`“${first.title}”`,
			pages.length > 0
				? `${pages.length > 1 ? "pp." : "p."} ${pages.join(", ")}`
				: null,
			doi ? `doi:${doi}` : null,
		]
			.filter(Boolean)
			.join(", ");
	});
}

export function CopyItem({
	item,
	proseRef,
}: {
	item: SavedItem;
	proseRef: React.RefObject<HTMLDivElement | null>;
}) {
	const [done, setDone] = useState(false);
	return (
		<IconButton
			label={done ? "Copied" : "Copy with references"}
			onPress={async () => {
				// A reply is copied from what is drawn, so bold and lists survive; chips are dropped.
				const body =
					item.kind === "reply" && proseRef.current
						? cleanCopy(proseRef.current)
						: { html: `<p>${esc(item.body)}</p>`, text: item.body };
				const refs = referenceLines(item.sources);
				try {
					await writeRich({
						html: body.html + refs.map((r) => `<p>${esc(r)}</p>`).join(""),
						text: [body.text.trim(), ...refs].join("\n\n"),
					});
					setDone(true);
					setTimeout(() => setDone(false), 1500);
				} catch {
					// clipboard can be unavailable; nothing useful to show
				}
			}}
		>
			{done ? <CheckIcon /> : <CopyIcon />}
		</IconButton>
	);
}

export const KIND_LABEL = {
	reply: "Reply",
	selection: "Selection",
	passage: "From a document",
} as const;

/** The chat an item came from, or its kept title when the chat is gone. */
function conversationKey(i: SavedItem): string {
	return i.conversationId ?? `title:${i.conversationTitle}`;
}

type ViewMode = "list" | "small" | "large";
const VIEW_KEY = "chingolo.savedView";
/** Grid cards share one height, so a long reply scrolls inside its card. */
const ROW_HEIGHT: Record<ViewMode, string> = {
	list: "auto",
	large: "18rem",
	small: "12rem",
};
const COLUMNS: Record<ViewMode, string> = {
	list: "1fr",
	// As many columns as fit, so 2 or 3 on a typical window and 1 on a narrow pane.
	large: "repeat(auto-fill, minmax(min(100%, 22rem), 1fr))",
	small: "repeat(auto-fill, minmax(min(100%, 15rem), 1fr))",
};

function readView(): ViewMode {
	try {
		const v = localStorage.getItem(VIEW_KEY);
		return v === "list" || v === "small" || v === "large" ? v : "large";
	} catch {
		return "large";
	}
}

type KindFilter = "all" | SavedItem["kind"];
const KIND_CHIPS: { id: KindFilter; name: string }[] = [
	{ id: "all", name: "All" },
	{ id: "reply", name: "Replies" },
	{ id: "selection", name: "Selections" },
	{ id: "passage", name: "From documents" },
];

function Chip({
	pressed,
	onPress,
	children,
}: {
	pressed: boolean;
	onPress: () => void;
	children: React.ReactNode;
}) {
	return (
		<RacButton
			aria-pressed={pressed}
			onPress={onPress}
			className={cx(
				"h-7 rounded-full border px-3 text-xs transition-colors",
				pressed
					? "border-primary/40 bg-accent-soft font-medium text-foreground"
					: "border-border text-muted-foreground data-[hovered]:bg-muted data-[hovered]:text-foreground",
				focus,
			)}
		>
			{children}
		</RacButton>
	);
}

/** The words of a reply as plain text, for the short previews in the grid. */
function preview(markdown: string): string {
	return markdown
		.replace(/\s*\[S\d+\]/g, "")
		.replace(/```\w*\n?/g, "")
		.replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
		.replace(/^#{1,6}\s+/gm, "")
		.replace(/^\s*[-*+]\s+/gm, "• ")
		.replace(/(\*\*|__)(.+?)\1/g, "$2")
		.replace(/(^|\s)[*_]([^*_\n]+)[*_](?=\s|[.,;:!?]|$)/g, "$1$2")
		.replace(/`([^`]*)`/g, "$1")
		.replace(/\n{2,}/g, "\n")
		.trim();
}

/**
 * Text cut after as many whole lines as fit the space it is given, with an ellipsis on the last
 * one. The space is the parent's leftover height (`flex-1`), so every card cuts at its own edge.
 */
function ClampedText({
	children,
	className,
}: {
	children: string;
	className?: string;
}) {
	const box = useRef<HTMLDivElement>(null);
	const [lines, setLines] = useState(3);
	useLayoutEffect(() => {
		const el = box.current;
		if (!el) return;
		const fit = () => {
			const p = el.firstElementChild;
			if (!p) return;
			const lineHeight = Number.parseFloat(getComputedStyle(p).lineHeight);
			if (lineHeight > 0)
				setLines(Math.max(1, Math.floor(el.clientHeight / lineHeight)));
		};
		fit();
		const watch = new ResizeObserver(fit);
		watch.observe(el);
		return () => watch.disconnect();
	}, []);
	return (
		<div ref={box} className="min-h-0 flex-1 overflow-hidden">
			<p
				className={cx("selectable whitespace-pre-line", className)}
				style={{
					display: "-webkit-box",
					WebkitBoxOrient: "vertical",
					WebkitLineClamp: lines,
					overflow: "hidden",
				}}
			>
				{children}
			</p>
		</div>
	);
}

/** Refreshes the saved lists, and reports a failed change. */
export function useSavedRefresh() {
	const qc = useQueryClient();
	const { alert } = useDialogs();
	const refresh = () => {
		qc.invalidateQueries({ queryKey: keys.saved });
		qc.invalidateQueries({ queryKey: keys.savedTags });
	};
	const fail = (title: string) => (e: unknown) =>
		alert({
			title,
			message: e instanceof Error ? e.message : String(e),
		});
	return { refresh, fail };
}

/** Tags, move, used mark, delete. Shared by the card and the detail page. */
export function itemActions({
	item,
	refresh,
	fail,
	confirm,
	onTags,
	onMove,
	onDeleted,
}: {
	item: SavedItem;
	refresh: () => void;
	fail: (title: string) => (e: unknown) => void;
	confirm: ReturnType<typeof useDialogs>["confirm"];
	onTags: () => void;
	onMove: () => void;
	onDeleted?: () => void;
}): Action[] {
	return [
		{
			id: "tags",
			icon: <TagIcon className="size-4" />,
			label: "Tags…",
			run: onTags,
		},
		{
			id: "move",
			icon: <FolderSimpleIcon className="size-4" />,
			label: "Move to…",
			run: onMove,
		},
		{
			id: "used",
			icon: <CheckCircleIcon className="size-4" />,
			label: item.usedAt ? "Mark as not used" : "Mark as used in my text",
			run: () =>
				api
					.setSavedUsed([item.id], !item.usedAt)
					.then(refresh, fail("Couldn't change the mark")),
		},
		{
			id: "delete",
			icon: <TrashIcon className="size-4" />,
			label: "Delete…",
			danger: true,
			run: async () => {
				const ok = await confirm({
					title: "Delete saved item?",
					message:
						"The item and its notes are removed. The conversation and the documents are not touched.",
					confirmLabel: "Delete",
					danger: true,
				});
				if (!ok) return;
				try {
					await api.deleteSavedItems([item.id]);
					refresh();
					onDeleted?.();
				} catch (e) {
					fail("Couldn't delete")(e);
				}
			},
		},
	];
}

function SavedCard({
	item,
	projects,
	chapters,
	mode,
	selected,
	selecting,
	onSelect,
}: {
	item: SavedItem;
	projects: Project[];
	chapters: Chapter[];
	mode: ViewMode;
	selected: boolean;
	/** Something is selected, so every card shows its checkbox. */
	selecting: boolean;
	/** `range` is set when shift was held. */
	onSelect: (on: boolean, range: boolean) => void;
}) {
	const shift = useRef(false);
	const [dialog, setDialog] = useState<"tags" | "move" | null>(null);
	const { select, savedScope, openSavedItem } = useNav();
	const { confirm } = useDialogs();
	const proseRef = useRef<HTMLDivElement>(null);
	const { refresh, fail } = useSavedRefresh();
	const citations = useMemo(
		() => savedAsCitations(item.sources),
		[item.sources],
	);
	const project = projects.find((p) => p.id === item.projectId);
	const chapter = chapters.find((c) => c.id === item.chapterId);
	// The project is only named where the list mixes projects.
	const place = [savedScope === "all" ? project?.name : null, chapter?.title]
		.filter(Boolean)
		.join(" › ");
	const open = () => {
		// A click that ends a text selection is not a request to open.
		if (window.getSelection()?.isCollapsed === false) return;
		openSavedItem(item.id);
	};

	const actions: Action[] = [
		{
			id: "note",
			icon: <PencilSimpleIcon className="size-4" />,
			label: "Add note",
			run: () => openSavedItem(item.id, { compose: true }),
		},
		...itemActions({
			item,
			refresh,
			fail,
			confirm,
			onTags: () => setDialog("tags"),
			onMove: () => setDialog("move"),
		}),
	];

	const date = new Date(item.createdAt).toLocaleDateString(undefined, {
		month: "short",
		day: "numeric",
		year:
			new Date(item.createdAt).getFullYear() === new Date().getFullYear()
				? undefined
				: "numeric",
	});

	return (
		<>
			{/* biome-ignore lint/a11y/useKeyWithClickEvents: the Open button is the keyboard route */}
			<li
				onClick={(e) => {
					// Chips, buttons, links, the note field and the open menu keep their own clicks.
					const t = e.target as Element;
					if (
						t.closest(
							"button, a, label, textarea, input, [role=menu], [role=menuitem], [data-cite]",
						)
					)
						return;
					// While anything is selected, a click on a card selects it instead of opening it.
					if (selecting) {
						if (window.getSelection()?.isCollapsed === false) return;
						onSelect(!selected, e.shiftKey);
						return;
					}
					open();
				}}
				className={cx(
					"group flex min-h-0 cursor-pointer flex-col gap-2 rounded-xl border border-border bg-card px-4 py-3 transition-colors hover:border-primary/70 hover:ring-1 hover:ring-primary/30",
					selected && "border-primary/70 ring-1 ring-primary/30",
				)}
			>
				<div className="flex items-center gap-2 text-xs text-muted-foreground">
					<div
						className={cx(
							"shrink-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100",
							selecting || selected ? "opacity-100" : "opacity-0",
						)}
						onPointerDownCapture={(e) => {
							shift.current = e.shiftKey;
						}}
						onKeyDownCapture={(e) => {
							shift.current = e.shiftKey;
						}}
					>
						<RowCheckbox
							label={`Select ${KIND_LABEL[item.kind].toLowerCase()} from ${date}`}
							isSelected={selected}
							onChange={(on) => onSelect(on, shift.current)}
							// Reaches the card's corner, so a click near the box counts.
							className="-m-3 p-3"
						/>
					</div>
					<span className="min-w-0 flex-1 truncate">
						{KIND_LABEL[item.kind]} · {date}
						{place ? ` · ${place}` : ""}
					</span>
					{/* Shown on hover or focus, and while the menu is open. */}
					<span className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100 has-[[aria-expanded=true]]:opacity-100">
						<IconButton label="Open" className="size-6" onPress={open}>
							<ArrowsOutSimpleIcon />
						</IconButton>
						<CopyItem item={item} proseRef={proseRef} />
						<ActionMenu label="Saved item actions" actions={actions} />
					</span>
				</div>

				{item.kind === "reply" ? (
					<>
						{mode !== "list" && (
							<ClampedText
								className={cx(
									"leading-relaxed",
									mode === "small" ? "text-[13px]" : "text-[15px]",
								)}
							>
								{preview(item.body)}
							</ClampedText>
						)}
						{/* Hidden in the grid, where the preview shows. Copy reads the formatted reply. */}
						<div
							ref={proseRef}
							data-saved-prose={item.id}
							className={cx(
								"selectable chat-prose text-[15px]",
								mode !== "list" && "hidden",
							)}
						>
							<CiteProvider
								citations={citations}
								conversationId={item.conversationId ?? ""}
								savedItemId={item.id}
							>
								<Streamdown
									plugins={plugins}
									remarkPlugins={citePlugins}
									components={citeComponents}
									allowedTags={citeTags}
								>
									{item.body}
								</Streamdown>
							</CiteProvider>
						</div>
					</>
				) : mode === "list" ? (
					<blockquote className="selectable whitespace-pre-wrap border-l-2 border-primary/50 pl-3 text-[15px] leading-relaxed">
						{item.body}
					</blockquote>
				) : (
					<ClampedText
						className={cx(
							"border-l-2 border-primary/50 pl-3 leading-relaxed",
							mode === "small" ? "text-[13px]" : "text-[15px]",
						)}
					>
						{item.body}
					</ClampedText>
				)}

				{item.kind === "passage" && item.sources[0] && (
					<p className="shrink-0 truncate text-xs text-muted-foreground">
						{where(item.sources[0])}
					</p>
				)}

				{item.conversationTitle &&
					(item.conversationId && item.messageId ? (
						<RacButton
							onPress={() =>
								select(item.conversationId, {
									messageId: item.messageId ?? undefined,
								})
							}
							className={cx(
								"w-fit max-w-full truncate rounded text-xs text-primary underline-offset-2 hover:underline",
								focus,
							)}
						>
							From “{item.conversationTitle}”
						</RacButton>
					) : (
						<span className="truncate text-xs text-muted-foreground">
							From “{item.conversationTitle}” (conversation deleted)
						</span>
					))}

				{(item.tags.length > 0 || item.usedAt || item.notes.length > 0) && (
					<div className="flex shrink-0 items-center gap-1.5 overflow-hidden text-xs text-muted-foreground">
						{item.tags.slice(0, 3).map((t) => (
							<TagChip key={t.id} name={t.name} color={t.color} />
						))}
						{item.tags.length > 3 && <span>+{item.tags.length - 3}</span>}
						{item.usedAt && (
							<span className="flex shrink-0 items-center gap-1">
								<CheckCircleIcon className="size-3.5" /> Used
							</span>
						)}
						{item.notes.length > 0 && (
							<span className="ml-auto flex shrink-0 items-center gap-1">
								<NoteIcon className="size-3.5" />
								{item.notes.length > 9 ? "9+" : item.notes.length}{" "}
								{item.notes.length === 1 ? "note" : "notes"}
							</span>
						)}
					</div>
				)}
			</li>
			{dialog === "tags" && (
				<TagsDialog
					items={[item]}
					onClose={() => setDialog(null)}
					onChanged={refresh}
					onError={(message) => fail("Couldn't change tags")(message)}
				/>
			)}
			{dialog === "move" && (
				<MoveDialog
					items={[item]}
					onClose={() => setDialog(null)}
					onMoved={refresh}
					onError={(message) => fail("Couldn't move")(message)}
				/>
			)}
		</>
	);
}

type SortKey = "new" | "old" | "note" | "source";
const SORT_KEY = "chingolo.savedSort";
const SORT_ITEMS: Choice[] = [
	{ id: "new", name: "Newest first" },
	{ id: "old", name: "Oldest first" },
	{ id: "note", name: "Latest note" },
	{ id: "source", name: "Source A to Z" },
];

type GroupKey = "none" | "folder" | "tag" | "source" | "conversation";
const GROUP_KEY = "chingolo.savedGroup";
const GROUP_ITEMS: Choice[] = [
	{ id: "none", name: "No grouping" },
	{ id: "folder", name: "Group by folder" },
	{ id: "tag", name: "Group by tag" },
	{ id: "source", name: "Group by source" },
	{ id: "conversation", name: "Group by conversation" },
];
const NO_GROUP: Record<Exclude<GroupKey, "none">, string> = {
	folder: "No folder",
	tag: "Untagged",
	source: "No source",
	conversation: "No conversation",
};

const USED_ITEMS: Choice[] = [
	{ id: "any", name: "Used or not" },
	{ id: "used", name: "Used in my text" },
	{ id: "unused", name: "Not used yet" },
];

function readChoice<T extends string>(
	key: string,
	allowed: readonly Choice[],
	fallback: T,
): T {
	try {
		const v = localStorage.getItem(key);
		return allowed.some((a) => a.id === v) ? (v as T) : fallback;
	} catch {
		return fallback;
	}
}

function remember(key: string, value: string) {
	try {
		localStorage.setItem(key, value);
	} catch {
		// remembering the choice is a convenience only
	}
}

/** `list` arrives newest first, and ties keep that order. */
function sortItems(list: SavedItem[], sort: SortKey): SavedItem[] {
	if (sort === "old") return [...list].reverse();
	if (sort === "note") {
		const last = (i: SavedItem) =>
			Math.max(0, ...i.notes.map((n) => n.updatedAt));
		return [...list].sort((a, b) => last(b) - last(a));
	}
	if (sort === "source") {
		const name = (i: SavedItem) =>
			i.sources[0] ? `${i.sources[0].label} ${i.sources[0].title}` : null;
		return [...list].sort((a, b) => {
			const [x, y] = [name(a), name(b)];
			if (x === y) return 0;
			if (x === null) return 1;
			if (y === null) return -1;
			return x.localeCompare(y);
		});
	}
	return list;
}

type Section = { key: string; label: string | null; items: SavedItem[] };

/**
 * Splits the list into sections. An item with two tags or two sources is in both. Sections are
 * in alphabetical order and the one for items with none comes last.
 */
function sectionsFor(
	list: SavedItem[],
	group: GroupKey,
	chapterLabel: (id: string) => string | null,
): Section[] {
	if (group === "none") return [{ key: "all", label: null, items: list }];
	const belongs = (i: SavedItem): { key: string; label: string }[] => {
		switch (group) {
			case "folder": {
				const label = i.chapterId ? chapterLabel(i.chapterId) : null;
				return i.chapterId && label ? [{ key: i.chapterId, label }] : [];
			}
			case "tag":
				return i.tags.map((t) => ({ key: t.id, label: t.name }));
			case "source": {
				const docs = new Map<string, string>();
				for (const x of i.sources)
					docs.set(x.documentId, x.label ? `${x.label} · ${x.title}` : x.title);
				return [...docs].map(([key, label]) => ({ key, label }));
			}
			default:
				return i.conversationId || i.conversationTitle
					? [
							{
								key: conversationKey(i),
								label: i.conversationId
									? i.conversationTitle
									: `${i.conversationTitle} (deleted)`,
							},
						]
					: [];
		}
	};
	const byKey = new Map<string, Section>();
	const rest: SavedItem[] = [];
	for (const i of list) {
		const mine = belongs(i);
		if (mine.length === 0) rest.push(i);
		for (const m of mine) {
			const sec = byKey.get(m.key) ?? { key: m.key, label: m.label, items: [] };
			sec.items.push(i);
			byKey.set(m.key, sec);
		}
	}
	const sections = [...byKey.values()].sort((a, b) =>
		(a.label ?? "").localeCompare(b.label ?? ""),
	);
	if (rest.length > 0)
		sections.push({ key: "__none__", label: NO_GROUP[group], items: rest });
	return sections;
}

export function SavedView() {
	const { savedScope, setSavedScope } = useNav();
	const projects = useProjects();
	const chapters = useAllChapters();
	const tags = useSavedTags();
	const saved = useSaved(filterFor(savedScope));
	const [query, setQuery] = useState("");
	const [kind, setKind] = useState<KindFilter>("all");
	const [source, setSource] = useState(ALL);
	const [conversation, setConversation] = useState(ALL);
	const [folder, setFolder] = useState(ALL);
	const [tagFilter, setTagFilter] = useState<string[]>([]);
	const [used, setUsed] = useState<"any" | "used" | "unused">("any");
	const [copied, setCopied] = useState(false);
	const [dialog, setDialog] = useState<"tags" | "move" | null>(null);
	const { confirm } = useDialogs();
	const { refresh, fail } = useSavedRefresh();
	const [withNote, setWithNote] = useState(false);
	const [sort, setSort] = useState<SortKey>(() =>
		readChoice(SORT_KEY, SORT_ITEMS, "new"),
	);
	const [group, setGroup] = useState<GroupKey>(() =>
		readChoice(GROUP_KEY, GROUP_ITEMS, "none"),
	);
	const [mode, setMode] = useState<ViewMode>(readView);
	const pickMode = (m: ViewMode) => {
		setMode(m);
		remember(VIEW_KEY, m);
	};

	const items = saved.data ?? [];
	const counts = useMemo(() => {
		const c: Record<KindFilter, number> = {
			all: items.length,
			reply: 0,
			selection: 0,
			passage: 0,
		};
		for (const i of items) c[i.kind] += 1;
		return c;
	}, [items]);
	// Documents the saved items cite, for the source filter.
	const sources = useMemo<Choice[]>(() => {
		const byDoc = new Map<string, string>();
		for (const i of items)
			for (const x of i.sources)
				byDoc.set(x.documentId, x.label ? `${x.label} · ${x.title}` : x.title);
		return [...byDoc]
			.map(([id, name]) => ({ id, name }))
			.sort((a, b) => a.name.localeCompare(b.name));
	}, [items]);
	// A source that is not in this project's items falls back to all.
	const sourceKey = sources.some((x) => x.id === source) ? source : ALL;
	// Chats the items came from. A deleted chat is told apart by the title the item kept.
	const conversations = useMemo<Choice[]>(() => {
		const byKey = new Map<string, string>();
		for (const i of items) {
			if (!i.conversationId && !i.conversationTitle) continue;
			const key = conversationKey(i);
			if (!byKey.has(key))
				byKey.set(
					key,
					i.conversationId
						? i.conversationTitle
						: `${i.conversationTitle} (deleted)`,
				);
		}
		return [...byKey]
			.map(([id, name]) => ({ id, name }))
			.sort((a, b) => a.name.localeCompare(b.name));
	}, [items]);
	const conversationKeyChosen = conversations.some((x) => x.id === conversation)
		? conversation
		: ALL;
	// Folders can be picked inside one project, where their names are enough.
	const projectScope =
		typeof savedScope === "object" ? savedScope.projectId : null;
	const folders = useMemo<Choice[]>(
		() =>
			chapters
				.filter((c) => c.projectId === projectScope)
				.map((c) => ({ id: c.id, name: c.title })),
		[chapters, projectScope],
	);
	const folderKey =
		projectScope && (folder === NONE || folders.some((f) => f.id === folder))
			? folder
			: ALL;
	// Tags that were deleted drop out of the filter.
	const tagIds = useMemo(
		() => tagFilter.filter((id) => tags.data?.some((t) => t.id === id)),
		[tagFilter, tags.data],
	);
	const chapterLabel = useCallback(
		(id: string) => {
			const c = chapters.find((x) => x.id === id);
			if (!c) return null;
			const p = projects.data?.find((x) => x.id === c.projectId);
			return savedScope === "all" && p ? `${p.name} › ${c.title}` : c.title;
		},
		[chapters, projects.data, savedScope],
	);

	const shown = useMemo(() => {
		const q = query.trim().toLowerCase();
		const list = items.filter(
			(i) =>
				(kind === "all" || i.kind === kind) &&
				(sourceKey === ALL ||
					i.sources.some((x) => x.documentId === sourceKey)) &&
				(conversationKeyChosen === ALL ||
					conversationKey(i) === conversationKeyChosen) &&
				(folderKey === ALL ||
					(folderKey === NONE ? !i.chapterId : i.chapterId === folderKey)) &&
				(tagIds.length === 0 || i.tags.some((t) => tagIds.includes(t.id))) &&
				(used === "any" || (used === "used") === Boolean(i.usedAt)) &&
				(!withNote || i.notes.length > 0) &&
				(!q ||
					[
						i.body,
						...i.notes.map((n) => n.body),
						...i.tags.map((t) => t.name),
						i.conversationTitle,
						...i.sources.flatMap((x) => [x.title, x.label]),
					]
						.join("\n")
						.toLowerCase()
						.includes(q)),
		);
		return sortItems(list, sort);
	}, [
		items,
		query,
		kind,
		sourceKey,
		conversationKeyChosen,
		folderKey,
		tagIds,
		used,
		withNote,
		sort,
	]);
	const sections = useMemo(
		() => sectionsFor(shown, group, chapterLabel),
		[shown, group, chapterLabel],
	);

	const filtering =
		query.trim() !== "" ||
		kind !== "all" ||
		sourceKey !== ALL ||
		conversationKeyChosen !== ALL ||
		folderKey !== ALL ||
		tagIds.length > 0 ||
		used !== "any" ||
		withNote;
	const clear = () => {
		setQuery("");
		setKind("all");
		setSource(ALL);
		setConversation(ALL);
		setFolder(ALL);
		setTagFilter([]);
		setUsed("any");
		setWithNote(false);
	};

	const {
		selected,
		select,
		selectAll,
		clear: clearSelection,
	} = useSelection(
		// What is on screen, in order. An item in two sections is listed once.
		[...new Set(sections.flatMap((s) => s.items.map((i) => i.id)))],
		`${JSON.stringify(savedScope)}|${kind}|${sourceKey}|${conversationKeyChosen}|${folderKey}|${tagIds.join()}|${used}|${withNote}`,
	);
	// Only what is on screen counts, so a filter never leaves hidden items in an action.
	const chosen = useMemo(
		() => shown.filter((i) => selected.has(i.id)),
		[shown, selected],
	);
	const allUsed = chosen.length > 0 && chosen.every((i) => i.usedAt);

	const deleteChosen = async () => {
		const n = chosen.length;
		const ok = await confirm({
			title: n === 1 ? "Delete saved item?" : `Delete ${n} saved items?`,
			message: `${n === 1 ? "The item and its notes are" : "The items and their notes are"} removed. The conversations and the documents are not touched.`,
			confirmLabel: "Delete",
			danger: true,
		});
		if (!ok) return;
		try {
			await api.deleteSavedItems(chosen.map((i) => i.id));
		} catch (e) {
			fail("Couldn't delete")(e);
		}
		clearSelection();
		refresh();
	};

	// Replies are copied from what is drawn, as the card's own Copy does. The references of all
	// items go in one list, one line per document.
	const copyChosen = async () => {
		const parts = chosen.map((i) => {
			const prose =
				i.kind === "reply"
					? document.querySelector<HTMLElement>(
							`[data-saved-prose="${CSS.escape(i.id)}"]`,
						)
					: null;
			return prose
				? cleanCopy(prose)
				: { html: `<p>${esc(i.body)}</p>`, text: i.body };
		});
		const refs = referenceLines(chosen.flatMap((i) => i.sources));
		try {
			await writeRich({
				html:
					parts.map((p) => p.html).join("") +
					refs.map((r) => `<p>${esc(r)}</p>`).join(""),
				text: [...parts.map((p) => p.text.trim()), ...refs].join("\n\n"),
			});
			setCopied(true);
			setTimeout(() => setCopied(false), 1500);
		} catch {
			// clipboard can be unavailable; nothing useful to show
		}
	};

	const views: { id: ViewMode; label: string; icon: React.ReactNode }[] = [
		{ id: "list", label: "List", icon: <ListIcon /> },
		{ id: "large", label: "Large grid", icon: <SquaresFourIcon /> },
		{ id: "small", label: "Small grid", icon: <GridNineIcon /> },
	];

	const cards = (list: SavedItem[], prefix: string) => (
		<ul
			className="grid gap-3"
			style={{
				gridTemplateColumns: COLUMNS[mode],
				gridAutoRows: ROW_HEIGHT[mode],
			}}
		>
			{list.map((i) => (
				<SavedCard
					key={`${prefix}:${i.id}`}
					item={i}
					projects={projects.data ?? []}
					chapters={chapters}
					mode={mode}
					selected={selected.has(i.id)}
					selecting={chosen.length > 0}
					onSelect={(on, range) => select(i.id, on, range)}
				/>
			))}
		</ul>
	);

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center justify-between gap-3 px-6"
			>
				<div className="flex items-center gap-3">
					<h1 className="text-[15px] font-semibold">Saved</h1>
					<ScopeSelect value={savedScope} onChange={setSavedScope} />
				</div>
				<fieldset className="m-0 flex min-w-0 items-center gap-0.5 border-0 p-0">
					<legend className="sr-only">Layout</legend>
					{views.map((v) => (
						<IconButton
							key={v.id}
							label={v.label}
							aria-pressed={mode === v.id}
							onPress={() => pickMode(v.id)}
							className={cx(mode === v.id && "bg-border/60 text-foreground")}
						>
							{v.icon}
						</IconButton>
					))}
				</fieldset>
			</header>

			<div className="flex items-center gap-2 px-6 pb-3">
				<div className="relative min-w-0 flex-1">
					<MagnifyingGlassIcon className="pointer-events-none absolute left-2.5 top-2 size-4 text-muted-foreground" />
					<input
						type="search"
						aria-label="Search saved items"
						placeholder="Search text, notes, tags and sources"
						value={query}
						onChange={(e) => setQuery(e.target.value)}
						onKeyDown={(e) => e.key === "Escape" && setQuery("")}
						className="h-8 w-full rounded-lg border border-transparent bg-muted pl-8 pr-3 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50"
					/>
				</div>
				<FilterSelect
					label="Order"
					icon={<SortAscendingIcon className="size-4" />}
					value={sort}
					items={SORT_ITEMS}
					onChange={(k) => {
						setSort(k as SortKey);
						remember(SORT_KEY, k);
					}}
				/>
				<FilterSelect
					label="Group"
					icon={<RowsIcon className="size-4" />}
					value={group}
					items={GROUP_ITEMS}
					onChange={(k) => {
						setGroup(k as GroupKey);
						remember(GROUP_KEY, k);
					}}
				/>
			</div>

			<div className="flex flex-wrap items-center gap-2 px-6 pb-3">
				<TagFilter value={tagIds} onChange={setTagFilter} onChanged={refresh} />
				{folders.length > 0 && (
					<FilterSelect
						label="Folder"
						icon={<FolderIcon className="size-4" />}
						value={folderKey}
						items={[
							{ id: ALL, name: "Any folder" },
							{ id: NONE, name: "No folder" },
							...folders,
						]}
						onChange={setFolder}
					/>
				)}
				{sources.length > 0 && (
					<FilterSelect
						label="Source"
						value={sourceKey}
						items={[{ id: ALL, name: "Any source" }, ...sources]}
						onChange={setSource}
					/>
				)}
				{conversations.length > 0 && (
					<FilterSelect
						label="Conversation"
						value={conversationKeyChosen}
						items={[{ id: ALL, name: "Any conversation" }, ...conversations]}
						onChange={setConversation}
					/>
				)}
				<FilterSelect
					label="Used in my text"
					icon={<CheckCircleIcon className="size-4" />}
					value={used}
					items={USED_ITEMS}
					onChange={(k) => setUsed(k as typeof used)}
				/>
			</div>

			<div className="flex flex-wrap items-center gap-1.5 px-6 pb-3">
				{KIND_CHIPS.map((k) => (
					<Chip
						key={k.id}
						pressed={kind === k.id}
						onPress={() => setKind(k.id)}
					>
						{k.name} {counts[k.id]}
					</Chip>
				))}
				<Chip pressed={withNote} onPress={() => setWithNote((v) => !v)}>
					With notes
				</Chip>
				{filtering && (
					<RacButton
						onPress={clear}
						className={cx(
							"ml-1 rounded text-xs text-primary underline-offset-2 hover:underline",
							focus,
						)}
					>
						Clear filters
					</RacButton>
				)}
			</div>

			{saved.isSuccess && items.length > 0 && (
				<div className="flex h-10 shrink-0 items-center gap-3 border-y border-border px-6 text-xs text-muted-foreground">
					<RowCheckbox
						label="Select all shown items"
						isSelected={shown.length > 0 && chosen.length === shown.length}
						isIndeterminate={chosen.length > 0 && chosen.length < shown.length}
						onChange={selectAll}
					/>
					{chosen.length === 0 ? (
						<span>
							{shown.length === items.length
								? `${items.length} saved`
								: `${shown.length} of ${items.length}`}
						</span>
					) : (
						<>
							<span className="font-medium text-foreground">
								{chosen.length} selected
							</span>
							<div className="flex items-center gap-1">
								<Button onPress={() => setDialog("tags")}>
									<TagIcon className="size-4" /> Tags…
								</Button>
								<Button onPress={() => setDialog("move")}>
									<FolderSimpleIcon className="size-4" /> Move to…
								</Button>
								<Button
									onPress={() =>
										api
											.setSavedUsed(
												chosen.map((i) => i.id),
												!allUsed,
											)
											.then(refresh, fail("Couldn't change the mark"))
									}
								>
									<CheckCircleIcon className="size-4" />
									{allUsed ? "Not used" : "Mark used"}
								</Button>
								<Button onPress={copyChosen}>
									{copied ? (
										<CheckIcon className="size-4" />
									) : (
										<CopyIcon className="size-4" />
									)}
									{copied ? "Copied" : "Copy with references"}
								</Button>
								<Button variant="ghost" onPress={deleteChosen}>
									<TrashIcon className="size-4" /> Delete…
								</Button>
							</div>
							<RacButton
								onPress={clearSelection}
								className={cx(
									"rounded text-primary underline-offset-2 hover:underline",
									focus,
								)}
							>
								Clear
							</RacButton>
						</>
					)}
				</div>
			)}

			{/* relative: the checkboxes' hidden inputs are absolutely positioned and would otherwise
			    stretch the page's scroll area instead of this list's. */}
			<div className="relative min-h-0 flex-1 overflow-y-auto px-6 pb-6 pt-3">
				{saved.isPending && (
					<div className="flex justify-center py-10">
						<Spinner />
					</div>
				)}
				{saved.isError && (
					<p role="alert" className="text-sm text-problem">
						{saved.error.message}
					</p>
				)}
				{saved.isSuccess && items.length === 0 && (
					<div className="mx-auto max-w-sm py-16 text-center">
						<BookmarkSimpleIcon className="mx-auto mb-2 size-6 text-muted-foreground" />
						<p className="font-medium">Nothing saved yet</p>
						<p className="mt-1 text-sm text-muted-foreground">
							Use the bookmark under a reply to keep it here, or select text in
							a reply or a document and choose Save.
						</p>
					</div>
				)}
				{saved.isSuccess && items.length > 0 && shown.length === 0 && (
					<p className="py-2 text-sm text-muted-foreground">
						No saved items match.
					</p>
				)}
				{group === "none" ? (
					cards(shown, "all")
				) : (
					<div className="flex flex-col gap-6">
						{sections.map((sec) => (
							<section key={sec.key}>
								<h2 className="mb-2 flex items-baseline gap-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
									<span className="truncate">{sec.label}</span>
									<span className="font-normal">{sec.items.length}</span>
								</h2>
								{cards(sec.items, sec.key)}
							</section>
						))}
					</div>
				)}
			</div>

			{dialog === "tags" && (
				<TagsDialog
					items={chosen}
					onClose={() => setDialog(null)}
					onChanged={refresh}
					onError={(message) => fail("Couldn't change tags")(message)}
				/>
			)}
			{dialog === "move" && (
				<MoveDialog
					items={chosen}
					onClose={() => setDialog(null)}
					onMoved={() => {
						clearSelection();
						refresh();
					}}
					onError={(message) => fail("Couldn't move")(message)}
				/>
			)}
		</div>
	);
}
