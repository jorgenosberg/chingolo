import type { PageBoxes } from "../../lib/bindings";

/** The part of a PDF.js page viewport that the highlight code needs. */
type Viewport = {
	width: number;
	height: number;
	convertToViewportPoint: (x: number, y: number) => number[];
};

type PageView = {
	div?: HTMLElement;
	pdfPage?: unknown;
	viewport: Viewport;
};

const OVERLAY_CLASS = "passage-highlight";

/**
 * Box position as percentages of the page, so it stays right when the zoom changes before
 * the page is drawn again. Two opposite corners go through the viewport, which handles
 * rotation and crop boxes, then they are normalized.
 */
export function rectToPercent(
	viewport: Viewport,
	[left, bottom, right, top]: [number, number, number, number],
) {
	const [x1, y1] = viewport.convertToViewportPoint(left, bottom);
	const [x2, y2] = viewport.convertToViewportPoint(right, top);
	return {
		left: (Math.min(x1, x2) / viewport.width) * 100,
		top: (Math.min(y1, y2) / viewport.height) * 100,
		width: (Math.abs(x2 - x1) / viewport.width) * 100,
		height: (Math.abs(y2 - y1) / viewport.height) * 100,
	};
}

/** Removes highlights from every page element under `root`. */
export function clearHighlights(root: HTMLElement) {
	for (const el of root.querySelectorAll(`.${OVERLAY_CLASS}`)) el.remove();
}

/**
 * Draws a passage's boxes on the pages that are ready. Returns the first box drawn, so the
 * caller can scroll to it. Pages that aren't drawn yet are skipped. Call again when they are.
 */
export function drawHighlights(
	boxes: PageBoxes[],
	getPageView: (index: number) => PageView | undefined,
): HTMLElement | null {
	let first: HTMLElement | null = null;
	for (const group of boxes) {
		const pv = getPageView(group.page - 1);
		if (!pv?.div || !pv.pdfPage) continue;
		for (const old of pv.div.querySelectorAll(`.${OVERLAY_CLASS}`))
			old.remove();

		const overlay = document.createElement("div");
		overlay.className = OVERLAY_CLASS;
		overlay.setAttribute("aria-hidden", "true");
		for (const rect of group.rects) {
			// A non-finite coordinate arrives as null. Skip that box.
			if (rect.some((n) => n === null)) continue;
			const p = rectToPercent(
				pv.viewport,
				rect as [number, number, number, number],
			);
			const box = document.createElement("div");
			box.className = "passage-highlight-box";
			box.style.left = `${p.left}%`;
			box.style.top = `${p.top}%`;
			box.style.width = `${p.width}%`;
			box.style.height = `${p.height}%`;
			overlay.append(box);
			first ??= box;
		}
		pv.div.append(overlay);
	}
	return first;
}
