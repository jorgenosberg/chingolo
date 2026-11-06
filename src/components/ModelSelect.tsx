import { CaretDownIcon } from "@phosphor-icons/react";
import {
	Button,
	Header,
	ListBox,
	ListBoxItem,
	ListBoxSection,
	Popover,
	Select,
	SelectValue,
} from "react-aria-components";
import type { ModelInfo } from "../lib/bindings";
import {
	groupModels,
	type ModelRef,
	modelKey,
	providerName,
	withSavedModel,
} from "../lib/providers";
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
	value: ModelRef | null;
	onChange: (model: ModelRef) => void;
	label: string;
	isDisabled?: boolean;
	placeholder?: string;
	className?: string;
}) {
	const items = withSavedModel(models, value);
	const groups = groupModels(items);

	return (
		<Select
			aria-label={label}
			selectedKey={value ? modelKey(value) : null}
			onSelectionChange={(k) => {
				const m = items.find((i) => modelKey(i) === String(k));
				if (m) onChange({ provider: m.provider, id: m.id });
			}}
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
				<CaretDownIcon className="size-4 shrink-0 text-muted-foreground" />
			</Button>
			<Popover className="max-h-72 min-w-(--trigger-width) overflow-auto rounded-xl border border-border bg-card p-1 shadow-lg">
				<ListBox className="outline-none">
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
									<span className="truncate">{m.displayName}</span>
									{m.inputTokenLimit ? (
										<span className="shrink-0 text-xs text-muted-foreground">
											{formatTokens(m.inputTokenLimit)}
										</span>
									) : null}
								</ListBoxItem>
							))}
						</ListBoxSection>
					))}
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

/**
 * For Gemini, the first "flash" model that isn't a lite or preview build. Other providers have
 * no naming rule we trust, so the user picks.
 */
export function pickDefaultModel(models: ModelInfo[]): ModelRef | null {
	const gemini = models.filter((m) => m.provider === "gemini");
	const found =
		gemini.find(
			(m) => /flash/i.test(m.id) && !/lite|preview|exp/i.test(m.id),
		) ??
		gemini.find((m) => /flash/i.test(m.id)) ??
		gemini[0];
	return found ? { provider: found.provider, id: found.id } : null;
}
