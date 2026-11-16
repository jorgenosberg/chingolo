import type { PageRegion } from "../../lib/bindings";
import { rectToPercent } from "./highlight";

type PageView = Parameters<typeof rectToPercent>[0] extends infer V
	? { div?: HTMLElement; pdfPage?: unknown; viewport: V }
	: never;

const CLASS = "layout-overlay";

/** What a label means for passages: furniture is dropped, the rest keeps its own kind. */
function kind(label: string): string {
	if (
		["header", "header_image", "footer", "footer_image", "number"].includes(
			label,
		)
	)
		return "furniture";
	if (label === "table") return "table";
	if (["chart", "image", "figure_title", "chart_title", "seal"].includes(label))
		return "figure";
	if (["footnote", "vision_footnote"].includes(label)) return "aside";
	if (label.includes("formula")) return "formula";
	return "body";
}

export function clearLayout(root: HTMLElement) {
	for (const el of root.querySelectorAll(`.${CLASS}`)) el.remove();
}

/** Draws the regions on every page that is ready. Boxes are numbered in reading order. */
export function drawLayout(
	regions: PageRegion[],
	getPageView: (index: number) => PageView | undefined,
) {
	const pages = new Map<number, PageRegion[]>();
	for (const r of regions) pages.set(r.page, [...(pages.get(r.page) ?? []), r]);
	for (const [page, list] of pages) {
		const pv = getPageView(page - 1);
		if (!pv?.div || !pv.pdfPage) continue;
		for (const old of pv.div.querySelectorAll(`.${CLASS}`)) old.remove();
		const overlay = document.createElement("div");
		overlay.className = CLASS;
		overlay.setAttribute("aria-hidden", "true");
		for (const r of list) {
			if (r.rect.some((n) => n === null)) continue;
			const p = rectToPercent(
				pv.viewport,
				r.rect as [number, number, number, number],
			);
			const score = r.score ?? 0;
			const box = document.createElement("div");
			box.className = `layout-box layout-${kind(r.label)}`;
			box.style.left = `${p.left}%`;
			box.style.top = `${p.top}%`;
			box.style.width = `${p.width}%`;
			box.style.height = `${p.height}%`;
			box.title = `${r.ordinal + 1}. ${r.label} (${score.toFixed(2)})`;
			const tag = document.createElement("span");
			tag.textContent = `${r.ordinal + 1} ${r.label} ${Math.round(score * 100)}`;
			box.append(tag);
			overlay.append(box);
		}
		pv.div.append(overlay);
	}
}
