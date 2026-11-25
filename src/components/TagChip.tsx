import { XIcon } from "@phosphor-icons/react";
import type { CSSProperties } from "react";
import { cx } from "./ui";

/** The colours a tag can have. The names are the ones the backend accepts. */
export const TAG_COLORS = [
	"gray",
	"green",
	"blue",
	"purple",
	"pink",
	"red",
	"orange",
	"yellow",
] as const;

const tone = (color: string) =>
	({ "--tag": `var(--tag-${color})` }) as CSSProperties;

/** A round dot in a tag's colour. */
export function TagDot({
	color,
	className = "size-2.5",
}: {
	color: string;
	/** Sets the size. */
	className?: string;
}) {
	return (
		<span
			style={tone(color)}
			className={cx("block shrink-0 rounded-full bg-[var(--tag)]", className)}
		/>
	);
}

export function TagChip({
	name,
	color,
	onRemove,
	className,
}: {
	name: string;
	color: string;
	/** Shows a small button that takes the tag off. */
	onRemove?: () => void;
	className?: string;
}) {
	return (
		<span
			style={tone(color)}
			className={cx(
				"inline-flex h-5 max-w-40 shrink-0 items-center gap-1 rounded-full border border-[color-mix(in_oklch,var(--tag)_35%,transparent)] bg-[color-mix(in_oklch,var(--tag)_14%,transparent)] px-2 text-[11px] font-medium text-[var(--tag)]",
				className,
			)}
		>
			<span className="truncate">{name}</span>
			{onRemove && (
				<button
					type="button"
					aria-label={`Remove tag ${name}`}
					onClick={onRemove}
					className="-mr-1 rounded-full p-0.5 hover:bg-[color-mix(in_oklch,var(--tag)_20%,transparent)]"
				>
					<XIcon className="size-3" />
				</button>
			)}
		</span>
	);
}
