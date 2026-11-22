import { CaretDownIcon } from "@phosphor-icons/react";
import { useMemo, useState } from "react";
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
	pickerModels,
	providerName,
} from "../lib/providers";
import { useSettings } from "../lib/queries";
import { ModelOption, modelHints, ShowAllButton } from "./ModelOption";
import { cx, focus, ITEM, POPOVER } from "./ui";

export function ModelSelect({
	models,
	value,
	onChange,
	label,
	isDisabled,
	placeholder = "Choose a model",
	className,
	all,
}: {
	models: ModelInfo[];
	value: ModelRef | null;
	onChange: (model: ModelRef) => void;
	label: string;
	isDisabled?: boolean;
	placeholder?: string;
	className?: string;
	/** List every model, with no favorites or curation (first-run setup). */
	all?: boolean;
}) {
	const [showAll, setShowAll] = useState(false);
	const { data: settings } = useSettings();
	const { items, hidden } = pickerModels(
		models,
		settings?.favoriteModels ?? [],
		value,
		showAll || !!all,
	);
	const groups = groupModels(items);
	const hints = useMemo(() => modelHints(items), [items]);

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
					"flex min-h-8 w-full items-center justify-between gap-2 rounded-lg border border-input bg-card px-2.5 py-1 text-left text-[13px]",
					"data-[hovered]:bg-muted data-[disabled]:opacity-40",
					focus,
				)}
			>
				<SelectValue className="truncate data-[placeholder]:text-muted-foreground" />
				<CaretDownIcon className="size-4 shrink-0 text-muted-foreground" />
			</Button>
			<Popover
				className={`max-h-80 min-w-(--trigger-width) overflow-auto p-1 ${POPOVER}`}
			>
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
									className={`${ITEM} flex items-center justify-between gap-3 data-[selected]:font-medium`}
								>
									<ModelOption model={m} hints={hints.get(modelKey(m)) ?? []} />
								</ListBoxItem>
							))}
						</ListBoxSection>
					))}
				</ListBox>
				{!all && (
					<ShowAllButton
						hidden={hidden}
						showAll={showAll}
						onToggle={() => setShowAll((v) => !v)}
					/>
				)}
			</Popover>
		</Select>
	);
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
