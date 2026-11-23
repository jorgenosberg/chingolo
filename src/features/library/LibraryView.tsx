import {
	ArrowClockwiseIcon,
	BookOpenIcon,
	CaretDownIcon,
	FileIcon,
	FilePlusIcon,
	FolderIcon,
	FolderMinusIcon,
	FolderPlusIcon,
	MagnifyingGlassIcon,
	PencilSimpleIcon,
	ProhibitIcon,
	TrashIcon,
	WarningIcon,
} from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { useDeferredValue, useEffect, useMemo, useState } from "react";
import {
	ListBox,
	ListBoxItem,
	Menu,
	MenuItem,
	MenuTrigger,
	Popover,
	Button as RacButton,
	Select,
	SelectValue,
} from "react-aria-components";
import { useNav } from "../../app/nav";
import { type Action, ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
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

function DocumentRow({
	doc,
	actions,
	onOpen,
	onEdit,
	onKeep,
}: {
	doc: Document;
	actions: Action[];
	onEdit: () => void;
	/** Accepts the blanks as they are. */
	onKeep: () => void;
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
		<li className="flex items-start gap-3 border-b border-border px-6 py-3">
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
				{byline && (
					<p className="truncate text-xs text-muted-foreground">{byline}</p>
				)}
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
	onChange,
}: {
	value: string | null;
	onChange: (scope: string | null) => void;
}) {
	const projects = useProjects();
	const items = [
		{ id: ALL, name: "All documents" },
		...(projects.data ?? []).map((p) => ({ id: p.id, name: p.name })),
	];
	return (
		<Select
			aria-label="Show documents from"
			selectedKey={value ?? ALL}
			onSelectionChange={(k) => k && onChange(k === ALL ? null : String(k))}
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
	const [projectsFor, setProjectsFor] = useState<string | null>(null);

	const scopeName = projects.data?.find((p) => p.id === scope)?.name ?? null;
	const refresh = () => qc.invalidateQueries({ queryKey: keys.documents });
	const fail = (title: string) => (e: unknown) =>
		alert({ title, message: errorText(e) });

	const detailsIds = useDetailsIds();
	const [attentionOnly, setAttentionOnly] = useState(false);
	const needing = useMemo(
		() =>
			(docs.data ?? []).filter(
				(d) => !detailsIds.has(d.id) && attentionFor(d).length > 0,
			),
		[docs.data, detailsIds],
	);
	const shown = useMemo(() => {
		const all =
			attentionOnly && needing.length > 0 ? needing : (docs.data ?? []);
		return query.trim() ? all.filter((d) => matches(d, query)) : all;
	}, [docs.data, needing, attentionOnly, query]);
	const jobs = summarizeJobs(useJobs());
	const working = jobs.state === "busy" ? jobs.text : null;
	const projectsDoc = docs.data?.find((d) => d.id === projectsFor) ?? null;

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
				run: () => setProjectsFor(d.id),
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
					<ScopeSelect value={scope} onChange={setLibraryScope} />
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

			{needing.length > 0 && (
				<div className="flex items-center gap-2 px-6 pb-2 text-xs text-muted-foreground">
					<WarningIcon className="size-3.5 text-problem" />
					<span>
						{plural(needing.length, "document")} need
						{needing.length === 1 ? "s" : ""} a look.
					</span>
					<RacButton
						onPress={() => setAttentionOnly((v) => !v)}
						className={cx(
							"rounded text-primary underline-offset-2 hover:underline",
							focus,
						)}
					>
						{attentionOnly ? "Show all" : "Show them"}
					</RacButton>
				</div>
			)}
			{summary && <SummaryBanner summary={summary} scopeName={scopeName} />}
			<EmbeddingNotice />

			<div className="min-h-0 flex-1 overflow-y-auto">
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
						<ul>
							{shown.map((d) => (
								<DocumentRow
									key={d.id}
									doc={d}
									actions={actionsFor(d)}
									onEdit={() => setEditing(d)}
									onKeep={() => keepBlank(d)}
									onOpen={d.kind === "pdf" ? () => open(d) : undefined}
								/>
							))}
						</ul>
						{searching && shown.length === 0 && (
							<p className="px-6 py-2 text-sm text-muted-foreground">
								No titles or authors match.
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
							{textHits.data?.map((h) => (
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
			{projectsDoc && (
				<ProjectsDialog
					doc={projectsDoc}
					onClose={() => setProjectsFor(null)}
					onError={(message) => alert({ title: "Couldn't update", message })}
				/>
			)}
		</div>
	);
}
