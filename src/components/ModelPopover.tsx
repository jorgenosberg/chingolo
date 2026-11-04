import { useState } from "react";
import {
	Dialog,
	DialogTrigger,
	ListBox,
	ListBoxItem,
	Popover,
	Tooltip,
	TooltipTrigger,
} from "react-aria-components";
import type { ModelInfo } from "../lib/bindings";
import { CheckIcon, SparkleIcon } from "./icons";
import { formatTokens } from "./ModelSelect";
import { IconButton } from "./ui";

/** Small button that opens a popover to see and change the conversation's model. */
export function ModelPopover({
	models,
	value,
	onChange,
	isDisabled,
}: {
	models: ModelInfo[];
	value: string | null;
	onChange: (id: string) => void;
	isDisabled?: boolean;
}) {
	const [open, setOpen] = useState(false);
	// Keep a saved model visible even if the provider no longer lists it.
	const items: ModelInfo[] =
		value && !models.some((m) => m.id === value)
			? [
					{
						id: value,
						displayName: value,
						inputTokenLimit: null,
						outputTokenLimit: null,
					},
					...models,
				]
			: models;
	const current =
		items.find((m) => m.id === value)?.displayName ?? "none chosen";

	return (
		<DialogTrigger isOpen={open} onOpenChange={setOpen}>
			<TooltipTrigger delay={400} isDisabled={open}>
				<IconButton
					label={`Model: ${current}`}
					isDisabled={isDisabled}
					className="size-8 rounded-full"
				>
					<SparkleIcon />
				</IconButton>
				<Tooltip
					placement="top"
					offset={8}
					className="rounded-md bg-foreground px-2 py-1 text-xs text-background shadow-md"
				>
					{current}
				</Tooltip>
			</TooltipTrigger>
			<Popover
				placement="top end"
				offset={10}
				className="w-72 rounded-xl border border-border bg-card p-1.5 shadow-lg"
			>
				<Dialog aria-label="Model" className="outline-none">
					<p className="px-2.5 pb-1 pt-1 text-xs font-medium text-muted-foreground">
						Model
					</p>
					<ListBox
						aria-label="Models"
						selectionMode="single"
						disallowEmptySelection
						selectedKeys={value ? [value] : []}
						onSelectionChange={(keys) => {
							if (keys === "all") return;
							const id = [...keys][0];
							if (id === undefined || String(id) === value) return;
							// Close first: switching can open a confirmation dialog.
							setOpen(false);
							onChange(String(id));
						}}
						items={items}
						className="max-h-72 overflow-auto outline-none"
					>
						{(m) => (
							<ListBoxItem
								id={m.id}
								textValue={m.displayName}
								className="flex cursor-default items-center justify-between gap-3 rounded-md px-2.5 py-1.5 text-[13px] outline-none data-[focused]:bg-muted data-[selected]:font-medium"
							>
								{({ isSelected }) => (
									<>
										<span className="truncate">{m.displayName}</span>
										<span className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
											{m.inputTokenLimit
												? formatTokens(m.inputTokenLimit)
												: null}
											{isSelected && (
												<CheckIcon className="size-3.5 text-primary" />
											)}
										</span>
									</>
								)}
							</ListBoxItem>
						)}
					</ListBox>
				</Dialog>
			</Popover>
		</DialogTrigger>
	);
}
