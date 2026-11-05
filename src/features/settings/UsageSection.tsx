import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Radio, RadioGroup, Switch } from "react-aria-components";
import { Button, cx, focus, Spinner } from "../../components/ui";
import { api } from "../../lib/api";
import type { UsagePeriod, UsageRow } from "../../lib/bindings";

const PERIODS: { id: UsagePeriod; label: string }[] = [
	{ id: "day", label: "24 hours" },
	{ id: "week", label: "7 days" },
	{ id: "all", label: "All time" },
];

const KINDS: Record<string, string> = {
	chat: "Chat replies",
	handoff: "Hand-off notes",
	embed_index: "Indexing papers",
	embed_query: "Search queries",
};

const tokens = (n: number) =>
	n >= 1_000_000
		? `${(n / 1_000_000).toFixed(2)}M`
		: n >= 1_000
			? `${(n / 1_000).toFixed(1)}k`
			: String(n);

const money = (usd: number) =>
	usd === 0 ? "$0" : usd < 0.01 ? "<$0.01" : `$${usd.toFixed(2)}`;

function Cost({
	row,
}: {
	row: Pick<UsageRow, "costUsd" | "unpricedCalls" | "calls">;
}) {
	if (row.unpricedCalls === row.calls) {
		return (
			<span title="No price for this model in the price list">no price</span>
		);
	}
	return (
		<span
			title={
				row.unpricedCalls
					? `${row.unpricedCalls} calls have no price and are left out`
					: undefined
			}
		>
			{money(row.costUsd ?? 0)}
			{row.unpricedCalls > 0 && "+"}
		</span>
	);
}

/** What paid API calls used and roughly cost, from our own records. Google has no usage API for keys. */
export function UsageSection() {
	const qc = useQueryClient();
	const [period, setPeriod] = useState<UsagePeriod>("week");
	const [updating, setUpdating] = useState(false);
	const [updateError, setUpdateError] = useState<string | null>(null);
	const { data, isError, error } = useQuery({
		queryKey: ["usage", period],
		queryFn: () => api.getUsage(period),
	});

	const refreshUsage = () => qc.invalidateQueries({ queryKey: ["usage"] });
	const toggleAuto = async (on: boolean) => {
		setUpdateError(null);
		try {
			await api.setPricesAutoUpdate(on);
		} finally {
			await refreshUsage();
		}
	};
	const updateNow = async () => {
		setUpdating(true);
		setUpdateError(null);
		try {
			await api.updatePricesNow();
			await refreshUsage();
		} catch (e) {
			setUpdateError(e instanceof Error ? e.message : String(e));
		} finally {
			setUpdating(false);
		}
	};

	const rows = data?.rows ?? [];
	const total = rows.reduce((sum, r) => sum + (r.costUsd ?? 0), 0);

	return (
		<div className="flex flex-col gap-3">
			<RadioGroup
				aria-label="Usage period"
				orientation="horizontal"
				value={period}
				onChange={(v) => setPeriod(v as UsagePeriod)}
				className="inline-flex gap-1 self-start rounded-lg bg-muted p-1"
			>
				{PERIODS.map((p) => (
					<Radio
						key={p.id}
						value={p.id}
						className={cx(
							"cursor-default rounded-md px-3 py-1 text-[13px] text-muted-foreground",
							"data-[selected]:bg-card data-[selected]:font-medium data-[selected]:text-foreground data-[selected]:shadow-sm",
							focus,
						)}
					>
						{p.label}
					</Radio>
				))}
			</RadioGroup>

			{isError && (
				<p role="alert" className="text-sm text-problem">
					Couldn't load usage: {String((error as Error).message)}
				</p>
			)}
			{data && rows.length === 0 && (
				<p className="text-sm text-muted-foreground">
					No paid API calls in this period.
				</p>
			)}
			{rows.length > 0 && (
				<div className="overflow-x-auto">
					<table className="w-full text-left text-[13px]">
						<thead className="text-muted-foreground">
							<tr>
								<th className="py-1 pr-3 font-normal">What</th>
								<th className="py-1 pr-3 text-right font-normal">Calls</th>
								<th className="py-1 pr-3 text-right font-normal">In</th>
								<th className="py-1 pr-3 text-right font-normal">Out</th>
								<th className="py-1 pr-3 text-right font-normal">Avg ms</th>
								<th className="py-1 text-right font-normal">Cost</th>
							</tr>
						</thead>
						<tbody>
							{rows.map((r) => (
								<tr
									key={`${r.provider}/${r.model}/${r.kind}`}
									className="border-t border-border align-top"
								>
									<td className="py-1.5 pr-3">
										<div>{KINDS[r.kind] ?? r.kind}</div>
										<div className="text-muted-foreground">
											{r.model}
											{r.errors > 0 && (
												<span className="text-problem">
													{" "}
													· {r.errors} failed
												</span>
											)}
										</div>
									</td>
									<td className="py-1.5 pr-3 text-right tabular-nums">
										{r.calls.toLocaleString()}
									</td>
									<td className="py-1.5 pr-3 text-right tabular-nums">
										{r.estimated && "~"}
										{tokens(r.inputTokens)}
										{r.cachedTokens > 0 && (
											<div className="text-muted-foreground">
												{tokens(r.cachedTokens)} cached
											</div>
										)}
									</td>
									<td className="py-1.5 pr-3 text-right tabular-nums">
										{tokens(r.outputTokens)}
									</td>
									<td className="py-1.5 pr-3 text-right tabular-nums">
										{r.avgLatencyMs.toLocaleString()}
									</td>
									<td className="py-1.5 text-right tabular-nums">
										<Cost row={r} />
									</td>
								</tr>
							))}
						</tbody>
						<tfoot>
							<tr className="border-t border-border font-medium">
								<td className="py-1.5" colSpan={5}>
									Estimated total
								</td>
								<td className="py-1.5 text-right tabular-nums">
									{money(total)}
								</td>
							</tr>
						</tfoot>
					</table>
				</div>
			)}

			<p className="text-[13px] leading-relaxed text-muted-foreground">
				Estimated from token counts and prices from{" "}
				{data?.pricesSource ?? "the price list"}, last updated{" "}
				{data?.pricesFetched ?? "…"} (see below). "~" means a token count was
				guessed from text length. Your provider's own dashboard (AI Studio for
				Gemini, platform.openai.com for OpenAI) has the real bill.
			</p>
			<div className="flex flex-col gap-2 border-t border-border pt-3">
				<Switch
					isSelected={data?.pricesAutoUpdate ?? true}
					onChange={toggleAuto}
					className="group flex items-center gap-2 text-[13px]"
				>
					<span className="h-5 w-9 rounded-full bg-border p-0.5 transition-colors group-data-[focus-visible]:ring-2 group-data-[focus-visible]:ring-primary group-data-[selected]:bg-primary">
						<span className="block size-4 rounded-full bg-card shadow transition-transform group-data-[selected]:translate-x-4" />
					</span>
					Update prices automatically, about once a week
				</Switch>
				<div className="flex items-center gap-3">
					<Button onPress={updateNow} isDisabled={updating}>
						{updating && <Spinner />}
						Update prices now
					</Button>
					<span className="text-[13px] text-muted-foreground">
						Last updated {data?.pricesFetched ?? "…"}
					</span>
				</div>
				{updateError && (
					<p role="alert" className="text-[13px] text-problem">
						{updateError}
					</p>
				)}
			</div>
			<div>
				<Button onPress={() => api.showLogs()}>Show log folder</Button>
			</div>
		</div>
	);
}
