import { PiggyBankIcon } from "@phosphor-icons/react";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Dialog, DialogTrigger, Popover } from "react-aria-components";
import { IconButton, Spinner } from "../../components/ui";
import { api } from "../../lib/api";
import { keys } from "../../lib/queries";
import { Cost, KINDS, money, tokens } from "../settings/UsageSection";

/** A dollar button that opens what this conversation has cost so far, from our own records. */
export function ConversationCost({
	conversationId,
}: {
	conversationId: string;
}) {
	const [open, setOpen] = useState(false);
	// Fetched each time the popover opens, so it includes the latest reply.
	const { data, isError, error, isFetching } = useQuery({
		queryKey: keys.conversationCost(conversationId),
		queryFn: () => api.getConversationCost(conversationId),
		enabled: open,
		staleTime: 0,
	});
	const rows = data ?? [];
	const total = rows.reduce((sum, r) => sum + (r.costUsd ?? 0), 0);
	const unpriced = rows.reduce((sum, r) => sum + r.unpricedCalls, 0);
	const calls = rows.reduce((sum, r) => sum + r.calls, 0);

	return (
		<DialogTrigger isOpen={open} onOpenChange={setOpen}>
			<IconButton label="Cost of this conversation">
				<PiggyBankIcon />
			</IconButton>
			<Popover
				placement="bottom end"
				offset={8}
				className="w-80 rounded-xl border border-border bg-card p-3 shadow-lg"
			>
				<Dialog aria-label="Cost of this conversation" className="outline-none">
					<p className="text-xs font-medium text-muted-foreground">
						This conversation
					</p>
					{isError ? (
						<p role="alert" className="mt-1 text-sm text-problem">
							Couldn't load the cost: {String((error as Error).message)}
						</p>
					) : !data ? (
						<div className="flex justify-center py-4">
							<Spinner />
						</div>
					) : rows.length === 0 ? (
						<p className="mt-1 text-sm text-muted-foreground">
							No cost recorded yet. Replies sent before costs were tracked per
							conversation aren't counted.
						</p>
					) : (
						<>
							<p className="mt-1 text-2xl font-semibold tabular-nums">
								{money(total)}
								{unpriced > 0 && "+"}
							</p>
							<p className="text-xs text-muted-foreground">
								Estimated, {calls} {calls === 1 ? "call" : "calls"}
								{isFetching ? " · updating" : ""}
							</p>
							<table className="mt-3 w-full text-left text-[13px]">
								<tbody>
									{rows.map((r) => (
										<tr
											key={`${r.provider}/${r.model}/${r.kind}`}
											className="border-t border-border align-top"
										>
											<td className="py-1.5 pr-2">
												<div>{KINDS[r.kind] ?? r.kind}</div>
												<div className="text-xs text-muted-foreground">
													{r.model} · {tokens(r.inputTokens)} in
													{r.cachedTokens > 0
														? ` (${tokens(r.cachedTokens)} cached)`
														: ""}
													, {tokens(r.outputTokens)} out
												</div>
											</td>
											<td className="py-1.5 text-right tabular-nums">
												<Cost row={r} />
											</td>
										</tr>
									))}
								</tbody>
							</table>
							<p className="mt-2 text-xs text-muted-foreground">
								Search queries and replies sent before this was tracked aren't
								included. Settings → Usage has the full picture.
							</p>
						</>
					)}
				</Dialog>
			</Popover>
		</DialogTrigger>
	);
}
