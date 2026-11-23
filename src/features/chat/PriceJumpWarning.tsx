import { WarningIcon } from "@phosphor-icons/react";
import { useState } from "react";
import {
	Dialog,
	DialogTrigger,
	Popover,
	Button as RacButton,
} from "react-aria-components";
import { formatTokens } from "../../components/ModelOption";
import { Button, cx, focus, POPOVER, Tip } from "../../components/ui";

/** Warn this long before the threshold, so the user can choose before the price changes. */
const WARN_AT = 0.85;

/**
 * Some models bill the whole request at a higher rate once the input passes a size. This shows
 * next to the context meter as that size gets close, and lets the user choose what to do.
 */
export function PriceJumpWarning({
	used,
	over,
	factor,
	onHandoff,
}: {
	used: number | null;
	over: number | null;
	factor: number | null;
	onHandoff: () => void;
}) {
	const [open, setOpen] = useState(false);
	if (!used || !over || !factor || factor <= 1 || used < over * WARN_AT) {
		return null;
	}
	const crossed = used > over;
	const times = `${Math.round(factor * 10) / 10}×`;
	const limit = formatTokens(over);
	return (
		<DialogTrigger isOpen={open} onOpenChange={setOpen}>
			<Tip label={`Input costs ${times} above ${limit} tokens`}>
				<RacButton
					aria-label={`Price warning: input costs ${times} above ${limit} tokens`}
					className={cx(
						"flex shrink-0 items-center gap-1 rounded-full px-1.5 py-1 text-xs font-medium text-problem",
						focus,
					)}
				>
					<WarningIcon className="size-4" />
					{crossed ? times : `Near ${limit}`}
				</RacButton>
			</Tip>
			<Popover placement="bottom end" className={`w-80 p-3 ${POPOVER}`}>
				<Dialog aria-label="Price warning" className="outline-none">
					<p className="text-sm font-medium">
						{crossed
							? `Past ${limit} tokens, input costs ${times} as much`
							: `Close to ${limit} tokens, where input costs ${times} as much`}
					</p>
					<p className="mt-1 text-[13px] text-muted-foreground">
						This conversation has about {formatTokens(used)} tokens. On this
						model, a request above {limit} tokens is billed at {times} the
						normal input price for the whole request.
						{crossed
							? " Every new message now pays that."
							: " Every message past that point pays it."}
					</p>
					<p className="mt-2 text-[13px] text-muted-foreground">
						To avoid it, hand off to a new conversation with a short note, or
						fork from an earlier message with the fork button on that message.
						Or keep going, it is your call.
					</p>
					<div className="mt-3 flex justify-end">
						<Button
							onPress={() => {
								setOpen(false);
								onHandoff();
							}}
						>
							Hand off to a new conversation
						</Button>
					</div>
				</Dialog>
			</Popover>
		</DialogTrigger>
	);
}
