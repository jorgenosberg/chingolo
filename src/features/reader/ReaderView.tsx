import { useEffect, useMemo } from "react";
import type { ReaderTarget } from "../../app/nav";
import { ChevronIcon } from "../../components/icons";
import { Button, Spinner } from "../../components/ui";
import { useDocuments, usePassage } from "../../lib/queries";
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
	const docs = useDocuments(null);
	const passage = usePassage(reader?.passageId ?? null);

	const doc = docs.data?.find((d) => d.id === reader?.documentId);
	const removed = docs.isSuccess && reader && !doc;
	useEffect(() => {
		if (removed) closeReader();
	}, [removed, closeReader]);

	// Stable identity: the reader moves the view whenever this object changes.
	const boxes = passage.data?.boxes ?? NO_BOXES;
	const page = reader?.page ?? 1;
	const highlight = useMemo<Highlight>(() => ({ page, boxes }), [page, boxes]);

	const notice =
		reader?.passageId && passage.isSuccess && boxes.length === 0
			? `Couldn't find the exact spot, so this is the start of page ${page}.`
			: reader?.passageId && passage.isError
				? `Couldn't load the passage: ${passage.error.message}`
				: null;

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className={`flex h-12 shrink-0 items-center gap-3 ${compact ? "px-4" : "px-6"}`}
			>
				<Button variant="ghost" className="-ml-2 px-2" onPress={closeReader}>
					{!compact && <ChevronIcon className="size-3.5 rotate-180" />}
					{backLabel}
				</Button>
				<h1 className="min-w-0 truncate text-[15px] font-semibold">
					{doc ? doc.title || doc.originalName : ""}
				</h1>
			</header>

			{!reader || !doc ? (
				<div className="flex flex-1 items-center justify-center">
					<Spinner />
				</div>
			) : doc.kind !== "pdf" ? (
				<div className="selectable min-h-0 flex-1 overflow-y-auto px-6 pb-6 text-sm">
					<p className="mb-3 text-xs text-muted-foreground">
						Word documents can't be previewed yet.
						{passage.data ? " Here is the passage that matched." : ""}
					</p>
					{passage.data && (
						<blockquote className="max-w-prose border-l-2 border-primary/50 pl-3">
							{passage.data.text}
						</blockquote>
					)}
				</div>
			) : (
				<>
					{notice && (
						<p
							role="status"
							className="mx-6 mb-2 rounded-md bg-muted px-2.5 py-1.5 text-xs text-muted-foreground"
						>
							{notice}
						</p>
					)}
					<PdfReader key={doc.id} documentId={doc.id} highlight={highlight} />
				</>
			)}
		</div>
	);
}
