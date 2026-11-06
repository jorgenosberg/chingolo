import { createContext, type ReactNode, useContext, useMemo } from "react";
import { Button, Tooltip, TooltipTrigger } from "react-aria-components";
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

function warningFor(c: Citation): string | null {
	return c.status === "quote_mismatch"
		? "The quoted words are not in this source."
		: c.status === "weak"
			? "This source may not support the claim."
			: null;
}

function where(c: Citation): string {
	return c.page > 0 ? `${c.title}, p. ${c.page}` : c.title;
}

/** Shown when a chip is hovered or focused: what it points to and any warning. */
function CiteTip({ c }: { c: Citation }) {
	const warning = warningFor(c);
	return (
		<Tooltip
			placement="top"
			offset={6}
			className="max-w-sm rounded-lg border border-border bg-card px-3 py-2 text-xs text-foreground shadow-lg"
		>
			{warning && <p className="mb-1 font-medium text-problem">{warning}</p>}
			<p className="font-medium">{where(c)}</p>
			<p className="mt-1 line-clamp-5 text-muted-foreground">
				“{c.snippet.trim()}…”
			</p>
		</Tooltip>
	);
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
		<TooltipTrigger delay={300} closeDelay={100}>
			<Button
				aria-label={`Open source ${c.handle}: ${c.title}${c.page > 0 ? `, page ${c.page}` : ""}${flagged ? " (not fully supported)" : ""}`}
				onPress={() => ctx.open(targetOf(c, ctx.conversationId))}
				className={`${base} cursor-pointer outline-none transition-colors data-[focus-visible]:ring-2 data-[focus-visible]:ring-primary/60 ${
					flagged
						? "border-dashed border-problem text-problem data-[hovered]:bg-muted"
						: "border-border bg-muted text-muted-foreground data-[hovered]:bg-accent-soft data-[hovered]:text-foreground"
				}`}
			>
				{label}
			</Button>
			<CiteTip c={c} />
		</TooltipTrigger>
	);
}

function targetOf(c: Citation, conversationId: string): ReaderTarget {
	return {
		documentId: c.documentId,
		page: Math.max(c.page, 1),
		passageId: c.passageId,
		source: { conversationId, handle: c.handle },
	};
}

/** The documents a reply cited, with a chip for each passage. Collapsed until opened. */
export function SourcesList({
	citations,
	conversationId,
}: {
	citations: Citation[];
	conversationId: string;
}) {
	const { openCitation } = useNav();
	const docs = useMemo(() => {
		const byDoc = new Map<string, Citation[]>();
		for (const c of [...citations].sort((a, b) => a.handle - b.handle)) {
			byDoc.set(c.documentId, [...(byDoc.get(c.documentId) ?? []), c]);
		}
		return [...byDoc.values()];
	}, [citations]);
	if (docs.length === 0) return null;
	return (
		<details className="group/sources text-[13px]">
			<summary className="w-fit cursor-pointer select-none list-none rounded-md px-1 py-0.5 text-xs text-muted-foreground hover:text-foreground">
				Sources ({docs.length})
			</summary>
			<ul className="mt-1 flex flex-col gap-1.5 border-l border-border pl-3">
				{docs.map((passages) => (
					<li key={passages[0].documentId} className="flex flex-col gap-0.5">
						<span className="font-medium">{passages[0].title}</span>
						<span className="flex flex-wrap items-center">
							{passages.map((c) => (
								<TooltipTrigger key={c.handle} delay={300} closeDelay={100}>
									<Button
										aria-label={`Open source ${c.handle}: ${where(c)}`}
										onPress={() => openCitation(targetOf(c, conversationId))}
										className={`${base} cursor-pointer outline-none data-[focus-visible]:ring-2 data-[focus-visible]:ring-primary/60 ${
											c.status !== "ok"
												? "border-dashed border-problem text-problem data-[hovered]:bg-muted"
												: "border-border bg-muted text-muted-foreground data-[hovered]:bg-accent-soft data-[hovered]:text-foreground"
										}`}
									>
										S{c.handle}
										{c.page > 0 && (
											<span className="ml-1 opacity-70">p. {c.page}</span>
										)}
									</Button>
									<CiteTip c={c} />
								</TooltipTrigger>
							))}
						</span>
					</li>
				))}
			</ul>
		</details>
	);
}

export const citeComponents = { cite: Cite };
