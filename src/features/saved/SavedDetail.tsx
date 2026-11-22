import {
	ArrowLeftIcon,
	ArrowRightIcon,
	ChatCircleTextIcon,
} from "@phosphor-icons/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button as RacButton } from "react-aria-components";
import { Streamdown } from "streamdown";
import { useNav } from "../../app/nav";
import { ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import { Button, cx, focus, Spinner } from "../../components/ui";
import { api } from "../../lib/api";
import type { Project, SavedItem } from "../../lib/bindings";
import { useProjects, useSaved } from "../../lib/queries";
import {
	CiteProvider,
	citeComponents,
	citePlugins,
	citeTags,
	savedAsCitations,
	targetOf,
	where,
} from "../chat/citations";
import {
	CopyItem,
	itemActions,
	KIND_LABEL,
	plugins,
	useSavedRefresh,
} from "./SavedView";

/** One saved item on its own page: the full text, the note, every source and where it came from. */
export function SavedDetail({
	itemId,
	compose,
}: {
	itemId: string;
	compose: boolean;
}) {
	const { openSaved } = useNav();
	const saved = useSaved({ kind: "all" });
	const projects = useProjects();
	const item = saved.data?.find((i) => i.id === itemId);

	if (!item) {
		return (
			<div className="flex h-full flex-col">
				<Header onBack={() => openSaved()} />
				<div className="flex flex-1 items-start justify-center py-16">
					{saved.isPending ? (
						<Spinner />
					) : (
						<p className="text-sm text-muted-foreground">
							This item is no longer saved.
						</p>
					)}
				</div>
			</div>
		);
	}
	return (
		<Detail
			key={item.id}
			item={item}
			projects={projects.data ?? []}
			compose={compose}
		/>
	);
}

function Header({
	onBack,
	children,
}: {
	onBack: () => void;
	children?: React.ReactNode;
}) {
	return (
		<header
			data-tauri-drag-region
			className="flex h-12 shrink-0 items-center justify-between gap-3 px-6"
		>
			<Button variant="ghost" className="-ml-2 px-2" onPress={onBack}>
				<ArrowLeftIcon className="size-[17px]" />
				Saved
			</Button>
			<div className="flex items-center gap-0.5">{children}</div>
		</header>
	);
}

function Section({
	title,
	children,
}: {
	title: string;
	children: React.ReactNode;
}) {
	return (
		<section className="flex flex-col gap-2">
			<h2 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
				{title}
			</h2>
			{children}
		</section>
	);
}

/** Text written but not posted yet, kept per item while the app runs so leaving the page is safe. */
const drafts = new Map<string, string>();

/** "Nov 23, 12:21". */
const stamp = (ms: number) =>
	new Date(ms).toLocaleString(undefined, {
		month: "short",
		day: "numeric",
		hour: "2-digit",
		minute: "2-digit",
	});

const bubble =
	"w-full resize-y rounded-2xl border border-primary/40 bg-muted px-4 py-2.5 text-[14px] leading-relaxed outline-none placeholder:text-muted-foreground";

/** The item's notes, laid out like comments under a post. New ones go at the bottom. */
function NoteThread({ item, compose }: { item: SavedItem; compose: boolean }) {
	const { refresh, fail } = useSavedRefresh();
	const { confirm } = useDialogs();
	const [draft, setDraftText] = useState(drafts.get(item.id) ?? "");
	const [editing, setEditing] = useState<{ id: string; text: string } | null>(
		null,
	);
	const [busy, setBusy] = useState(false);
	const box = useRef<HTMLTextAreaElement>(null);
	useEffect(() => {
		if (compose) box.current?.focus();
	}, [compose]);
	const setDraft = (v: string) => {
		setDraftText(v);
		if (v) drafts.set(item.id, v);
		else drafts.delete(item.id);
	};

	const run = async (what: string, fn: () => Promise<unknown>) => {
		setBusy(true);
		try {
			await fn();
			refresh();
			return true;
		} catch (e) {
			fail(what)(e);
			return false;
		} finally {
			setBusy(false);
		}
	};
	const post = async () => {
		if (!draft.trim() || busy) return;
		if (
			await run("Couldn't add the note", () => api.addSavedNote(item.id, draft))
		)
			setDraft("");
	};
	const saveEdit = async () => {
		if (!editing?.text.trim() || busy) return;
		if (
			await run("Couldn't save the note", () =>
				api.updateSavedNote(editing.id, editing.text),
			)
		)
			setEditing(null);
	};
	const textButton = cx(
		"rounded text-xs text-muted-foreground underline-offset-2 data-[hovered]:text-foreground data-[hovered]:underline",
		focus,
	);

	return (
		<Section
			title={item.notes.length > 0 ? `Notes (${item.notes.length})` : "Notes"}
		>
			<ul className="flex flex-col gap-4">
				{item.notes.map((n) => (
					<li key={n.id} className="flex items-start gap-3">
						<div className="min-w-0 flex-1">
							{editing?.id === n.id ? (
								<div className="flex flex-col gap-2">
									<textarea
										aria-label="Edit note"
										rows={3}
										value={editing.text}
										onChange={(e) =>
											setEditing({ id: n.id, text: e.target.value })
										}
										onKeyDown={(e) => {
											if (e.key === "Escape") setEditing(null);
											else if (e.key === "Enter" && (e.metaKey || e.ctrlKey))
												saveEdit();
										}}
										// biome-ignore lint/a11y/noAutofocus: opened by pressing Edit
										autoFocus
										className={bubble}
									/>
									<div className="flex justify-end gap-2">
										<Button variant="ghost" onPress={() => setEditing(null)}>
											Cancel
										</Button>
										<Button
											variant="primary"
											isDisabled={!editing.text.trim() || busy}
											onPress={saveEdit}
										>
											Save
										</Button>
									</div>
								</div>
							) : (
								<>
									<p className="whitespace-pre-wrap rounded-2xl bg-muted px-4 py-2.5 text-[14px] leading-relaxed">
										{n.body}
									</p>
									<div className="mt-1 flex items-center gap-3 px-2">
										<time
											dateTime={new Date(n.createdAt).toISOString()}
											title={new Date(n.createdAt).toLocaleString()}
											className="text-xs tabular-nums text-muted-foreground"
										>
											{stamp(n.createdAt)}
											{n.updatedAt - n.createdAt > 1000 ? " · edited" : ""}
										</time>
										<RacButton
											className={textButton}
											onPress={() => setEditing({ id: n.id, text: n.body })}
										>
											Edit
										</RacButton>
										<RacButton
											className={textButton}
											onPress={async () => {
												const ok = await confirm({
													title: "Delete note?",
													message: "This note is removed from the saved item.",
													confirmLabel: "Delete",
													danger: true,
												});
												if (ok)
													run("Couldn't delete the note", () =>
														api.deleteSavedNote(n.id),
													);
											}}
										>
											Delete
										</RacButton>
									</div>
								</>
							)}
						</div>
					</li>
				))}

				<li className="flex items-start gap-3">
					<div className="flex min-w-0 flex-1 flex-col gap-2">
						<textarea
							ref={box}
							aria-label="New note"
							placeholder="Add a note…"
							rows={draft ? 3 : 1}
							value={draft}
							onChange={(e) => setDraft(e.target.value)}
							onKeyDown={(e) => {
								if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) post();
							}}
							className={cx(
								bubble,
								"min-h-10 resize-none border-transparent focus:border-primary/40",
								!draft && "rounded-full",
							)}
						/>
						{draft.trim() && (
							<div className="flex items-center justify-end gap-3">
								<span className="text-xs text-muted-foreground">
									⌘↵ to post
								</span>
								<Button variant="primary" isDisabled={busy} onPress={post}>
									Post
								</Button>
							</div>
						)}
					</div>
				</li>
			</ul>
		</Section>
	);
}

function Detail({
	item,
	projects,
	compose,
}: {
	item: SavedItem;
	projects: Project[];
	compose: boolean;
}) {
	const { openSaved, select, openCitation } = useNav();
	const { confirm } = useDialogs();
	const proseRef = useRef<HTMLDivElement>(null);
	const { refresh, fail } = useSavedRefresh();
	const citations = useMemo(
		() => savedAsCitations(item.sources),
		[item.sources],
	);
	const project = projects.find((p) => p.id === item.projectId);

	return (
		<div className="flex h-full flex-col">
			<Header onBack={() => openSaved()}>
				<CopyItem item={item} proseRef={proseRef} />
				<ActionMenu
					label="Saved item actions"
					actions={itemActions({
						item,
						projects,
						refresh,
						fail,
						confirm,
						onDeleted: () => openSaved(),
					})}
				/>
			</Header>

			<div className="@container min-h-0 flex-1 overflow-y-auto px-6 pb-10">
				<article className="mx-auto grid max-w-6xl gap-8 @3xl:grid-cols-[minmax(0,1fr)_minmax(18rem,24rem)]">
					<div className="flex min-w-0 max-w-3xl flex-col gap-6">
						<p className="text-xs text-muted-foreground">
							{KIND_LABEL[item.kind]} ·{" "}
							{new Date(item.createdAt).toLocaleString()}
							{project ? ` · ${project.name}` : ""}
						</p>

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
							<blockquote className="selectable whitespace-pre-wrap border-l-2 border-primary/50 pl-4 text-[15px] leading-relaxed">
								{item.body}
							</blockquote>
						)}
						<NoteThread item={item} compose={compose} />
					</div>
					<aside className="flex min-w-0 flex-col gap-6">
						{item.conversationTitle &&
							(item.conversationId && item.messageId ? (
								<RacButton
									onPress={() =>
										select(item.conversationId, {
											messageId: item.messageId ?? undefined,
										})
									}
									className={cx(
										"flex items-center gap-3 rounded-xl border border-primary/40 bg-accent-soft px-4 py-3 text-left transition-colors data-[hovered]:border-primary",
										focus,
									)}
								>
									<ChatCircleTextIcon className="size-5 shrink-0 text-primary" />
									<span className="min-w-0 flex-1">
										<span className="block text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
											From conversation
										</span>
										<span className="block truncate text-[13px] font-medium">
											{item.conversationTitle}
										</span>
										<span className="block text-xs text-muted-foreground">
											Open at this message
										</span>
									</span>
									<ArrowRightIcon className="size-4 shrink-0 text-primary" />
								</RacButton>
							) : (
								<div className="flex items-center gap-3 rounded-xl border border-border bg-muted px-4 py-3">
									<ChatCircleTextIcon className="size-5 shrink-0 text-muted-foreground" />
									<span className="min-w-0 flex-1">
										<span className="block text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
											From conversation
										</span>
										<span className="block truncate text-[13px] font-medium">
											{item.conversationTitle}
										</span>
										<span className="block text-xs text-muted-foreground">
											The conversation was deleted
										</span>
									</span>
								</div>
							))}

						{item.sources.length > 0 && (
							<Section title={item.sources.length === 1 ? "Source" : "Sources"}>
								<ul className="flex flex-col gap-3">
									{citations.map((c, i) => (
										<li
											key={c.handle}
											className="flex flex-col gap-1.5 rounded-xl border border-border bg-card px-4 py-3"
										>
											<div className="flex items-start justify-between gap-3">
												<div className="min-w-0 text-[13px]">
													<p className="font-medium">{where(c)}</p>
													{item.sources[i].doi && (
														<p className="truncate text-xs text-muted-foreground">
															doi:{item.sources[i].doi}
														</p>
													)}
												</div>
												<Button
													className="shrink-0"
													onPress={() =>
														openCitation(
															targetOf(c, item.conversationId ?? "", item.id),
														)
													}
												>
													Open document
												</Button>
											</div>
											{/* A passage item's source text is the item's own text. */}
											{item.kind !== "passage" && (
												<blockquote className="selectable line-clamp-5 border-l-2 border-border pl-3 text-[13px] leading-relaxed text-muted-foreground">
													{c.snippet}
												</blockquote>
											)}
										</li>
									))}
								</ul>
							</Section>
						)}
					</aside>
				</article>
			</div>
		</div>
	);
}
