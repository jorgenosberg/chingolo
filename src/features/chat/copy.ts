/** Chrome Streamdown draws around content: copy/download buttons, code block headers. */
const UI = [
	"[data-cite]",
	"button",
	"[data-streamdown=code-block-header]",
	"[data-streamdown=code-block-actions]",
	"[data-streamdown=table-fullscreen]",
	"[data-streamdown=image-overlay]",
	".katex-html",
].join(",");

const KEEP_ATTRS: Record<string, string[]> = { a: ["href"], td: ["colspan"] };

/** Clean HTML and plain text for a rendered reply or a selection inside one. */
export function cleanCopy(source: Node): { html: string; text: string } {
	const root = document.createElement("div");
	root.append(source.cloneNode(true));

	// Streamdown draws bold as a span; make it a real element before spans are unwrapped.
	for (const el of root.querySelectorAll("span[data-streamdown=strong]")) {
		const strong = document.createElement("strong");
		strong.append(...el.childNodes);
		el.replaceWith(strong);
	}

	// "blooms [S15], [S19], [S22]." must paste as "blooms." Drop the separators between
	// chips and the space before the first one.
	const isChip = (n: Node | null) =>
		n instanceof Element && n.hasAttribute("data-cite");
	for (const chip of root.querySelectorAll("[data-cite]")) {
		const next = chip.nextSibling;
		if (
			next?.nodeType === Node.TEXT_NODE &&
			/^[\s,;]*$/.test(next.textContent ?? "") &&
			isChip(next.nextSibling)
		) {
			next.remove();
		}
		if (!isChip(chip.previousSibling)) {
			const prev = chip.previousSibling;
			if (prev?.nodeType === Node.TEXT_NODE) {
				prev.textContent = (prev.textContent ?? "").replace(/\s+$/, "");
			}
		}
	}

	// KaTeX renders each formula twice (MathML and styled HTML). Keep the TeX source.
	for (const m of root.querySelectorAll(".katex")) {
		const tex = m.querySelector('annotation[encoding="application/x-tex"]');
		m.replaceWith(tex?.textContent ? `$${tex.textContent}$` : "");
	}

	for (const el of root.querySelectorAll(UI)) el.remove();

	for (const el of root.querySelectorAll("*")) {
		const keep = KEEP_ATTRS[el.localName] ?? [];
		for (const name of el.getAttributeNames()) {
			if (!keep.includes(name)) el.removeAttribute(name);
		}
	}
	for (const el of root.querySelectorAll("span"))
		el.replaceWith(...el.childNodes);
	root.normalize();

	// innerText needs the node in the document to apply block layout.
	root.style.cssText = "position:fixed;left:-9999px;top:0;white-space:normal";
	document.body.append(root);
	// biome-ignore lint/performance/useDomNodeTextContent: textContent drops the line breaks
	const text = root.innerText.replace(/\n{3,}/g, "\n\n").trim();
	root.remove();

	return { html: root.innerHTML, text };
}

/** Puts both flavours on the clipboard; plain text alone if rich copy is unavailable. */
export async function writeRich({
	html,
	text,
}: {
	html: string;
	text: string;
}) {
	try {
		await navigator.clipboard.write([
			new ClipboardItem({
				"text/html": new Blob([html], { type: "text/html" }),
				"text/plain": new Blob([text], { type: "text/plain" }),
			}),
		]);
	} catch {
		await navigator.clipboard.writeText(text);
	}
}
