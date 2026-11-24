import {
	ArrowClockwiseIcon,
	BookOpenIcon,
	CheckIcon,
	FileIcon,
	FilePlusIcon,
	FolderIcon,
	FolderMinusIcon,
	FolderPlusIcon,
	MagnifyingGlassIcon,
	MinusIcon,
	PencilSimpleIcon,
	ProhibitIcon,
	TrashIcon,
	WarningIcon,
} from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import {
	Checkbox,
	Menu,
	MenuItem,
	MenuTrigger,
	Popover,
	Button as RacButton,
} from "react-aria-components";
import { useNav } from "../../app/nav";
import { type Action, ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import { FilterSelect } from "../../components/FilterSelect";
import { Button, cx, focus, ITEM, POPOVER, Spinner } from "../../components/ui";
import { api } from "../../lib/api";
import type {
	DocStatus,
	Document,
	ImportSummary,
	MetadataPatch,
} from "../../lib/bindings";
import {
	summarizeJobs,
	useDetailsBusy,
	useDetailsIds,
	useJobs,
} from "../../lib/jobs";
import {
	attentionFor,
	missingFields,
	setImportSummary,
	useEmbeddingStatus,
	useImportSummary,
	useProgress,
} from "../../lib/library";
import {
	keys,
	useDocuments,
	useLibrarySearch,
	useProjects,
} from "../../lib/queries";
import { EmbeddingNotice } from "./EmbeddingNotice";
import { MetadataDialog } from "./MetadataDialog";
import { ProjectsDialog } from "./ProjectsDialog";

const ALL = "__all__";
const NONE = "__none__";
const SORT_KEY = "chingolo.librarySort";

type TypeFilter = "all" | "pdf" | "docx";
type StatusFilter = "all" | "attention" | "failed" | "scan" | "reading";
type Sort = "new" | "old" | "title" | "author" | "year";

const TYPE_ITEMS = [
	{ id: "all", name: "Any type" },
	{ id: "pdf", name: "PDF" },
	{ id: "docx", name: "Word" },
];
const STATUS_ITEMS = [
	{ id: "all", name: "Any status" },
	{ id: "attention", name: "Needs a look" },
	{ id: "failed", name: "Failed" },
	{ id: "scan", name: "No text found" },
	{ id: "reading", name: "Reading" },
];
const SORT_ITEMS = [
	{ id: "new", name: "Newest added" },
	{ id: "old", name: "Oldest added" },
	{ id: "title", name: "Title A to Z" },
	{ id: "author", name: "Author A to Z" },
	{ id: "year", name: "Year, newest first" },
];

function readSort(): Sort {
	try {
		const v = localStorage.getItem(SORT_KEY);
		return SORT_ITEMS.some((i) => i.id === v) ? (v as Sort) : "new";
	} catch {
		return "new";
	}
}

const collator = new Intl.Collator(undefined, { sensitivity: "base" });

/** Empty values go last. */
function byText(a: string, b: string) {
	if (!a !== !b) return a ? -1 : 1;
	return collator.compare(a, b);
}

function sortDocs(list: Document[], sort: Sort) {
	const by: Record<Sort, (a: Document, b: Document) => number> = {
		new: (a, b) => b.createdAt - a.createdAt,
		old: (a, b) => a.createdAt - b.createdAt,
		title: (a, b) =>
			byText(a.title || a.originalName, b.title || b.originalName),
		author: (a, b) => byText(a.authors[0] ?? "", b.authors[0] ?? ""),
		year: (a, b) => (b.year ?? -1) - (a.year ?? -1),
	};
	return [...list].sort(by[sort]);
}
/** How long typing must pause before meaning search runs (it can cost an API call). */
const SETTLE_MS = 500;

const STATUS_LABEL: Record<DocStatus, string> = {
	queued: "Waiting",
	processing: "Reading",
	ready: "Ready",
	no_text: "No text found",
	error: "Failed",
	cancelled: "Cancelled",
};

const STATUS_STYLE: Record<DocStatus, string> = {
	queued: "text-muted-foreground",
	processing: "text-primary",
	ready: "text-muted-foreground",
	no_text: "text-muted-foreground",
	error: "text-problem",
	cancelled: "text-muted-foreground",
};

function matches(d: Document, q: string) {
	const hay = [
		d.title,
		d.originalName,
		d.authors.join(" "),
		d.doi,
		d.year ? String(d.year) : "",
	]
		.join(" ")
		.toLowerCase();
	return q
		.toLowerCase()
		.split(/\s+/)
		.filter(Boolean)
		.every((t) => hay.includes(t));
}

function errorText(e: unknown) {
	return e instanceof Error ? e.message : String(e);
}

function plural(n: number, one: string, many = `${one}s`) {
	return `${n} ${n === 1 ? one : many}`;
}

function SummaryBanner({
	summary,
	scopeName,
}: {
	summary: ImportSummary;
	scopeName: string | null;
}) {
	const { added, duplicates, skipped } = summary;
	const parts: string[] = [];
	if (added.length) parts.push(`${plural(added.length, "file")} added.`);
	if (duplicates.length) {
		const where = scopeName ? ` (now in ${scopeName})` : "";
		parts.push(
			`${plural(duplicates.length, "file was", "files were")} already in the library${where}.`,
		);
	}
	if (!parts.length && !skipped.length) parts.push("Nothing to import.");
	return (
		<div
			role="status"
			className="mx-6 mb-3 flex items-start justify-between gap-3 rounded-lg border border-border bg-muted px-3 py-2 text-[13px]"
		>
			<div className="min-w-0">
				{parts.length > 0 && <p>{parts.join(" ")}</p>}
				{skipped.length > 0 && (
					<div className="text-muted-foreground">
						<p>{plural(skipped.length, "file")} skipped:</p>
						<ul className="ml-4 list-disc">
							{skipped.slice(0, 5).map((s) => (
								<li key={`${s.name}:${s.reason}`} className="truncate">
									{s.name}: {s.reason}
								</li>
							))}
							{skipped.length > 5 && <li>and {skipped.length - 5} more</li>}
						</ul>
					</div>
				)}
			</div>
			<RacButton
				onPress={() => setImportSummary(null)}
				className={cx("shrink-0 text-xs text-muted-foreground", focus)}
			>
				Dismiss
			</RacButton>
		</div>
	);
}

/** The original file name, cut in the middle so the extension stays visible. */
function FileLink({ name, onPress }: { name: string; onPress: () => void }) {
	const dot = name.lastIndexOf(".");
	const [base, ext] =
		dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
	return (
		<RacButton
			onPress={onPress}
			aria-label={`Show ${name} in Finder`}
			className={cx(
				"flex min-w-0 shrink items-baseline rounded text-left data-[hovered]:text-foreground data-[hovered]:underline",
				focus,
			)}
		>
			<span className="truncate">{base}</span>
			<span className="shrink-0">{ext}</span>
		</RacButton>
	);
}

function RowCheckbox({
	label,
	isSelected,
	isIndeterminate,
	onChange,
}: {
	label: string;
	isSelected: boolean;
	isIndeterminate?: boolean;
	onChange: (on: boolean) => void;
}) {
	return (
		<Checkbox
			aria-label={label}
			isSelected={isSelected}
			isIndeterminate={isIndeterminate}
			onChange={onChange}
			className={cx("group flex cursor-pointer items-center", focus)}
		>
			<span className="relative flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[indeterminate]:border-primary group-data-[selected]:border-primary group-data-[indeterminate]:bg-primary group-data-[selected]:bg-primary">
				<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
				<MinusIcon className="absolute size-4 opacity-0 group-data-[indeterminate]:opacity-100" />
			</span>
		</Checkbox>
	);
}

function DocumentRow({
	doc,
	actions,
	onOpen,
	onEdit,
	onKeep,
	onReveal,
	projectNames,
	selected,
	onSelect,
}: {
	doc: Document;
	projectNames: string[];
	selected: boolean;
	/** `range` is set when shift was held. */
	onSelect: (on: boolean, range: boolean) => void;
	actions: Action[];
	onEdit: () => void;
	/** Accepts the blanks as they are. */
	onKeep: () => void;
	/** Shows Chingolo's copy of the file in Finder. */
	onReveal: () => void;
	/** Set when the document can be opened in the reader. */
	onOpen?: () => void;
}) {
	const progress = useProgress(doc.id);
	const detailsBusy = useDetailsBusy(doc.id);
	// While a lookup runs it may still fill the gaps, so wait for it.
	const attention = detailsBusy ? [] : attentionFor(doc);
	const embedding = useEmbeddingStatus();
	const indexing =
		doc.status === "ready" &&
		embedding?.model.kind === "ready" &&
		doc.embeddedCount < doc.passageCount;
	const shift = useRef(false);
	const pct = progress?.total ? (progress.done / progress.total) * 100 : 0;
	const byline = [
		doc.authors.length > 2
			? `${doc.authors[0]} et al.`
			: doc.authors.join(", "),
		doc.year ? String(doc.year) : "",
	]
		.filter(Boolean)
		.join(" · ");

	return (
		<li
			className={cx(
				"flex items-start gap-3 border-b border-border px-6 py-3",
				selected && "bg-muted/60",
			)}
		>
			<div
				className="pt-0.5"
				onPointerDownCapture={(e) => {
					shift.current = e.shiftKey;
				}}
				onKeyDownCapture={(e) => {
					shift.current = e.shiftKey;
				}}
			>
				<RowCheckbox
					label={`Select ${doc.title || doc.originalName}`}
					isSelected={selected}
					onChange={(on) => onSelect(on, shift.current)}
				/>
			</div>
			<div className="min-w-0 flex-1">
				<div className="flex items-baseline gap-2">
					<RacButton
						onPress={onOpen}
						isDisabled={!onOpen}
						className={cx(
							"truncate text-left text-sm data-[disabled]:cursor-default",
							onOpen && "data-[hovered]:underline",
							doc.title ? "font-medium" : "text-muted-foreground",
							focus,
						)}
					>
						{doc.title || doc.originalName}
					</RacButton>
					{!doc.title && (
						<span className="shrink-0 text-xs text-muted-foreground">
							No title found
						</span>
					)}
				</div>
				{(byline || doc.title) && (
					<p className="flex min-w-0 items-baseline gap-1.5 text-xs text-muted-foreground">
						{byline && (
							<span className="min-w-0 shrink-[3] truncate">{byline}</span>
						)}
						{byline && doc.title && <span aria-hidden>·</span>}
						{doc.title && (
							<FileLink name={doc.originalName} onPress={onReveal} />
						)}
					</p>
				)}
				<p className="mt-0.5 flex flex-wrap gap-1 text-xs text-muted-foreground">
					{projectNames.length === 0 ? (
						<span>No project</span>
					) : (
						projectNames.map((n) => (
							<span key={n} className="rounded bg-muted px-1.5 py-0.5">
								{n}
							</span>
						))
					)}
				</p>
				{indexing && (
					<p className="text-xs text-muted-foreground">
						Indexing for meaning search: {doc.embeddedCount} of{" "}
						{doc.passageCount}
					</p>
				)}
				{doc.status === "processing" && (
					<div className="mt-1.5 flex items-center gap-2">
						{progress && progress.total > 0 ? (
							<>
								<div
									role="progressbar"
									aria-label={`Reading ${doc.originalName}`}
									aria-valuemin={0}
									aria-valuemax={progress.total}
									aria-valuenow={progress.done}
									className="h-1 w-40 overflow-hidden rounded-full bg-border"
								>
									<div
										className="h-full bg-primary transition-[width]"
										style={{ width: `${pct}%` }}
									/>
								</div>
								<span className="text-xs text-muted-foreground">
									Page {progress.done} of {progress.total}
								</span>
							</>
						) : (
							<Spinner className="size-3" />
						)}
					</div>
				)}
				{detailsBusy && doc.status !== "processing" && (
					<p className="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground">
						<Spinner className="size-3" /> Updating details
					</p>
				)}
				{attention.length > 0 && (
					<p className="mt-1 flex flex-wrap items-center gap-x-1.5 text-xs text-foreground/80">
						<WarningIcon className="size-3.5 shrink-0 text-problem" />
						{attention.join(" · ")}
						<RacButton
							onPress={onEdit}
							className={cx(
								"rounded text-primary underline-offset-2 hover:underline",
								focus,
							)}
						>
							Edit details
						</RacButton>
						{missingFields(doc).length > 0 && (
							<RacButton
								onPress={onKeep}
								className={cx(
									"rounded text-muted-foreground underline-offset-2 hover:underline",
									focus,
								)}
							>
								Leave blank
							</RacButton>
						)}
					</p>
				)}
				{doc.status === "no_text" && (
					<p className="mt-1 text-xs text-muted-foreground">
						This looks like a scanned document. Chingolo can't read scans yet,
						so it can't search or cite it.
					</p>
				)}
				{doc.error && (
					<p
						className={cx(
							"mt-1 text-xs",
							doc.status === "error" ? "text-problem" : "text-muted-foreground",
						)}
					>
						{doc.error}
					</p>
				)}
			</div>
			<div className="flex shrink-0 items-center gap-3">
				<span className={cx("text-xs", STATUS_STYLE[doc.status])}>
					{STATUS_LABEL[doc.status]}
					{doc.status === "ready" && doc.pageCount
						? ` · ${plural(doc.pageCount, "page")}`
						: ""}
				</span>
				<ActionMenu
					label={`Actions for ${doc.title || doc.originalName}`}
					actions={actions}
				/>
			</div>
		</li>
	);
}

function ImportMenu({
	onPick,
	onRefreshAll,
	isDisabled,
}: {
	onPick: (folder: boolean) => void;
	onRefreshAll: () => void;
	isDisabled: boolean;
}) {
	return (
		<MenuTrigger>
			<Button variant="primary" isDisabled={isDisabled}>
				<FilePlusIcon className="size-[17px]" /> Import
			</Button>
			<Popover placement="bottom end" className={`min-w-40 p-1 ${POPOVER}`}>
				<Menu
					aria-label="Import"
					onAction={(k) =>
						k === "refresh" ? onRefreshAll() : onPick(k === "folder")
					}
					className="outline-none"
				>
					<MenuItem id="files" className={`${ITEM} flex items-center gap-2`}>
						<FileIcon className="size-4" />
						Files…
					</MenuItem>
					<MenuItem id="folder" className={`${ITEM} flex items-center gap-2`}>
						<FolderPlusIcon className="size-4" />
						Folder…
					</MenuItem>
					<MenuItem id="refresh" className={`${ITEM} flex items-center gap-2`}>
						<ArrowClockwiseIcon className="size-4" />
						Refresh all details
					</MenuItem>
				</Menu>
			</Popover>
		</MenuTrigger>
	);
}

function ScopeSelect({
	value,
	noProject,
	onChange,
}: {
	value: string | null;
	noProject: boolean;
	onChange: (scope: string | null, noProject: boolean) => void;
}) {
	const projects = useProjects();
	return (
		<FilterSelect
			label="Show documents from"
			value={noProject ? NONE : (value ?? ALL)}
			items={[
				{ id: ALL, name: "All documents" },
				{ id: NONE, name: "Not in any project" },
				...(projects.data ?? []).map((p) => ({ id: p.id, name: p.name })),
			]}
			onChange={(k) => onChange(k === ALL || k === NONE ? null : k, k === NONE)}
		/>
	);
}

export function LibraryView() {
	const qc = useQueryClient();
	const {
		libraryScope: scope,
		setLibraryScope,
		openReader,
		openActivity,
	} = useNav();
	const { confirm, alert } = useDialogs();
	const projects = useProjects();
	const docs = useDocuments(scope);
	const summary = useImportSummary();
	const [query, setQuery] = useState("");
	const deferred = useDeferredValue(query);
	// Meaning search can cost an API call, so it waits until typing has paused (or Enter).
	const [settled, setSettled] = useState("");
	useEffect(() => {
		const t = setTimeout(() => setSettled(query), SETTLE_MS);
		return () => clearTimeout(t);
	}, [query]);
	const embedding = useEmbeddingStatus();
	const meaningReady = embedding?.model.kind === "ready";
	const keywordHits = useLibrarySearch(deferred, scope, false);
	const meaningHits = useLibrarySearch(settled, scope, true, meaningReady);
	const meaningCurrent =
		meaningReady && settled.trim() === query.trim() && meaningHits.isSuccess;
	// Keyword hits show at once and are replaced when the merged ones arrive.
	const textHits = meaningCurrent ? meaningHits : keywordHits;
	const lookingByMeaning =
		meaningReady &&
		query.trim().length >= 3 &&
		!meaningCurrent &&
		keywordHits.isSuccess;
	const [importing, setImporting] = useState(false);
	const [editing, setEditing] = useState<Document | null>(null);
	const [projectsFor, setProjectsFor] = useState<string[] | null>(null);
	const [noProjectPick, setNoProject] = useState(false);
	const noProject = noProjectPick && scope === null;
	const [typeFilter, setTypeFilter] = useState<TypeFilter>("all");
	const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
	const [sort, setSort] = useState<Sort>(readSort);
	const pickSort = (v: Sort) => {
		setSort(v);
		try {
			localStorage.setItem(SORT_KEY, v);
		} catch {
			// remembering the order is a convenience only
		}
	};
	const [selected, setSelected] = useState<Set<string>>(new Set());
	const anchor = useRef<string | null>(null);

	const scopeName = projects.data?.find((p) => p.id === scope)?.name ?? null;
	const refresh = () => qc.invalidateQueries({ queryKey: keys.documents });
	const fail = (title: string) => (e: unknown) =>
		alert({ title, message: errorText(e) });

	const detailsIds = useDetailsIds();

	const needing = useMemo(
		() =>
			(docs.data ?? []).filter(
				(d) => !detailsIds.has(d.id) && attentionFor(d).length > 0,
			),
		[docs.data, detailsIds],
	);
	const needingIds = useMemo(
		() => new Set(needing.map((d) => d.id)),
		[needing],
	);
	// Everything except the text query, which also narrows the passage hits below.
	const base = useMemo(
		() =>
			(docs.data ?? []).filter(
				(d) =>
					(!noProject || d.projectIds.length === 0) &&
					(typeFilter === "all" || d.kind === typeFilter) &&
					(statusFilter === "all" ||
						(statusFilter === "attention" && needingIds.has(d.id)) ||
						(statusFilter === "failed" &&
							(d.status === "error" || d.status === "cancelled")) ||
						(statusFilter === "scan" && d.status === "no_text") ||
						(statusFilter === "reading" &&
							(d.status === "queued" || d.status === "processing"))),
			),
		[docs.data, noProject, typeFilter, statusFilter, needingIds],
	);
	const shown = useMemo(
		() =>
			sortDocs(
				query.trim() ? base.filter((d) => matches(d, query)) : base,
				sort,
			),
		[base, query, sort],
	);
	const filtering = noProject || typeFilter !== "all" || statusFilter !== "all";
	const baseIds = useMemo(() => new Set(base.map((d) => d.id)), [base]);
	const clearFilters = () => {
		setNoProject(false);
		setTypeFilter("all");
		setStatusFilter("all");
	};
	const projectName = useMemo(
		() => new Map((projects.data ?? []).map((p) => [p.id, p.name])),
		[projects.data],
	);

	// A selection belongs to the list it was made in.
	// biome-ignore lint/correctness/useExhaustiveDependencies: clears when the list's filters change
	useEffect(() => {
		setSelected(new Set());
		anchor.current = null;
	}, [scope, noProject, typeFilter, statusFilter]);
	const chosen = useMemo(
		() => shown.filter((d) => selected.has(d.id)),
		[shown, selected],
	);
	const clearSelection = () => {
		setSelected(new Set());
		anchor.current = null;
	};
	useEffect(() => {
		if (selected.size === 0) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key === "Escape" && !(e.target instanceof HTMLInputElement)) {
				setSelected(new Set());
				anchor.current = null;
			}
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [selected.size]);
	const select = (id: string, on: boolean, range: boolean) => {
		setSelected((prev) => {
			const next = new Set(prev);
			const from = anchor.current
				? shown.findIndex((d) => d.id === anchor.current)
				: -1;
			const to = shown.findIndex((d) => d.id === id);
			const ids =
				range && from >= 0 && to >= 0
					? shown
							.slice(Math.min(from, to), Math.max(from, to) + 1)
							.map((d) => d.id)
					: [id];
			for (const i of ids) {
				if (on) next.add(i);
				else next.delete(i);
			}
			return next;
		});
		anchor.current = id;
	};
	const selectAll = (on: boolean) =>
		setSelected(on ? new Set(shown.map((d) => d.id)) : new Set());
	const jobs = summarizeJobs(useJobs());
	const working = jobs.state === "busy" ? jobs.text : null;
	const projectsDocs = (docs.data ?? []).filter((d) =>
		projectsFor?.includes(d.id),
	);
	const refreshable = chosen.filter(
		(d) => d.status === "ready" || d.status === "no_text",
	);

	const refreshChosen = () =>
		Promise.all(refreshable.map((d) => api.refreshDetails(d.id))).catch(
			fail("Couldn't refresh the details"),
		);

	const deleteChosen = async () => {
		const n = chosen.length;
		const ok = await confirm({
			title: n === 1 ? "Delete document?" : `Delete ${n} documents?`,
			message: `${n === 1 ? "It is" : "They are"} removed from the library, along with Chingolo's copy and the search index. Your original files are not touched.`,
			confirmLabel: "Delete",
			danger: true,
		});
		if (!ok) return;
		try {
			const failed = await api.deleteDocuments(chosen.map((d) => d.id));
			if (failed > 0)
				await alert({
					title: "Couldn't delete everything",
					message: `${plural(failed, "document")} could not be deleted.`,
				});
		} catch (e) {
			await fail("Couldn't delete")(e);
		}
		clearSelection();
		refresh();
		qc.invalidateQueries({ queryKey: ["library-search"] });
	};

	const pick = async (folder: boolean) => {
		setImporting(true);
		try {
			setImportSummary(await api.pickAndImport(folder, scope));
			refresh();
		} catch (e) {
			await fail("Import failed")(e);
		} finally {
			setImporting(false);
		}
	};

	// Saying a blank is right marks it as the user's, so a refresh won't ask again.
	const keepBlank = (d: Document) => {
		const patch: Partial<MetadataPatch> = {};
		for (const f of missingFields(d)) {
			if (f === "title") patch.title = "";
			else if (f === "authors") patch.authors = [];
			else patch.year = 0;
		}
		api
			.updateDocumentMetadata(d.id, patch)
			.then(refresh)
			.catch(fail("Couldn't save"));
	};

	const refreshAll = () =>
		api.refreshAllDetails().catch(fail("Couldn't refresh the details"));

	const open = (d: Document) =>
		openReader({ documentId: d.id, page: 1, passageId: null });

	const actionsFor = (d: Document): Action[] => {
		const name = d.title || d.originalName;
		const list: Action[] = [
			...(d.kind === "pdf"
				? [
						{
							id: "open",
							icon: <BookOpenIcon className="size-4" />,
							label: "Open",
							run: () => open(d),
						},
					]
				: []),
			{
				id: "edit",
				icon: <PencilSimpleIcon className="size-4" />,
				label: "Edit details…",
				run: () => setEditing(d),
			},
			{
				id: "projects",
				icon: <FolderIcon className="size-4" />,
				label: "Projects…",
				run: () => setProjectsFor([d.id]),
			},
		];
		if (d.status === "queued" || d.status === "processing") {
			list.push({
				id: "cancel",
				icon: <ProhibitIcon className="size-4" />,
				label: "Cancel reading",
				run: () => api.cancelImport(d.id),
			});
		}
		if (
			d.status === "error" ||
			d.status === "cancelled" ||
			d.status === "ready" ||
			d.status === "no_text"
		) {
			const again = d.status === "ready" || d.status === "no_text";
			list.push({
				id: "retry",
				icon: <ArrowClockwiseIcon className="size-4" />,
				label: again ? "Read again" : "Try again",
				run: () => api.retryImport(d.id).catch(fail("Couldn't retry")),
			});
		}
		if (d.status === "ready" || d.status === "no_text") {
			list.push({
				id: "refresh-details",
				icon: <ArrowClockwiseIcon className="size-4" />,
				label: "Refresh details",
				run: () =>
					api.refreshDetails(d.id).catch(fail("Couldn't refresh the details")),
			});
		}
		if (scope) {
			list.push({
				id: "detach",
				icon: <FolderMinusIcon className="size-4" />,
				label: scopeName ? `Remove from ${scopeName}` : "Remove from project",
				run: async () => {
					await api.detachDocument(scope, d.id).catch(fail("Couldn't remove"));
					refresh();
				},
			});
		}
		list.push({
			id: "delete",
			icon: <TrashIcon className="size-4" />,
			label: "Delete…",
			danger: true,
			run: async () => {
				const ok = await confirm({
					title: "Delete document?",
					message: `"${name}" is removed from the library, along with Chingolo's copy and its search index. Your original file is not touched.`,
					confirmLabel: "Delete",
					danger: true,
				});
				if (!ok) return;
				await api.deleteDocument(d.id).catch(fail("Couldn't delete"));
				refresh();
				qc.invalidateQueries({ queryKey: ["library-search"] });
			},
		});
		return list;
	};

	const searching = query.trim().length > 0;

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center justify-between gap-3 px-6"
			>
				<div className="flex items-center gap-3">
					<h1 className="text-[15px] font-semibold">Library</h1>
					<ScopeSelect
						value={scope}
						noProject={noProject}
						onChange={(s, none) => {
							setLibraryScope(s);
							setNoProject(none);
						}}
					/>
				</div>
				<div className="flex items-center gap-3">
					{working && (
						<RacButton
							onPress={openActivity}
							aria-label={`${working}. Open activity`}
							className={cx(
								"flex items-center gap-1.5 rounded-md px-1.5 py-1 text-xs text-muted-foreground data-[hovered]:bg-muted data-[hovered]:text-foreground",
								focus,
							)}
						>
							<Spinner decorative className="size-3" /> {working}
						</RacButton>
					)}
					<ImportMenu
						onPick={pick}
						onRefreshAll={refreshAll}
						isDisabled={importing}
					/>
				</div>
			</header>

			<div className="px-6 pb-3">
				<div className="relative">
					<MagnifyingGlassIcon className="pointer-events-none absolute left-2.5 top-2 size-4 text-muted-foreground" />
					<input
						type="search"
						aria-label="Search documents"
						placeholder="Search titles, authors and text"
						value={query}
						onChange={(e) => setQuery(e.target.value)}
						onKeyDown={(e) => {
							if (e.key === "Escape") {
								setQuery("");
								setSettled("");
							} else if (e.key === "Enter") {
								setSettled(query);
							}
						}}
						className="h-8 w-full rounded-lg border border-transparent bg-muted pl-8 pr-3 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50"
					/>
				</div>
			</div>

			<div className="flex flex-wrap items-center gap-2 px-6 pb-3">
				<FilterSelect
					label="File type"
					value={typeFilter}
					items={TYPE_ITEMS}
					onChange={(k) => setTypeFilter(k as TypeFilter)}
				/>
				<FilterSelect
					label="Status"
					value={statusFilter}
					items={STATUS_ITEMS}
					onChange={(k) => setStatusFilter(k as StatusFilter)}
				/>
				<FilterSelect
					label="Sort by"
					value={sort}
					items={SORT_ITEMS}
					onChange={(k) => pickSort(k as Sort)}
				/>
				{filtering && (
					<RacButton
						onPress={clearFilters}
						className={cx(
							"rounded text-xs text-primary underline-offset-2 hover:underline",
							focus,
						)}
					>
						Clear filters
					</RacButton>
				)}
			</div>

			{needing.length > 0 && (
				<div className="flex items-center gap-2 px-6 pb-2 text-xs text-muted-foreground">
					<WarningIcon className="size-3.5 text-problem" />
					<span>
						{plural(needing.length, "document")} need
						{needing.length === 1 ? "s" : ""} a look.
					</span>
					<RacButton
						onPress={() =>
							setStatusFilter((v) => (v === "attention" ? "all" : "attention"))
						}
						className={cx(
							"rounded text-primary underline-offset-2 hover:underline",
							focus,
						)}
					>
						{statusFilter === "attention" ? "Show all" : "Show them"}
					</RacButton>
				</div>
			)}
			{summary && <SummaryBanner summary={summary} scopeName={scopeName} />}
			<EmbeddingNotice />

			{/* relative: the checkboxes' hidden inputs are absolutely positioned and would otherwise
			    stretch the page's scroll area instead of this list's. */}
			<div className="relative min-h-0 flex-1 overflow-y-auto">
				{docs.isPending && (
					<div className="flex justify-center py-10">
						<Spinner />
					</div>
				)}
				{docs.isError && (
					<p role="alert" className="px-6 text-sm text-problem">
						{docs.error.message}
					</p>
				)}
				{docs.isSuccess && docs.data.length === 0 && (
					<div className="mx-auto max-w-sm px-6 py-16 text-center">
						<p className="font-medium">
							{scopeName ? `No documents in ${scopeName}` : "No documents yet"}
						</p>
						<p className="mt-1 text-sm text-muted-foreground">
							Drop PDF or Word files, or a whole folder, anywhere on this
							window. You can also use Import.
							{scopeName ? ` They will be added to ${scopeName}.` : ""}
						</p>
					</div>
				)}
				{docs.isSuccess && docs.data.length > 0 && (
					<>
						{searching && (
							<h2 className="px-6 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
								Documents
							</h2>
						)}
						<div className="flex h-12 items-center gap-3 border-b border-border px-6 text-xs text-muted-foreground">
							<RowCheckbox
								label="Select all shown documents"
								isSelected={shown.length > 0 && chosen.length === shown.length}
								isIndeterminate={
									chosen.length > 0 && chosen.length < shown.length
								}
								onChange={selectAll}
							/>
							{chosen.length === 0 ? (
								<span>{plural(shown.length, "document")}</span>
							) : (
								<>
									<span className="font-medium text-foreground">
										{chosen.length} selected
									</span>
									<div className="flex items-center gap-1">
										<Button
											onPress={() => setProjectsFor(chosen.map((d) => d.id))}
										>
											<FolderIcon className="size-4" /> Projects…
										</Button>
										<Button
											onPress={refreshChosen}
											isDisabled={refreshable.length === 0}
										>
											<ArrowClockwiseIcon className="size-4" /> Refresh details
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
						<ul>
							{shown.map((d) => (
								<DocumentRow
									key={d.id}
									doc={d}
									projectNames={d.projectIds.flatMap(
										(id) => projectName.get(id) ?? [],
									)}
									selected={selected.has(d.id)}
									onSelect={(on, range) => select(d.id, on, range)}
									actions={actionsFor(d)}
									onEdit={() => setEditing(d)}
									onKeep={() => keepBlank(d)}
									onReveal={() =>
										api
											.revealDocumentFile(d.id)
											.catch(fail("Couldn't show the file"))
									}
									onOpen={d.kind === "pdf" ? () => open(d) : undefined}
								/>
							))}
						</ul>
						{(searching || filtering) && shown.length === 0 && (
							<p className="px-6 py-2 text-sm text-muted-foreground">
								{filtering
									? "No documents match the filters."
									: "No titles or authors match."}
							</p>
						)}
					</>
				)}

				{searching && (
					<section className="pb-6">
						<h2 className="px-6 pb-1 pt-4 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
							In the text
							{lookingByMeaning && (
								<span className="ml-2 font-normal normal-case tracking-normal">
									Looking for related passages…
								</span>
							)}
						</h2>
						{textHits.isPending && (
							<p className="px-6 text-sm text-muted-foreground">Searching…</p>
						)}
						{textHits.isError && (
							<p role="alert" className="px-6 text-sm text-problem">
								{textHits.error.message}
							</p>
						)}
						{textHits.isSuccess && textHits.data.length === 0 && (
							<p className="px-6 text-sm text-muted-foreground">
								No passages match. Only documents marked Ready are searched.
							</p>
						)}
						<ul>
							{textHits.data
								?.filter((h) => !filtering || baseIds.has(h.documentId))
								.map((h) => (
									<li key={h.passageId} className="border-b border-border">
										<RacButton
											onPress={() =>
												openReader({
													documentId: h.documentId,
													page: h.pageStart,
													passageId: h.passageId,
												})
											}
											className={cx(
												"block w-full px-6 py-3 text-left data-[hovered]:bg-muted",
												focus,
											)}
										>
											<p className="text-[13px] font-medium">
												{h.documentTitle}
												<span className="ml-2 font-normal text-muted-foreground">
													{h.pageStart === h.pageEnd
														? `Page ${h.pageStart}`
														: `Pages ${h.pageStart} to ${h.pageEnd}`}
												</span>
												{h.matchedBy === "meaning" && (
													<span className="ml-2 rounded bg-muted px-1.5 py-0.5 text-[11px] font-normal text-muted-foreground">
														Related by meaning
													</span>
												)}
											</p>
											<p className="mt-0.5 line-clamp-3 text-xs text-muted-foreground">
												{h.text}
											</p>
										</RacButton>
									</li>
								))}
						</ul>
					</section>
				)}
			</div>

			{editing && (
				<MetadataDialog
					key={editing.id}
					doc={editing}
					onClose={() => setEditing(null)}
					onSaved={refresh}
				/>
			)}
			{projectsDocs.length > 0 && (
				<ProjectsDialog
					docs={projectsDocs}
					onClose={() => setProjectsFor(null)}
					onError={(message) => alert({ title: "Couldn't update", message })}
				/>
			)}
		</div>
	);
}
