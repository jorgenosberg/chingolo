import {
	Button,
	ListBox,
	ListBoxItem,
	Popover,
	Select,
	SelectValue,
} from "react-aria-components";
import type { ModelInfo } from "../lib/bindings";
import { ChevronIcon } from "./icons";
import { cx, focus } from "./ui";

export function ModelSelect({
	models,
	value,
	onChange,
	label,
	isDisabled,
	placeholder = "Choose a model",
	className,
}: {
	models: ModelInfo[];
	value: string | null;
	onChange: (id: string) => void;
	label: string;
	isDisabled?: boolean;
	placeholder?: string;
	className?: string;
}) {
	// Keep a saved model selectable even if the provider no longer lists it.
	const items =
		value && !models.some((m) => m.id === value)
			? [{ id: value, displayName: value } as ModelInfo, ...models]
			: models;

	return (
		<Select
			aria-label={label}
			selectedKey={value}
			onSelectionChange={(k) => k && onChange(String(k))}
			isDisabled={isDisabled}
			placeholder={placeholder}
			className={className}
		>
			<Button
				className={cx(
					"flex h-8 w-full items-center justify-between gap-2 rounded-lg border border-input bg-card px-2.5 text-[13px]",
					"data-[hovered]:bg-muted data-[disabled]:opacity-40",
					focus,
				)}
			>
				<SelectValue className="truncate data-[placeholder]:text-muted-foreground" />
				<ChevronIcon className="size-3.5 shrink-0 rotate-90 text-muted-foreground" />
			</Button>
			<Popover className="max-h-72 min-w-(--trigger-width) overflow-auto rounded-xl border border-border bg-card p-1 shadow-lg">
				<ListBox items={items} className="outline-none">
					{(m) => (
						<ListBoxItem
							id={m.id}
							textValue={m.displayName}
							className="flex cursor-default items-center justify-between gap-3 rounded-md px-2.5 py-1.5 text-[13px] outline-none data-[focused]:bg-muted data-[selected]:font-medium"
						>
							<span className="truncate">{m.displayName}</span>
							{m.inputTokenLimit ? (
								<span className="shrink-0 text-xs text-muted-foreground">
									{formatTokens(m.inputTokenLimit)}
								</span>
							) : null}
						</ListBoxItem>
					)}
				</ListBox>
			</Popover>
		</Select>
	);
}

export function formatTokens(n: number) {
	return n >= 1_000_000
		? `${+(n / 1_000_000).toFixed(1)}M`
		: `${Math.round(n / 1000)}k`;
}

/** First "flash" model that isn't a lite or preview build, else the first model. */
export function pickDefaultModel(models: ModelInfo[]): string | null {
	const plain = models.find(
		(m) => /flash/i.test(m.id) && !/lite|preview|exp/i.test(m.id),
	);
	return (
		(plain ?? models.find((m) => /flash/i.test(m.id)) ?? models[0])?.id ?? null
	);
}
