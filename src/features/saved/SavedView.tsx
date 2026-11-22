import {
	BookmarkSimpleIcon,
	CaretDownIcon,
	CheckIcon,
	CopyIcon,
	EraserIcon,
	FolderMinusIcon,
	FolderSimpleIcon,
	GridNineIcon,
	ListIcon,
	MagnifyingGlassIcon,
	PencilSimpleIcon,
	SquaresFourIcon,
	TrashIcon,
} from "@phosphor-icons/react";
import { math } from "@streamdown/math";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import {
	ListBox,
	ListBoxItem,
	Popover,
	Button as RacButton,
	Select,
	SelectValue,
} from "react-aria-components";
import { Streamdown } from "streamdown";
import { type SavedScope, useNav } from "../../app/nav";
import { type Action, ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import {
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
import {
	CiteProvider,
	citeComponents,
	citePlugins,
	citeTags,
	SourcesList,
	savedAsCitations,
} from "../chat/citations";
import { cleanCopy, writeRich } from "../chat/copy";

const plugins = { math };
const ALL = "__all__";
const NONE = "__none__";

function filterFor(scope: SavedScope): SavedFilter {
	return scope === "all"
		? { kind: "all" }
		: scope === "none"
			? { kind: "unassigned" }
			: { kind: "project", id: scope.projectId };
}

type Choice = { id: string; name: string };

function FilterSelect({
	label,
	value,
	items,
	onChange,
}: {
	label: string;
	value: string;
	items: Choice[];
	onChange: (id: string) => void;
}) {
	return (
		<Select
			aria-label={label}
			selectedKey={value}
			onSelectionChange={(k) => k && onChange(String(k))}
		>
			<RacButton
				className={cx(
					"flex h-8 items-center gap-2 rounded-lg border border-input bg-card px-2.5 text-[13px] data-[hovered]:bg-muted",
					focus,
				)}
			>
				<SelectValue className="max-w-48 truncate" />
				<CaretDownIcon className="size-4 shrink-0 text-muted-foreground" />
			</RacButton>
			<Popover
				className={`max-h-72 min-w-(--trigger-width) overflow-auto p-1 ${POPOVER}`}
			>
				<ListBox items={items} className="outline-none">
					{(i) => (
						<ListBoxItem
							id={i.id}
							textValue={i.name}
							className={`${ITEM} truncate data-[selected]:font-medium`}
						>
							{i.name}
						</ListBoxItem>
					)}
				</ListBox>
			</Popover>
		</Select>
	);
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

function CopyItem({
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

const KIND_LABEL = {
	reply: "Reply",
	selection: "Selection",
	passage: "From a document",
} as const;

type ViewMode = "list" | "small" | "large";
const VIEW_KEY = "chingolo.savedView";
/** Grid cards share one height, so a long reply scrolls inside its card. */
const ROW_HEIGHT: Record<ViewMode, string> = {
	list: "auto",
	large: "24rem",
	small: "17rem",
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

/** How much of a long reply a card shows before it scrolls inside itself. */
const BODY_HEIGHT: Record<ViewMode, string> = {
	list: "",
	large: "min-h-0 flex-1 overflow-y-auto",
	small: "min-h-0 flex-1 overflow-y-auto",
};

function SavedCard({
	item,
	projects,
	mode,
}: {
	item: SavedItem;
	projects: Project[];
	mode: ViewMode;
}) {
	const qc = useQueryClient();
	const { select, savedScope } = useNav();
	const { confirm, alert } = useDialogs();
	const proseRef = useRef<HTMLDivElement>(null);
	const [note, setNote] = useState(item.note);
	const [editing, setEditing] = useState(false);
	const noteField = useRef<HTMLTextAreaElement>(null);
	useEffect(() => {
		if (editing) noteField.current?.focus();
	}, [editing]);
	const refresh = () => qc.invalidateQueries({ queryKey: keys.saved });
	const fail = (title: string) => (e: unknown) =>
		alert({
			title,
			message: e instanceof Error ? e.message : String(e),
		});
	// The note saves half a second after typing stops, and when the card goes away (another
	// view, another filter), so switching views never loses it.
	const latest = useRef({ note, saved: item.note });
	latest.current = { note, saved: item.note };
	const saveNote = useRef(() => {});
	saveNote.current = () => {
		if (latest.current.note.trim() === latest.current.saved) return;
		latest.current.saved = latest.current.note.trim();
		api
			.updateSavedNote(item.id, latest.current.note)
			.then(refresh, fail("Couldn't save the note"));
	};
	// biome-ignore lint/correctness/useExhaustiveDependencies: restart the timer on every edit
	useEffect(() => {
		const t = setTimeout(() => saveNote.current(), 500);
		return () => clearTimeout(t);
	}, [note]);
	useEffect(() => () => saveNote.current(), []);
	const citations = useMemo(
		() => savedAsCitations(item.sources),
		[item.sources],
	);
	const project = projects.find((p) => p.id === item.projectId);

	const actions: Action[] = [
		{
			id: "note",
			icon: <PencilSimpleIcon className="size-4" />,
			label: note.trim() ? "Edit note" : "Add note",
			run: () => setEditing(true),
		},
		...(note.trim()
			? [
					{
						id: "remove-note",
						icon: <EraserIcon className="size-4" />,
						label: "Remove note",
						run: () => {
							setNote("");
							setEditing(false);
						},
					},
				]
			: []),
		...projects
			.filter((p) => p.id !== item.projectId)
			.map((p) => ({
				id: `move:${p.id}`,
				icon: <FolderSimpleIcon className="size-4" />,
				label: `Move to ${p.name}`,
				run: () =>
					api.moveSaved(item.id, p.id).then(refresh, fail("Couldn't move")),
			})),
		...(item.projectId
			? [
					{
						id: "unassign",
						icon: <FolderMinusIcon className="size-4" />,
						label: "Remove from project",
						run: () =>
							api.moveSaved(item.id, null).then(refresh, fail("Couldn't move")),
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
						"The item and its note are removed. The conversation and the documents are not touched.",
					confirmLabel: "Delete",
					danger: true,
				});
				if (!ok) return;
				await api.deleteSaved(item.id).then(refresh, fail("Couldn't delete"));
			},
		},
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
		<li className="flex min-h-0 flex-col gap-2 rounded-xl border border-border bg-card px-4 py-3">
			<div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
				<span className="min-w-0 truncate">
					{KIND_LABEL[item.kind]} · {date}
					{savedScope === "all" && project ? ` · ${project.name}` : ""}
				</span>
				<span className="flex shrink-0 items-center gap-0.5">
					<CopyItem item={item} proseRef={proseRef} />
					<ActionMenu label="Saved item actions" actions={actions} />
				</span>
			</div>

			{item.kind === "reply" ? (
				<div
					ref={proseRef}
					className={cx(
						"selectable chat-prose",
						mode === "small" ? "text-[13px]" : "text-[15px]",
						BODY_HEIGHT[mode],
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
			) : (
				<blockquote
					className={cx(
						"selectable whitespace-pre-wrap border-l-2 border-primary/50 pl-3 leading-relaxed",
						mode === "small" ? "text-[13px]" : "text-[15px]",
						BODY_HEIGHT[mode],
					)}
				>
					{item.body}
				</blockquote>
			)}

			{item.sources.length > 0 && (
				<SourcesList
					citations={citations}
					conversationId={item.conversationId ?? ""}
					savedItemId={item.id}
				/>
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

			{editing ? (
				<textarea
					ref={noteField}
					aria-label="Note"
					placeholder="Write a note"
					rows={3}
					value={note}
					onChange={(e) => setNote(e.target.value)}
					onBlur={() => {
						saveNote.current();
						if (!note.trim()) setEditing(false);
					}}
					onKeyDown={(e) => e.key === "Escape" && e.currentTarget.blur()}
					className="shrink-0 resize-none rounded-lg border border-transparent bg-muted px-3 py-1.5 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50"
				/>
			) : (
				note.trim() && (
					<p
						className={cx(
							"shrink-0 whitespace-pre-wrap rounded-lg bg-muted px-3 py-1.5 text-[13px]",
							mode === "list" ? "" : "line-clamp-3",
						)}
					>
						{note.trim()}
					</p>
				)
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

	const shown = useMemo(() => {
		const q = query.trim().toLowerCase();
		const list = items.filter(
			(i) =>
				(kind === "all" || i.kind === kind) &&
				(sourceKey === ALL ||
					i.sources.some((x) => x.documentId === sourceKey)) &&
				(!withNote || i.note.trim() !== "") &&
				(!q ||
					[
						i.body,
						i.note,
						i.conversationTitle,
						...i.sources.flatMap((x) => [x.title, x.label]),
					]
						.join("\n")
						.toLowerCase()
						.includes(q)),
		);
		return oldestFirst ? [...list].reverse() : list;
	}, [items, query, kind, sourceKey, withNote, oldestFirst]);

	const filtering =
		query.trim() !== "" || kind !== "all" || sourceKey !== ALL || withNote;
	const clear = () => {
		setQuery("");
		setKind("all");
		setSource(ALL);
		setWithNote(false);
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
				{saved.isSuccess && items.length > 0 && (
					<span className="ml-auto text-xs text-muted-foreground">
						{shown.length === items.length
							? `${items.length} saved`
							: `${shown.length} of ${items.length}`}
					</span>
				)}
			</div>

			<div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6">
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
						/>
					))}
				</ul>
			</div>
		</div>
	);
}
