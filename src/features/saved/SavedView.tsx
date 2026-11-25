import {
	ArrowsOutSimpleIcon,
	BookmarkSimpleIcon,
	CheckIcon,
	CopyIcon,
	FolderMinusIcon,
	FolderSimpleIcon,
	GridNineIcon,
	ListIcon,
	MagnifyingGlassIcon,
	NoteIcon,
	PencilSimpleIcon,
	SquaresFourIcon,
	TrashIcon,
} from "@phosphor-icons/react";
import { math } from "@streamdown/math";
import { useQueryClient } from "@tanstack/react-query";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import {
	Menu,
	MenuItem,
	MenuTrigger,
	Popover,
	Button as RacButton,
} from "react-aria-components";
import { Streamdown } from "streamdown";
import { type SavedScope, useNav } from "../../app/nav";
import { type Action, ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import { type Choice, FilterSelect } from "../../components/FilterSelect";
import { RowCheckbox } from "../../components/RowCheckbox";
import {
	Button,
	cx,
	focus,
	IconButton,
	ITEM,
	POPOVER,
	Spinner,
} from "../../components/ui";
import { api } from "../../lib/api";
import type {
	Project,
	SavedFilter,
	SavedItem,
	SavedSource,
} from "../../lib/bindings";
import { keys, useProjects, useSaved } from "../../lib/queries";
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
	const refresh = () => qc.invalidateQueries({ queryKey: keys.saved });
	const fail = (title: string) => (e: unknown) =>
		alert({
			title,
			message: e instanceof Error ? e.message : String(e),
		});
	return { refresh, fail };
}

/** Move to a project, take out of its project, delete. Shared by the card and the detail page. */
export function itemActions({
	item,
	projects,
	refresh,
	fail,
	confirm,
	onDeleted,
}: {
	item: SavedItem;
	projects: Project[];
	refresh: () => void;
	fail: (title: string) => (e: unknown) => void;
	confirm: ReturnType<typeof useDialogs>["confirm"];
	onDeleted?: () => void;
}): Action[] {
	return [
		...projects
			.filter((p) => p.id !== item.projectId)
			.map((p) => ({
				id: `move:${p.id}`,
				icon: <FolderSimpleIcon className="size-4" />,
				label: `Move to ${p.name}`,
				run: () =>
					api
						.moveSavedItems([item.id], p.id)
						.then(refresh, fail("Couldn't move")),
			})),
		...(item.projectId
			? [
					{
						id: "unassign",
						icon: <FolderMinusIcon className="size-4" />,
						label: "Remove from project",
						run: () =>
							api
								.moveSavedItems([item.id], null)
								.then(refresh, fail("Couldn't move")),
					},
				]
			: []),
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
	mode,
	selected,
	selecting,
	onSelect,
}: {
	item: SavedItem;
	projects: Project[];
	mode: ViewMode;
	selected: boolean;
	/** Something is selected, so every card shows its checkbox. */
	selecting: boolean;
	/** `range` is set when shift was held. */
	onSelect: (on: boolean, range: boolean) => void;
}) {
	const shift = useRef(false);
	const { select, savedScope, openSavedItem } = useNav();
	const { confirm } = useDialogs();
	const proseRef = useRef<HTMLDivElement>(null);
	const { refresh, fail } = useSavedRefresh();
	const citations = useMemo(
		() => savedAsCitations(item.sources),
		[item.sources],
	);
	const project = projects.find((p) => p.id === item.projectId);
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
		...itemActions({ item, projects, refresh, fail, confirm }),
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
		// biome-ignore lint/a11y/useKeyWithClickEvents: the Open button is the keyboard route
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
					/>
				</div>
				<span className="min-w-0 flex-1 truncate">
					{KIND_LABEL[item.kind]} · {date}
					{savedScope === "all" && project ? ` · ${project.name}` : ""}
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

			{item.notes.length > 0 && (
				<p className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
					<NoteIcon className="size-3.5" />
					{item.notes.length > 9 ? "9+" : item.notes.length}{" "}
					{item.notes.length === 1 ? "note" : "notes"}
				</p>
			)}
		</li>
	);
}

export function SavedView() {
	const { savedScope, setSavedScope } = useNav();
	const projects = useProjects();
	const saved = useSaved(filterFor(savedScope));
	const [query, setQuery] = useState("");
	const [kind, setKind] = useState<KindFilter>("all");
	const [source, setSource] = useState(ALL);
	const [conversation, setConversation] = useState(ALL);
	const [copied, setCopied] = useState(false);
	const { confirm } = useDialogs();
	const { refresh, fail } = useSavedRefresh();
	const [withNote, setWithNote] = useState(false);
	const [oldestFirst, setOldestFirst] = useState(false);
	const [mode, setMode] = useState<ViewMode>(readView);
	const pickMode = (m: ViewMode) => {
		setMode(m);
		try {
			localStorage.setItem(VIEW_KEY, m);
		} catch {
			// remembering the layout is a convenience only
		}
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

	const shown = useMemo(() => {
		const q = query.trim().toLowerCase();
		const list = items.filter(
			(i) =>
				(kind === "all" || i.kind === kind) &&
				(sourceKey === ALL ||
					i.sources.some((x) => x.documentId === sourceKey)) &&
				(conversationKeyChosen === ALL ||
					conversationKey(i) === conversationKeyChosen) &&
				(!withNote || i.notes.length > 0) &&
				(!q ||
					[
						i.body,
						...i.notes.map((n) => n.body),
						i.conversationTitle,
						...i.sources.flatMap((x) => [x.title, x.label]),
					]
						.join("\n")
						.toLowerCase()
						.includes(q)),
		);
		return oldestFirst ? [...list].reverse() : list;
	}, [
		items,
		query,
		kind,
		sourceKey,
		conversationKeyChosen,
		withNote,
		oldestFirst,
	]);

	const filtering =
		query.trim() !== "" ||
		kind !== "all" ||
		sourceKey !== ALL ||
		conversationKeyChosen !== ALL ||
		withNote;
	const clear = () => {
		setQuery("");
		setKind("all");
		setSource(ALL);
		setConversation(ALL);
		setWithNote(false);
	};

	const {
		selected,
		select,
		selectAll,
		clear: clearSelection,
	} = useSelection(
		shown.map((i) => i.id),
		`${JSON.stringify(savedScope)}|${kind}|${sourceKey}|${conversationKeyChosen}|${withNote}`,
	);
	// Only what is on screen counts, so a filter never leaves hidden items in an action.
	const chosen = useMemo(
		() => shown.filter((i) => selected.has(i.id)),
		[shown, selected],
	);

	const moveChosen = (projectId: string | null) =>
		api
			.moveSavedItems(
				chosen.map((i) => i.id),
				projectId,
			)
			.then(() => {
				clearSelection();
				refresh();
			}, fail("Couldn't move"));

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
						placeholder="Search text, notes and sources"
						value={query}
						onChange={(e) => setQuery(e.target.value)}
						onKeyDown={(e) => e.key === "Escape" && setQuery("")}
						className="h-8 w-full rounded-lg border border-transparent bg-muted pl-8 pr-3 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50"
					/>
				</div>
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
					label="Order"
					value={oldestFirst ? "old" : "new"}
					items={[
						{ id: "new", name: "Newest first" },
						{ id: "old", name: "Oldest first" },
					]}
					onChange={(k) => setOldestFirst(k === "old")}
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
								<MenuTrigger>
									<Button>
										<FolderSimpleIcon className="size-4" /> Move to…
									</Button>
									<Popover
										placement="bottom start"
										className={`min-w-44 p-1 ${POPOVER}`}
									>
										<Menu
											aria-label="Move to project"
											onAction={(k) =>
												moveChosen(k === NONE ? null : String(k))
											}
											className="outline-none"
										>
											<MenuItem
												id={NONE}
												className={cx(ITEM, "flex items-center gap-2")}
											>
												<FolderMinusIcon className="size-4" />
												No project
											</MenuItem>
											{(projects.data ?? []).map((p) => (
												<MenuItem
													key={p.id}
													id={p.id}
													className={cx(ITEM, "flex items-center gap-2")}
												>
													<FolderSimpleIcon className="size-4" />
													{p.name}
												</MenuItem>
											))}
										</Menu>
									</Popover>
								</MenuTrigger>
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
				<ul
					className="grid gap-3"
					style={{
						gridTemplateColumns: COLUMNS[mode],
						gridAutoRows: ROW_HEIGHT[mode],
					}}
				>
					{shown.map((i) => (
						<SavedCard
							key={i.id}
							item={i}
							projects={projects.data ?? []}
							mode={mode}
							selected={selected.has(i.id)}
							selecting={chosen.length > 0}
							onSelect={(on, range) => select(i.id, on, range)}
						/>
					))}
				</ul>
			</div>
		</div>
	);
}
