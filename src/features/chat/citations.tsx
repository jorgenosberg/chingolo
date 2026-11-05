import { createContext, type ReactNode, useContext, useMemo } from "react";
import { defaultRemarkPlugins } from "streamdown";
import { type ReaderTarget, useNav } from "../../app/nav";
import type { Citation } from "../../lib/bindings";

const TOKEN = /\[S(\d+)\]/g;

type Node = {
	type: string;
	value?: string;
	children?: Node[];
	data?: Record<string, unknown>;
};

/** Turns `[S12]` in text into `<cite>` elements. Leaves code and links alone. */
export function remarkCitations() {
	return (tree: Node) => walk(tree);
}

function walk(node: Node) {
	if (!node.children || node.type === "code" || node.type === "link") return;
	const next: Node[] = [];
	for (const child of node.children) {
		if (child.type === "text" && child.value && /\[S\d+\]/.test(child.value)) {
			const value = child.value;
			let last = 0;
			for (const m of value.matchAll(TOKEN)) {
				const at = m.index ?? 0;
				if (at > last)
					next.push({ type: "text", value: value.slice(last, at) });
				// "emphasis" is a node every renderer knows; hName turns it into <cite>.
				next.push({
					type: "emphasis",
					children: [{ type: "text", value: `S${m[1]}` }],
					data: { hName: "cite" },
				});
				last = at + m[0].length;
			}
			if (last < value.length)
				next.push({ type: "text", value: value.slice(last) });
		} else {
			walk(child);
			next.push(child);
		}
	}
	node.children = next;
}

/** Without this Streamdown's sanitizer drops the element and leaves bare text. */
export const citeTags = { cite: [] as string[] };
/** Streamdown replaces its own remark plugins (tables, strikethrough) when given any, so keep them. */
export const citePlugins = [
	...Object.values(defaultRemarkPlugins),
	remarkCitations,
];

const base =
	"mx-0.5 inline-flex h-5 items-center rounded-md border px-1.5 align-baseline text-[11px] font-medium leading-none";

function explain(c: Citation): string {
	const where = c.page > 0 ? `${c.title}, p. ${c.page}` : c.title;
	const warning =
		c.status === "quote_mismatch"
			? "The quoted words are not in this source. "
			: c.status === "weak"
				? "This source may not support the claim. "
				: "";
	return `${warning}${where}\n“${c.snippet.trim()}…”`;
}

function textOf(children: ReactNode): string {
	return typeof children === "string" ? children : String(children ?? "");
}

type Ctx = {
	byHandle: Map<number, Citation>;
	conversationId: string;
	open: (target: ReaderTarget) => void;
};
const CiteContext = createContext<Ctx | null>(null);

/** Gives the chips of one reply their checked citations. Wrap the reply's Markdown in it. */
export function CiteProvider({
	citations,
	conversationId,
	children,
}: {
	citations: Citation[];
	conversationId: string;
	children: ReactNode;
}) {
	const { openCitation } = useNav();
	const value = useMemo(
		() => ({
			byHandle: new Map(citations.map((c) => [c.handle, c])),
			conversationId,
			open: openCitation,
		}),
		[citations, conversationId, openCitation],
	);
	return <CiteContext.Provider value={value}>{children}</CiteContext.Provider>;
}

/** The `<cite>` element of a reply: a chip that opens the source beside the chat. */
function Cite({ children }: { children?: ReactNode }) {
	const ctx = useContext(CiteContext);
	const label = textOf(children);
	const c = ctx?.byHandle.get(Number(label.slice(1)));
	// While a reply streams, and for anything the app didn't check, show it as plain text.
	if (!ctx || !c)
		return (
			<span className={`${base} border-transparent text-muted-foreground`}>
				{label}
			</span>
		);
	const flagged = c.status !== "ok";
	return (
		<button
			type="button"
			title={explain(c)}
			aria-label={`Open source ${c.handle}: ${c.title}${c.page > 0 ? `, page ${c.page}` : ""}${flagged ? " (not fully supported)" : ""}`}
			onClick={() =>
				ctx.open({
					documentId: c.documentId,
					page: Math.max(c.page, 1),
					passageId: c.passageId,
					source: { conversationId: ctx.conversationId, handle: c.handle },
				})
			}
			className={`${base} cursor-pointer transition-colors ${
				flagged
					? "border-dashed border-problem text-problem hover:bg-muted"
					: "border-border bg-muted text-muted-foreground hover:bg-accent-soft hover:text-foreground"
			}`}
		>
			{label}
		</button>
	);
}

export const citeComponents = { cite: Cite };
