import type { ModelInfo } from "../lib/bindings";
import { modelKey } from "../lib/providers";

export function formatTokens(n: number) {
	return n >= 1_000_000
		? `${+(n / 1_000_000).toFixed(1)}M`
		: `${Math.round(n / 1000)}k`;
}

/** USD per 1M tokens: `$5`, `$0.30`, `$2.50`. */
export function formatPrice(n: number) {
	return `$${Number.isInteger(n) ? n : n.toFixed(2)}`;
}

/**
 * Relative labels worked out from the models being shown, not written by hand per model:
 * "Cheapest" and "Largest context". Needs at least three models to compare.
 */
export function modelHints(models: ModelInfo[]): Map<string, string[]> {
	const hints = new Map<string, string[]>();
	const add = (m: ModelInfo | undefined, text: string) => {
		if (m) hints.set(modelKey(m), [...(hints.get(modelKey(m)) ?? []), text]);
	};
	const priced = models.filter((m) => m.facts);
	if (priced.length >= 3) {
		const cost = (m: ModelInfo) =>
			(m.facts?.inputPrice ?? 0) + (m.facts?.outputPrice ?? 0);
		const cheapest = priced.reduce((a, b) => (cost(b) < cost(a) ? b : a));
		add(cheapest, "Cheapest");
	}
	const sized = models.filter((m) => m.inputTokenLimit);
	if (sized.length >= 3) {
		const most = Math.max(...sized.map((m) => m.inputTokenLimit ?? 0));
		const biggest = sized.filter((m) => m.inputTokenLimit === most);
		// A tie isn't a distinction.
		if (biggest.length === 1) add(biggest[0], "Largest context");
	}
	return hints;
}

function capabilities(m: ModelInfo): string[] {
	const f = m.facts;
	if (!f) return [];
	return [
		f.reasoning && "Reasoning",
		f.imageInput && "Images",
		f.pdfInput && "PDFs",
		f.toolCalls && "Tool use",
	].filter((c): c is string => !!c);
}

/** The text shown on hover: what doesn't fit in the row. */
export function modelDetails(m: ModelInfo): string {
	const f = m.facts;
	if (!f) return `${m.id}\nNo price data for this model.`;
	return [
		m.id,
		f.releaseDate && `Released ${f.releaseDate}`,
		f.knowledgeCutoff && `Knows about events up to ${f.knowledgeCutoff}`,
		f.cachedPrice != null &&
			`Cached input ${formatPrice(f.cachedPrice)} per 1M tokens`,
		capabilities(m).length > 0 && `Can use: ${capabilities(m).join(", ")}`,
		f.deprecated && "The provider has announced this model's retirement.",
	]
		.filter(Boolean)
		.join("\n");
}

/** One model in a picker: name, hints, then price and context on a quiet second line. */
export function ModelOption({
	model,
	hints,
}: {
	model: ModelInfo;
	hints: string[];
}) {
	const f = model.facts;
	const reasoning = f?.reasoning ? "Reasoning" : null;
	const line = [
		f && f.inputPrice != null && f.outputPrice != null
			? `${formatPrice(f.inputPrice)} in / ${formatPrice(f.outputPrice)} out per 1M`
			: "No price data",
		model.inputTokenLimit
			? `${formatTokens(model.inputTokenLimit)} context`
			: null,
	]
		.filter(Boolean)
		.join(" · ");
	return (
		<span className="flex min-w-0 flex-col" title={modelDetails(model)}>
			<span className="flex min-w-0 items-center gap-1.5">
				<span className="truncate">{model.displayName}</span>
				{[...hints, reasoning].filter(Boolean).map((h) => (
					<span
						key={h}
						className="shrink-0 rounded-full bg-muted px-1.5 py-px text-[11px] font-normal text-muted-foreground"
					>
						{h}
					</span>
				))}
			</span>
			<span className="truncate text-xs font-normal text-muted-foreground">
				{line}
			</span>
		</span>
	);
}

/** Toggle under a picker's list. Renders nothing when nothing is hidden. */
export function ShowAllButton({
	hidden,
	showAll,
	onToggle,
}: {
	hidden: number;
	showAll: boolean;
	onToggle: () => void;
}) {
	if (hidden <= 0 && !showAll) return null;
	return (
		<button
			type="button"
			onClick={onToggle}
			className="mt-1 w-full rounded-md px-2.5 py-1.5 text-left text-xs text-muted-foreground outline-none hover:bg-muted focus-visible:bg-muted"
		>
			{showAll ? "Show fewer models" : `Show all models (${hidden} more)`}
		</button>
	);
}
