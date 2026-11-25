import { useState } from "react";
import {
	Dialog,
	DialogTrigger,
	Popover,
	Button as RacButton,
} from "react-aria-components";
import { TAG_COLORS, TagDot } from "./TagChip";
import { cx, focus, POPOVER } from "./ui";

/** A tag's colour dot. Pressing it opens the standard colours, and picking one sets it. */
export function TagColorButton({
	color,
	label,
	onChange,
	isDisabled,
}: {
	color: string;
	/** Names the button for screen readers, like "Colour of Methods". */
	label: string;
	onChange: (color: string) => void;
	isDisabled?: boolean;
}) {
	const [open, setOpen] = useState(false);
	return (
		<DialogTrigger isOpen={open} onOpenChange={setOpen}>
			<RacButton
				aria-label={label}
				isDisabled={isDisabled}
				className={cx(
					"flex size-7 shrink-0 items-center justify-center rounded-md data-[hovered]:bg-muted data-[disabled]:opacity-60",
					focus,
				)}
			>
				<TagDot color={color} className="size-4" />
			</RacButton>
			<Popover placement="bottom start" className={`p-2 ${POPOVER}`}>
				<Dialog aria-label={label} className="outline-none">
					<div className="grid grid-cols-4 gap-1">
						{TAG_COLORS.map((c) => (
							<RacButton
								key={c}
								aria-label={c}
								aria-pressed={color === c}
								onPress={() => {
									onChange(c);
									setOpen(false);
								}}
								className={cx(
									"flex size-7 items-center justify-center rounded-md data-[hovered]:bg-muted",
									color === c && "bg-border/60",
									focus,
								)}
							>
								<TagDot color={c} className="size-4" />
							</RacButton>
						))}
					</div>
				</Dialog>
			</Popover>
		</DialogTrigger>
	);
}
