import { useState } from "react";
import {
	Dialog,
	DialogTrigger,
	Header,
	ListBox,
	ListBoxItem,
	ListBoxSection,
	Popover,
	Tooltip,
	TooltipTrigger,
} from "react-aria-components";
import type { ModelInfo, ProviderError } from "../lib/bindings";
import {
	groupModels,
	type ModelRef,
	modelKey,
	providerName,
	sameModel,
	withSavedModel,
} from "../lib/providers";
import { CheckIcon, SparkleIcon } from "./icons";
import { formatTokens } from "./ModelSelect";
import { IconButton } from "./ui";

/** Small button that opens a popover to see and change the conversation's model. */
export function ModelPopover({
	models,
	errors = [],
	value,
	onChange,
	isDisabled,
}: {
	models: ModelInfo[];
	/** Providers whose model list couldn't be loaded. */
	errors?: ProviderError[];
	value: ModelRef | null;
	onChange: (model: ModelRef) => void;
	isDisabled?: boolean;
}) {
	const [open, setOpen] = useState(false);
	const items = withSavedModel(models, value);
	const groups = groupModels(items);
	const current =
		items.find((m) => sameModel(m, value))?.displayName ?? "none chosen";

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
						selectedKeys={value ? [modelKey(value)] : []}
						onSelectionChange={(keys) => {
							if (keys === "all") return;
							const key = [...keys][0];
							const m = items.find((i) => modelKey(i) === String(key));
							if (!m || sameModel(m, value)) return;
							// Close first: switching can open a confirmation dialog.
							setOpen(false);
							onChange({ provider: m.provider, id: m.id });
						}}
						className="max-h-72 overflow-auto outline-none"
					>
						{groups.map((g) => (
							<ListBoxSection
								key={g.provider}
								aria-label={providerName(g.provider)}
							>
								{groups.length > 1 && (
									<Header className="px-2.5 pb-1 pt-2 text-xs font-medium text-muted-foreground">
										{providerName(g.provider)}
									</Header>
								)}
								{g.models.map((m) => (
									<ListBoxItem
										key={modelKey(m)}
										id={modelKey(m)}
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
								))}
							</ListBoxSection>
						))}
					</ListBox>
					{errors.map((e) => (
						<p
							key={e.provider}
							role="alert"
							className="px-2.5 pb-1 pt-2 text-xs text-problem"
						>
							Couldn't load {providerName(e.provider)} models: {e.message}
						</p>
					))}
				</Dialog>
			</Popover>
		</DialogTrigger>
	);
}
