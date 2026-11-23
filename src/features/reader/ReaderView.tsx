import { ArrowLeftIcon, XIcon } from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo } from "react";
import { type ReaderTarget, useNav } from "../../app/nav";
import { Button, Note, Spinner } from "../../components/ui";
import { api } from "../../lib/api";
import {
	keys,
	useConversations,
	useDocuments,
	usePassage,
	useSource,
} from "../../lib/queries";
import { type Highlight, PdfReader } from "./PdfReader";

const NO_BOXES: Highlight["boxes"] = [];

/**
 * A document with an optional passage highlighted. Fills the main pane when opened from the
 * library, or the right pane (`compact`) when opened from a citation.
 */
export default function ReaderView({
	target: reader,
	onBack: closeReader,
	backLabel,
	compact = false,
}: {
	target: ReaderTarget | null;
	onBack: () => void;
	backLabel: string;
	compact?: boolean;
}) {
	const qc = useQueryClient();
	const { libraryScope, conversationId } = useNav();
	const conversations = useConversations();
	// Text saved from the reader goes to the project in view: the open chat's when this is the
	// citation pane, the library's filter when it fills the main pane.
	const projectId = compact
		? (conversations.data?.find((c) => c.id === conversationId)?.projectId ??
			null)
		: libraryScope;
	const docs = useDocuments(null);
	const passage = usePassage(reader?.passageId ?? null);

	const doc = docs.data?.find((d) => d.id === reader?.documentId);
	const removed = docs.isSuccess && reader && !doc;
	// A citation keeps a saved copy of its passage, so it still opens after the document is gone
	// or was read again (which gives its passages new ids).
	const saved = useSource(
		reader?.source ?? null,
		Boolean(removed) || passage.isError,
	);
	// Once per document opened, for the opt-in usage counts.
	const openedId = reader?.documentId;
	useEffect(() => {
		if (openedId) api.countEvent("reader_opened");
	}, [openedId]);
	useEffect(() => {
		if (removed && !reader?.source) closeReader();
	}, [removed, reader?.source, closeReader]);

	// Stable identity: the reader moves the view whenever this object changes.
	const boxes = passage.data?.boxes ?? NO_BOXES;
	const page = reader?.page ?? 1;
	const highlight = useMemo<Highlight>(() => ({ page, boxes }), [page, boxes]);

	const notice =
		reader?.passageId && passage.isSuccess && boxes.length === 0
			? `Couldn't find the exact spot, so this is the start of page ${page}.`
			: reader?.passageId && passage.isError
				? saved.data
					? `This passage was replaced when the document was read again, so this is the start of page ${page}.`
					: `Couldn't load the passage: ${passage.error.message}`
				: null;
	const passageText = passage.data?.text ?? saved.data?.text;

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className={`flex h-12 shrink-0 items-center gap-3 ${compact ? "px-4" : "px-6"}`}
			>
				<Button variant="ghost" className="-ml-2 px-2" onPress={closeReader}>
					{compact ? (
						<XIcon className="size-[17px]" />
					) : (
						<ArrowLeftIcon className="size-[17px]" />
					)}
					{backLabel}
				</Button>
				<h1 className="min-w-0 truncate text-[15px] font-semibold">
					{doc ? doc.title || doc.originalName : (saved.data?.title ?? "")}
				</h1>
			</header>

			{removed && reader?.source ? (
				<div className="selectable min-h-0 flex-1 overflow-y-auto px-6 pb-6 text-sm">
					<Note
						title="This document is no longer in your library"
						className="mb-3"
					>
						{saved.isError
							? "The saved passage couldn't be loaded."
							: `Here is the passage as it was cited${
									saved.data && saved.data.page > 0
										? `, from page ${saved.data.page}`
										: ""
								}.`}
					</Note>
					{saved.data && (
						<blockquote className="max-w-prose border-l-2 border-primary/50 pl-3">
							{saved.data.text}
						</blockquote>
					)}
				</div>
			) : !reader || !doc ? (
				<div className="flex flex-1 items-center justify-center">
					<Spinner />
				</div>
			) : doc.kind !== "pdf" ? (
				<div className="selectable min-h-0 flex-1 overflow-y-auto px-6 pb-6 text-sm">
					<Note className="mb-3">
						Word documents can't be previewed yet.
						{passageText ? " Here is the passage that matched." : ""}
					</Note>
					{passageText && (
						<blockquote className="max-w-prose border-l-2 border-primary/50 pl-3">
							{passageText}
						</blockquote>
					)}
				</div>
			) : (
				<>
					{notice && <Note className="mx-6 mb-2">{notice}</Note>}
					<PdfReader
						key={doc.id}
						documentId={doc.id}
						highlight={highlight}
						onSaveSelection={async (page, text) => {
							await api.savePassage(doc.id, page, text, projectId);
							qc.invalidateQueries({ queryKey: keys.saved });
						}}
					/>
				</>
			)}
		</div>
	);
}
