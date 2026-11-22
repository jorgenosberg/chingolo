import "pdfjs-dist/web/pdf_viewer.css";
import {
	CaretDownIcon,
	CaretUpIcon,
	FrameCornersIcon,
	MagnifyingGlassIcon,
	MagnifyingGlassMinusIcon,
	MagnifyingGlassPlusIcon,
} from "@phosphor-icons/react";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { useEffect, useRef, useState } from "react";
import { SelectionPill } from "../../components/SelectionPill";
import { Button, IconButton, Spinner } from "../../components/ui";
import type { PageBoxes } from "../../lib/bindings";
import { useLayoutOverlay } from "../../lib/devtools";
import { useLayout } from "../../lib/queries";
import { clearHighlights, drawHighlights } from "./highlight";
import { clearLayout, drawLayout } from "./layoutOverlay";
import { attachPinchZoom } from "./pinch";

type Viewer = import("pdfjs-dist/web/pdf_viewer.mjs").PDFViewer;
type EventBus = import("pdfjs-dist/web/pdf_viewer.mjs").EventBus;

export type Highlight = {
	page: number;
	/** Empty when the exact spot isn't known. The page still opens. */
	boxes: PageBoxes[];
};

type Find = { query: string; current: number; total: number; pending: boolean };

const ZOOM_STEP = 1.15;
/** On a wide pane "fit width" would make pages huge. PDF.js centers pages that are narrower. */
const MAX_FIT_SCALE = 1.6;
const MIN_SCALE = 0.3;
const MAX_SCALE = 5;
function fitWidth(viewer: Viewer) {
	viewer.currentScaleValue = "page-width";
	if (viewer.currentScale > MAX_FIT_SCALE) viewer.currentScale = MAX_FIT_SCALE;
}

/**
 * The selected words on one line of text. The selection keeps PDF line breaks (and hyphens at
 * line ends), so join them back up.
 */
function selectedText(range: Range): string {
	const raw = window.getSelection()?.toString() || range.toString();
	return raw
		.replace(/-\s*\n\s*(?=[a-zæøå])/g, "")
		.replace(/\s+/g, " ")
		.trim();
}

/** `chingolo-doc` is registered in Rust. A document is addressed by id, never by path. */
const docUrl = (id: string) => `chingolo-doc://localhost/${id}`;

function assetUrl(dir: string) {
	return new URL(`/pdfjs/${dir}/`, window.location.origin).href;
}

/**
 * PDF.js viewer for one document: continuous scroll, selectable text, find, zoom, and a
 * highlight drawn from stored line boxes. `highlight` changes move the view without a reload.
 */
export function PdfReader({
	documentId,
	highlight,
	onSaveSelection,
}: {
	documentId: string;
	highlight: Highlight;
	/** Saves selected text. Without it the reader shows no Save button. */
	onSaveSelection?: (page: number, text: string) => Promise<unknown>;
}) {
	const containerRef = useRef<HTMLDivElement>(null);
	const viewerElRef = useRef<HTMLDivElement>(null);
	const viewerRef = useRef<Viewer | null>(null);
	const busRef = useRef<EventBus | null>(null);
	const autoFit = useRef(true);
	const highlightRef = useRef(highlight);
	highlightRef.current = highlight;
	const scrolledFor = useRef<Highlight | null>(null);
	const showLayout = useLayoutOverlay();
	const layout = useLayout(documentId, showLayout);
	const regionsRef = useRef(layout.data);
	regionsRef.current = showLayout ? layout.data : undefined;

	const [status, setStatus] = useState<"loading" | "ready" | "error">(
		"loading",
	);
	const [error, setError] = useState("");
	const [pages, setPages] = useState({ current: 1, total: 0 });
	const [find, setFind] = useState<Find>({
		query: "",
		current: 0,
		total: 0,
		pending: false,
	});

	// Draws the current highlight on pages that are ready and scrolls to it once.
	const paint = useRef(() => {});
	paint.current = () => {
		const viewer = viewerRef.current;
		const root = viewerElRef.current;
		if (!viewer || !root) return;
		clearHighlights(root);
		const h = highlightRef.current;
		const first = drawHighlights(h.boxes, (i) => {
			try {
				return viewer.getPageView(i);
			} catch {
				return undefined;
			}
		});
		clearLayout(root);
		const regions = regionsRef.current;
		if (regions?.length) {
			drawLayout(regions, (i) => {
				try {
					return viewer.getPageView(i);
				} catch {
					return undefined;
				}
			});
		}
		if (first && scrolledFor.current !== h) {
			scrolledFor.current = h;
			first.scrollIntoView({ block: "center", behavior: "instant" });
		}
	};

	useEffect(() => {
		let disposed = false;
		let teardown = () => {};
		let detachPinch = () => {};
		setStatus("loading");
		setError("");
		setPages({ current: 1, total: 0 });

		(async () => {
			// The viewer reads `globalThis.pdfjsLib` when it loads, so the core goes first.
			const pdfjs = await import("pdfjs-dist");
			const lib = await import("pdfjs-dist/web/pdf_viewer.mjs");
			if (typeof lib.EventBus !== "function") {
				throw new Error("the PDF.js viewer module didn't load correctly");
			}
			const container = containerRef.current;
			const viewerEl = viewerElRef.current;
			if (disposed || !container || !viewerEl) return;

			pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
			// Before the viewer, so its scroll guard runs ahead of PDF.js's own listener.
			detachPinch = attachPinchZoom({
				container,
				content: viewerEl,
				getViewer: () => viewerRef.current,
				min: MIN_SCALE,
				max: MAX_SCALE,
				onStart: () => {
					autoFit.current = false;
				},
			});
			const eventBus = new lib.EventBus();
			const linkService = new lib.PDFLinkService({ eventBus });
			const findController = new lib.PDFFindController({
				eventBus,
				linkService,
			});
			const viewer = new lib.PDFViewer({
				container,
				viewer: viewerEl,
				eventBus,
				linkService,
				findController,
			});
			linkService.setViewer(viewer);
			viewerRef.current = viewer;
			busRef.current = eventBus;

			eventBus.on("pagesinit", () => {
				if (disposed) return;
				fitWidth(viewer);
				const h = highlightRef.current;
				viewer.currentPageNumber = Math.min(
					Math.max(h.page, 1),
					viewer.pagesCount,
				);
				setPages({
					current: viewer.currentPageNumber,
					total: viewer.pagesCount,
				});
				setStatus("ready");
			});
			eventBus.on("pagerendered", () => paint.current());
			eventBus.on("pagechanging", (e: { pageNumber: number }) =>
				setPages((p) => ({ ...p, current: e.pageNumber })),
			);
			const onFind = (e: {
				state?: number;
				matchesCount: { current: number; total: number };
			}) =>
				setFind((f) => ({
					...f,
					current: e.matchesCount.current,
					total: e.matchesCount.total,
					// 3 is PDF.js's "pending" state: the search is still running.
					pending: e.state === 3,
				}));
			eventBus.on("updatefindmatchescount", onFind);
			eventBus.on("updatefindcontrolstate", onFind);

			const task = pdfjs.getDocument({
				url: docUrl(documentId),
				wasmUrl: assetUrl("wasm"),
				cMapUrl: assetUrl("cmaps"),
				standardFontDataUrl: assetUrl("standard_fonts"),
				iccUrl: assetUrl("iccs"),
			});
			const resize = new ResizeObserver(() => {
				if (autoFit.current && viewer.pagesCount) {
					fitWidth(viewer);
				}
			});
			resize.observe(container);

			teardown = () => {
				resize.disconnect();
				try {
					viewer.setDocument(null);
				} catch {
					// the viewer may already be torn down
				}
				task.destroy();
				viewerRef.current = null;
				busRef.current = null;
			};
			if (disposed) return teardown();

			try {
				const pdf = await task.promise;
				if (disposed) return;
				viewer.setDocument(pdf);
				linkService.setDocument(pdf, null);
			} catch (e) {
				if (disposed) return;
				setError(e instanceof Error ? e.message : String(e));
				setStatus("error");
			}
		})().catch((e) => {
			if (disposed) return;
			setError(e instanceof Error ? e.message : String(e));
			setStatus("error");
		});

		return () => {
			disposed = true;
			detachPinch();
			teardown();
		};
	}, [documentId]);

	// A new highlight on the same document: go to its page and draw it.
	useEffect(() => {
		const viewer = viewerRef.current;
		if (!viewer || status !== "ready") return;
		viewer.currentPageNumber = Math.min(
			Math.max(highlight.page, 1),
			viewer.pagesCount,
		);
		paint.current();
	}, [highlight, status]);

	// The developer overlay was switched on or off, or its regions arrived.
	// biome-ignore lint/correctness/useExhaustiveDependencies: repaint when these change
	useEffect(() => {
		if (status === "ready") paint.current();
	}, [showLayout, layout.data, status]);

	const runFind = (query: string, again = false, previous = false) => {
		setFind((f) => ({ ...f, query }));
		busRef.current?.dispatch("find", {
			source: window,
			type: again ? "again" : "",
			query,
			caseSensitive: false,
			entireWord: false,
			highlightAll: true,
			findPrevious: previous,
			matchDiacritics: true,
		});
	};

	const zoom = (factor: number) => {
		const viewer = viewerRef.current;
		if (!viewer) return;
		autoFit.current = false;
		viewer.currentScale = Math.min(
			Math.max(viewer.currentScale * factor, MIN_SCALE),
			MAX_SCALE,
		);
	};

	const goTo = (n: number) => {
		const viewer = viewerRef.current;
		if (!viewer || Number.isNaN(n)) return;
		viewer.currentPageNumber = Math.min(Math.max(n, 1), viewer.pagesCount);
	};

	const ready = status === "ready";
	const noMatches = find.query && !find.pending && find.total === 0;

	return (
		<div className="flex min-h-0 flex-1 flex-col">
			<form
				className="flex items-center gap-1.5 px-3 pb-2"
				onSubmit={(e) => {
					e.preventDefault();
					if (find.query) runFind(find.query, true);
				}}
			>
				<div className="relative min-w-0 flex-1">
					<MagnifyingGlassIcon className="pointer-events-none absolute left-2 top-1.5 size-4 text-muted-foreground" />
					<input
						type="search"
						aria-label="Find in document"
						placeholder="Find in document"
						value={find.query}
						disabled={!ready}
						onChange={(e) => runFind(e.target.value)}
						className="h-7 w-full rounded-md border border-transparent bg-muted pr-2 pl-7 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50 disabled:opacity-40"
					/>
				</div>
				{find.query && (
					<span
						className={`shrink-0 text-xs ${noMatches ? "text-problem" : "text-muted-foreground"}`}
					>
						{noMatches ? "No matches" : `${find.current} of ${find.total}`}
					</span>
				)}
				<IconButton
					label="Previous match"
					isDisabled={!find.query || !find.total}
					onPress={() => runFind(find.query, true, true)}
				>
					<CaretUpIcon />
				</IconButton>
				<IconButton
					label="Next match"
					isDisabled={!find.query || !find.total}
					onPress={() => runFind(find.query, true)}
				>
					<CaretDownIcon />
				</IconButton>
			</form>

			<div className="relative min-h-0 flex-1 border-y border-border bg-muted">
				<div
					ref={containerRef}
					className="selectable absolute inset-0 overflow-auto overscroll-contain"
				>
					<div ref={viewerElRef} className="pdfViewer" />
				</div>
				{onSaveSelection && (
					<SelectionPill
						containerRef={containerRef}
						onSave={(range) => {
							const start =
								range.startContainer instanceof Element
									? range.startContainer
									: range.startContainer.parentElement;
							const page = Number(
								start?.closest(".page")?.getAttribute("data-page-number"),
							);
							return onSaveSelection(
								page > 0 ? page : pages.current,
								selectedText(range),
							);
						}}
					/>
				)}
				{status === "loading" && (
					<div className="absolute inset-0 flex items-center justify-center">
						<Spinner />
					</div>
				)}
				{status === "error" && (
					<div className="absolute inset-0 flex flex-col items-center justify-center gap-2 px-6 text-center">
						<p role="alert" className="text-sm text-problem">
							Couldn't open this document: {error}
						</p>
					</div>
				)}
			</div>

			<div className="flex shrink-0 items-center justify-between gap-2 px-3 py-2">
				<div className="flex items-center gap-1.5 text-xs text-muted-foreground">
					<IconButton
						label="Previous page"
						isDisabled={!ready || pages.current <= 1}
						onPress={() => goTo(pages.current - 1)}
					>
						<CaretUpIcon />
					</IconButton>
					<input
						aria-label="Page"
						inputMode="numeric"
						disabled={!ready}
						value={pages.current}
						onChange={(e) => {
							const n = Number.parseInt(e.target.value, 10);
							if (!Number.isNaN(n)) goTo(n);
						}}
						className="h-7 w-10 rounded-md border border-input bg-card text-center text-[13px] text-foreground outline-none focus:border-primary/50 disabled:opacity-40"
					/>
					<span>of {pages.total || "…"}</span>
					<IconButton
						label="Next page"
						isDisabled={!ready || pages.current >= pages.total}
						onPress={() => goTo(pages.current + 1)}
					>
						<CaretDownIcon />
					</IconButton>
				</div>
				<div className="flex items-center gap-1">
					<IconButton
						label="Zoom out"
						isDisabled={!ready}
						onPress={() => zoom(1 / ZOOM_STEP)}
					>
						<MagnifyingGlassMinusIcon />
					</IconButton>
					<IconButton
						label="Zoom in"
						isDisabled={!ready}
						onPress={() => zoom(ZOOM_STEP)}
					>
						<MagnifyingGlassPlusIcon />
					</IconButton>
					<Button
						variant="ghost"
						className="h-7 px-2 text-xs"
						isDisabled={!ready}
						onPress={() => {
							autoFit.current = true;
							if (viewerRef.current) {
								fitWidth(viewerRef.current);
							}
						}}
					>
						<FrameCornersIcon className="size-4" />
						Fit width
					</Button>
				</div>
			</div>
		</div>
	);
}
