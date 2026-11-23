import { Button as RacButton } from "react-aria-components";
import { formatTokens } from "../../components/ModelOption";
import { cx, focus, Tip } from "../../components/ui";

const SIZE = 18;
const STROKE = 2.5;
const RADIUS = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/**
 * How much of the model's context window the conversation fills, from the size of the last reply's
 * input. Hidden until there is a reply and the model's limit is known.
 */
export function ContextMeter({
	used,
	limit,
	cached,
}: {
	used: number | null;
	limit: number | null;
	/** Cached input tokens of the last reply, when the provider reported any. */
	cached: number | null;
}) {
	if (!used || !limit) return null;
	const share = Math.min(used / limit, 1);
	const percent = share < 0.01 ? "<1" : String(Math.round(share * 100));
	const high = share >= 0.8;
	const label = [
		`${formatTokens(used)} of ${formatTokens(limit)} tokens used (${percent}%)`,
		cached ? `${formatTokens(cached)} of it was cached` : null,
		high
			? "Hand off to a new conversation from the ⋯ menu to start fresh"
			: null,
	]
		.filter(Boolean)
		.join(". ");
	return (
		<Tip label={label}>
			<RacButton
				aria-label={`Context ${percent}% full`}
				className={cx(
					"flex shrink-0 items-center gap-1 rounded-full px-1 py-1 text-xs tabular-nums",
					high ? "text-problem" : "text-muted-foreground",
					focus,
				)}
			>
				<svg
					width={SIZE}
					height={SIZE}
					viewBox={`0 0 ${SIZE} ${SIZE}`}
					className="-rotate-90"
					aria-hidden="true"
				>
					<circle
						cx={SIZE / 2}
						cy={SIZE / 2}
						r={RADIUS}
						fill="none"
						stroke="currentColor"
						strokeOpacity={0.2}
						strokeWidth={STROKE}
					/>
					<circle
						cx={SIZE / 2}
						cy={SIZE / 2}
						r={RADIUS}
						fill="none"
						stroke="currentColor"
						strokeWidth={STROKE}
						strokeLinecap="round"
						strokeDasharray={CIRCUMFERENCE}
						strokeDashoffset={CIRCUMFERENCE * (1 - share)}
					/>
				</svg>
				{percent}%
			</RacButton>
		</Tip>
	);
}
