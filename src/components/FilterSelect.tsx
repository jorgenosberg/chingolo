import { CaretDownIcon } from "@phosphor-icons/react";
import {
	ListBox,
	ListBoxItem,
	Popover,
	Button as RacButton,
	Select,
	SelectValue,
} from "react-aria-components";
import { cx, focus, ITEM, POPOVER } from "./ui";

export type Choice = { id: string; name: string };

export function FilterSelect({
	label,
	value,
	items,
	onChange,
}: {
	label: string;
	value: string;
	items: Choice[];
	onChange: (id: string) => void;
}) {
	return (
		<Select
			aria-label={label}
			selectedKey={value}
			onSelectionChange={(k) => k && onChange(String(k))}
		>
			<RacButton
				className={cx(
					"flex h-8 items-center gap-2 rounded-lg border border-input bg-card px-2.5 text-[13px] data-[hovered]:bg-muted",
					focus,
				)}
			>
				<SelectValue className="max-w-48 truncate" />
				<CaretDownIcon className="size-4 shrink-0 text-muted-foreground" />
			</RacButton>
			<Popover
				className={`max-h-72 min-w-(--trigger-width) overflow-auto p-1 ${POPOVER}`}
			>
				<ListBox items={items} className="outline-none">
					{(i) => (
						<ListBoxItem
							id={i.id}
							textValue={i.name}
							className={`${ITEM} truncate data-[selected]:font-medium`}
						>
							{i.name}
						</ListBoxItem>
					)}
				</ListBox>
			</Popover>
		</Select>
	);
}
