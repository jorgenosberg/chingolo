import type { PDFViewer } from "pdfjs-dist/web/pdf_viewer.mjs";

/**
 * Smooth pinch zoom for a PDF.js viewer.
 *
 * Asking PDF.js to rescale on every pinch event re-lays out and repaints the page canvases each
 * frame, which runs at about 15 fps in WKWebView. A CSS transform on a layer of its own is only
 * a GPU texture scale, which holds 60 fps. So while the fingers are down the whole viewer is
 * scaled with a transform, and PDF.js gets the final zoom once, when the gesture ends.
 *
 * `scale` below is the PDF.js scale (1 = 100 percent). `r` is the preview ratio against the
 * scale the gesture started at.
 */

type Opts = {
	container: HTMLElement;
	/** The `.pdfViewer` element inside the container. */
	content: HTMLElement;
	getViewer: () => PDFViewer | null;
	min: number;
	max: number;
	/** Called when a gesture starts, e.g. to stop auto fit. */
	onStart?: () => void;
};

type Gesture = {
	startScale: number;
	scale: number;
	/** The finger position inside the container. */
	fx: number;
	fy: number;
	/** Scroll position and layout of the page under the fingers when the gesture began. */
	scrollLeft: number;
	scrollTop: number;
	left0: number;
	pageW0: number;
	/** Width of the page border, which does not scale with the zoom. */
	border: number;
	width: number;
	height: number;
	/** The page under the fingers, and where in its content area (0 to 1) they were. */
	page: HTMLElement | null;
	ax: number;
	ay: number;
};

/** WebKit's non-standard pinch events carry a running `scale` and the pointer position. */
type GestureEvent = Event & { scale: number; clientX: number; clientY: number };

/** PDF.js rounds scale to 0.01. Rounding here keeps the preview equal to the final layout. */
const round2 = (n: number) => Math.round(n * 100) / 100;

/** A mouse wheel notch is 100. Capping keeps one notch from jumping the zoom. */
const WHEEL_STEP_CAP = 40;
/** A pinch on a trackpad that sends wheel events (Chrome) has no end event, so wait this long. */
const WHEEL_END_MS = 150;

export function attachPinchZoom({
	container,
	content,
	getViewer,
	min,
	max,
	onStart,
}: Opts): () => void {
	let g: Gesture | null = null;
	let spacer: HTMLElement | null = null;
	let wheelTimer = 0;
	let wheelDelta = 0;

	// PDF.js reads the scroll position to decide which pages are visible. During the preview
	// the scroll position is in preview units, so PDF.js would render the wrong pages and
	// report page changes. This listener must be added before the viewer's own.
	const swallowScroll = (e: Event) => {
		if (g) e.stopImmediatePropagation();
	};
	container.addEventListener("scroll", swallowScroll);

	function begin(clientX: number, clientY: number, scale: number) {
		const viewer = getViewer();
		if (!viewer?.pagesCount) return;
		onStart?.();
		const box = container.getBoundingClientRect();
		const under = document
			.elementFromPoint(clientX, clientY)
			?.closest<HTMLElement>(".page");
		const page =
			under ??
			(viewer.getPageView(viewer.currentPageNumber - 1)?.div as
				| HTMLElement
				| undefined) ??
			null;
		const pr = page?.getBoundingClientRect();
		const cr = content.getBoundingClientRect();
		g = {
			startScale: scale,
			scale,
			fx: clientX - box.left,
			fy: clientY - box.top,
			scrollLeft: container.scrollLeft,
			scrollTop: container.scrollTop,
			left0: pr ? pr.left - cr.left : 0,
			pageW0: pr?.width ?? 0,
			border: page?.clientLeft ?? 0,
			width: content.clientWidth,
			height: content.offsetHeight,
			page,
			ax:
				pr && page?.clientWidth
					? (clientX - pr.left - page.clientLeft) / page.clientWidth
					: 0,
			ay:
				pr && page?.clientHeight
					? (clientY - pr.top - page.clientTop) / page.clientHeight
					: 0,
		};
		content.style.transformOrigin = "0 0";
		content.style.willChange = "transform";
		// Once the viewer has its own layer, WebKit stops recomputing the scroll range when
		// only the transform changes, so scrollLeft and scrollTop would clamp to the old size.
		// A hidden box at the far corner of the scaled content keeps the range right.
		spacer = document.createElement("div");
		spacer.setAttribute("aria-hidden", "true");
		spacer.style.cssText =
			"position:absolute;width:1px;height:1px;visibility:hidden;pointer-events:none";
		container.append(spacer);
	}

	function preview(target: number) {
		if (!g) return;
		g.scale = round2(Math.min(Math.max(target, min), max));
		const r = g.scale / g.startScale;
		// Pages narrower than the container are centered, so the final layout moves them
		// sideways as they grow. Match that, or the commit would jump.
		const pageW = r * (g.pageW0 - 2 * g.border) + 2 * g.border;
		const leftFinal = Math.max(0, (g.width - pageW) / 2);
		const tx = leftFinal - r * g.left0;
		content.style.transform = `translate(${tx}px, 0) scale(${r})`;
		if (spacer) {
			spacer.style.left = `${tx + r * g.width}px`;
			spacer.style.top = `${r * g.height}px`;
		}
		// Keep the point that was under the fingers under the fingers.
		container.scrollLeft = tx + r * (g.scrollLeft + g.fx) - g.fx;
		container.scrollTop = r * (g.scrollTop + g.fy) - g.fy;
	}

	function commit() {
		window.clearTimeout(wheelTimer);
		wheelDelta = 0;
		const done = g;
		g = null; // scroll events reach PDF.js again from here
		if (!done) return;
		content.style.transform = "";
		content.style.transformOrigin = "";
		content.style.willChange = "";
		spacer?.remove();
		spacer = null;
		const viewer = getViewer();
		if (!viewer) return;
		if (done.scale !== viewer.currentScale) viewer.currentScale = done.scale;
		// PDF.js floors each page to whole pixels, so absolute offsets drift from the preview,
		// by hundreds of pixels deep into a long document. The page under the fingers does
		// not drift, so anchor on it.
		const page = done.page;
		if (page?.isConnected) {
			const pr = page.getBoundingClientRect();
			const box = container.getBoundingClientRect();
			const x = pr.left + page.clientLeft + done.ax * page.clientWidth;
			const y = pr.top + page.clientTop + done.ay * page.clientHeight;
			container.scrollLeft += x - (box.left + done.fx);
			container.scrollTop += y - (box.top + done.fy);
		}
	}

	const onGestureStart = (e: Event) => {
		e.preventDefault();
		const ev = e as GestureEvent;
		commit();
		const viewer = getViewer();
		if (viewer) begin(ev.clientX, ev.clientY, viewer.currentScale);
	};
	const onGestureChange = (e: Event) => {
		e.preventDefault();
		const ev = e as GestureEvent;
		if (!g) onGestureStart(e);
		if (g) preview(g.startScale * ev.scale);
	};
	const onGestureEnd = (e: Event) => {
		e.preventDefault();
		commit();
	};
	const onWheel = (e: WheelEvent) => {
		if (!e.ctrlKey) return; // plain two-finger scrolling is left to the browser
		e.preventDefault();
		if (!g) {
			const viewer = getViewer();
			if (!viewer) return;
			begin(e.clientX, e.clientY, viewer.currentScale);
			wheelDelta = 0;
		}
		if (!g) return;
		wheelDelta += Math.max(-WHEEL_STEP_CAP, Math.min(WHEEL_STEP_CAP, e.deltaY));
		preview(g.startScale * Math.exp(-wheelDelta / 100));
		window.clearTimeout(wheelTimer);
		wheelTimer = window.setTimeout(commit, WHEEL_END_MS);
	};

	container.addEventListener("gesturestart", onGestureStart);
	container.addEventListener("gesturechange", onGestureChange);
	container.addEventListener("gestureend", onGestureEnd);
	container.addEventListener("wheel", onWheel, { passive: false });

	return () => {
		commit();
		container.removeEventListener("scroll", swallowScroll);
		container.removeEventListener("gesturestart", onGestureStart);
		container.removeEventListener("gesturechange", onGestureChange);
		container.removeEventListener("gestureend", onGestureEnd);
		container.removeEventListener("wheel", onWheel);
	};
}
