import {
	BookmarkSimpleIcon,
	CaretDownIcon,
	CheckIcon,
	CopyIcon,
	FolderMinusIcon,
	FolderSimpleIcon,
	MagnifyingGlassIcon,
	TrashIcon,
} from "@phosphor-icons/react";
import { math } from "@streamdown/math";
import { useQueryClient } from "@tanstack/react-query";
import { useMemo, useRef, useState } from "react";
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

function ScopeSelect({
	value,
	onChange,
}: {
	value: SavedScope;
	onChange: (scope: SavedScope) => void;
}) {
	const projects = useProjects();
	const items = [
		{ id: ALL, name: "All saved" },
		{ id: NONE, name: "No project" },
		...(projects.data ?? []).map((p) => ({ id: p.id, name: p.name })),
	];
	const key = value === "all" ? ALL : value === "none" ? NONE : value.projectId;
	return (
		<Select
			aria-label="Show saved items from"
			selectedKey={key}
			onSelectionChange={(k) => {
				if (!k) return;
				onChange(
					k === ALL ? "all" : k === NONE ? "none" : { projectId: String(k) },
				);
			}}
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

function SavedCard({
	item,
	projects,
}: {
	item: SavedItem;
	projects: Project[];
}) {
	const qc = useQueryClient();
	const { select, savedScope } = useNav();
	const { confirm, alert } = useDialogs();
	const proseRef = useRef<HTMLDivElement>(null);
	const [note, setNote] = useState(item.note);
	const refresh = () => qc.invalidateQueries({ queryKey: keys.saved });
	const fail = (title: string) => (e: unknown) =>
		alert({
			title,
			message: e instanceof Error ? e.message : String(e),
		});
	const citations = useMemo(
		() => savedAsCitations(item.sources),
		[item.sources],
	);
	const project = projects.find((p) => p.id === item.projectId);

	const actions: Action[] = [
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
		<li className="flex flex-col gap-2 rounded-xl border border-border bg-card px-4 py-3">
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
				<div ref={proseRef} className="selectable chat-prose text-[15px]">
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
				<blockquote className="selectable whitespace-pre-wrap border-l-2 border-primary/50 pl-3 text-[15px] leading-relaxed">
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

			<textarea
				aria-label="Note"
				placeholder="Add a note"
				rows={note ? 2 : 1}
				value={note}
				onChange={(e) => setNote(e.target.value)}
				onBlur={() => {
					if (note.trim() === item.note) return;
					api
						.updateSavedNote(item.id, note)
						.then(refresh, fail("Couldn't save"));
				}}
				className="resize-none rounded-lg border border-transparent bg-muted px-3 py-1.5 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50"
			/>
		</li>
	);
}

export function SavedView() {
	const { savedScope, setSavedScope } = useNav();
	const projects = useProjects();
	const saved = useSaved(filterFor(savedScope));
	const [query, setQuery] = useState("");

	const shown = useMemo(() => {
		const q = query.trim().toLowerCase();
		if (!q) return saved.data ?? [];
		return (saved.data ?? []).filter((i) =>
			[
				i.body,
				i.note,
				i.conversationTitle,
				...i.sources.flatMap((s) => [s.title, s.label]),
			]
				.join("\n")
				.toLowerCase()
				.includes(q),
		);
	}, [saved.data, query]);

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center gap-3 px-6"
			>
				<h1 className="text-[15px] font-semibold">Saved</h1>
				<ScopeSelect value={savedScope} onChange={setSavedScope} />
			</header>

			<div className="px-6 pb-3">
				<div className="relative">
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
				{saved.isSuccess && saved.data.length === 0 && (
					<div className="mx-auto max-w-sm py-16 text-center">
						<BookmarkSimpleIcon className="mx-auto mb-2 size-6 text-muted-foreground" />
						<p className="font-medium">Nothing saved yet</p>
						<p className="mt-1 text-sm text-muted-foreground">
							Use the bookmark under a reply to keep it here, with the sources
							it cites.
						</p>
					</div>
				)}
				{saved.isSuccess && saved.data.length > 0 && shown.length === 0 && (
					<p className="py-2 text-sm text-muted-foreground">
						No saved items match.
					</p>
				)}
				<ul className="mx-auto flex max-w-3xl flex-col gap-3">
					{shown.map((i) => (
						<SavedCard key={i.id} item={i} projects={projects.data ?? []} />
					))}
				</ul>
			</div>
		</div>
	);
}
