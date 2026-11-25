import { CheckIcon, MinusIcon } from "@phosphor-icons/react";
import { Checkbox } from "react-aria-components";
import { cx, focus } from "./ui";

export function RowCheckbox({
	label,
	isSelected,
	isIndeterminate,
	onChange,
	className,
}: {
	label: string;
	isSelected: boolean;
	isIndeterminate?: boolean;
	onChange: (on: boolean) => void;
	/** Padding with a matching negative margin makes the click target bigger. */
	className?: string;
}) {
	return (
		<Checkbox
			aria-label={label}
			isSelected={isSelected}
			isIndeterminate={isIndeterminate}
			onChange={onChange}
			className={cx("group flex cursor-pointer items-center", focus, className)}
		>
			<span className="relative flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[indeterminate]:border-primary group-data-[selected]:border-primary group-data-[indeterminate]:bg-primary group-data-[selected]:bg-primary">
				<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
				<MinusIcon className="absolute size-4 opacity-0 group-data-[indeterminate]:opacity-100" />
			</span>
		</Checkbox>
	);
}
