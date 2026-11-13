import { StarIcon } from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { Checkbox } from "react-aria-components";
import {
	formatPrice,
	formatTokens,
	modelDetails,
} from "../../components/ModelOption";
import { ModelSelect } from "../../components/ModelSelect";
import { Button, cx, focus, IconButton, TextInput } from "../../components/ui";
import { api } from "../../lib/api";
import type { ModelInfo } from "../../lib/bindings";
import {
	curateModels,
	groupModels,
	type ModelRef,
	modelKey,
	pickerModels,
	providerName,
} from "../../lib/providers";
import { keys, useModels, useSettings } from "../../lib/queries";

/** Default model, then every model with a star. Starred models are the only ones the pickers list. */
export function ModelsSection() {
	const qc = useQueryClient();
	const { data: settings } = useSettings();
	const hasKeys = (settings?.apiKeys.length ?? 0) > 0;
	const models = useModels(hasKeys);
	const [filter, setFilter] = useState("");
	const [showOlder, setShowOlder] = useState(false);

	const favorites = settings?.favoriteModels ?? [];
	const all = models.data?.models ?? [];

	const rows = useMemo(() => {
		const newest = pickerModels(all, [], null, true).items;
		const usual = new Set(curateModels(all).map(modelKey));
		const starred = new Set(favorites);
		const q = filter.trim().toLowerCase();
		return newest
			.filter(
				(m) => showOlder || usual.has(modelKey(m)) || starred.has(modelKey(m)),
			)
			.filter(
				(m) =>
					!q ||
					m.displayName.toLowerCase().includes(q) ||
					m.id.toLowerCase().includes(q),
			);
	}, [all, favorites, filter, showOlder]);

	const save = async (next: string[]) => {
		await api.updateSettings({ favoriteModels: next });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};
	const toggle = (m: ModelInfo) => {
		const key = modelKey(m);
		return save(
			favorites.includes(key)
				? favorites.filter((k) => k !== key)
				: [...favorites, key],
		);
	};

	const defaultModel: ModelRef | null = settings?.defaultModel
		? {
				provider: settings.defaultProvider ?? "gemini",
				id: settings.defaultModel,
			}
		: null;
	const setDefaultModel = async (m: ModelRef) => {
		await api.updateSettings({
			defaultModel: m.id,
			defaultProvider: m.provider,
		});
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	if (!hasKeys)
		return (
			<p className="text-sm text-muted-foreground">
				Add an API key on the General tab to see the models you can use.
			</p>
		);

	return (
		<div className="flex flex-col gap-6">
			<div className="flex flex-col gap-2.5">
				<h3 className="text-[13px] font-semibold">Default model</h3>
				<ModelSelect
					label="Default model"
					models={all}
					value={defaultModel}
					onChange={setDefaultModel}
				/>
			</div>

			<div className="flex flex-col gap-3">
				<div className="flex items-start justify-between gap-4">
					<div className="flex flex-col gap-1">
						<h3 className="text-[13px] font-semibold">Favorite models</h3>
						<p className="text-sm leading-relaxed text-muted-foreground">
							{favorites.length > 0
								? `${favorites.length} starred. Only these show in the chat and in the other model lists, with a "Show all models" button under each list.`
								: "Star the models you use. Until you do, the lists show a short default set of recent models."}
						</p>
					</div>
					{favorites.length > 0 && (
						<Button variant="ghost" onPress={() => save([])}>
							Clear stars
						</Button>
					)}
				</div>

				<TextInput
					label="Filter models"
					value={filter}
					onChange={setFilter}
					placeholder="Filter by name"
				/>
				<Checkbox
					isSelected={showOlder}
					onChange={setShowOlder}
					className={cx(
						"flex cursor-default items-center gap-2 text-[13px]",
						focus,
					)}
				>
					Include older, preview and retired models
				</Checkbox>

				{models.isError && (
					<p role="alert" className="text-sm text-problem">
						Couldn't load models: {models.error.message}
					</p>
				)}
				{models.data?.errors.map((e) => (
					<p key={e.provider} role="alert" className="text-sm text-problem">
						Couldn't load {providerName(e.provider)} models: {e.message}
					</p>
				))}

				{groupModels(rows).map((g) => (
					<div key={g.provider} className="flex flex-col">
						<p className="px-1 pb-1 pt-3 text-xs font-medium text-muted-foreground">
							{providerName(g.provider)}
						</p>
						<ul className="flex flex-col divide-y divide-border rounded-lg border border-border">
							{g.models.map((m) => {
								const starred = favorites.includes(modelKey(m));
								const f = m.facts;
								return (
									<li
										key={modelKey(m)}
										title={modelDetails(m)}
										className="grid grid-cols-[auto_1fr_auto] items-center gap-x-3 px-2 py-1.5 text-[13px]"
									>
										<IconButton
											label={
												starred
													? `Remove ${m.displayName} from favorites`
													: `Add ${m.displayName} to favorites`
											}
											onPress={() => toggle(m)}
											className={starred ? "text-primary" : undefined}
										>
											<StarIcon weight={starred ? "fill" : "regular"} />
										</IconButton>
										<span className="flex min-w-0 flex-col">
											<span className="truncate">{m.displayName}</span>
											<span className="truncate text-xs text-muted-foreground">
												{[
													f?.releaseDate,
													f?.reasoning ? "Reasoning" : null,
													f?.deprecated ? "Retiring" : null,
												]
													.filter(Boolean)
													.join(" · ") || m.id}
											</span>
										</span>
										<span className="text-right text-xs text-muted-foreground">
											{f && f.inputPrice != null && f.outputPrice != null
												? `${formatPrice(f.inputPrice)} / ${formatPrice(f.outputPrice)}`
												: "No price data"}
											{m.inputTokenLimit
												? ` · ${formatTokens(m.inputTokenLimit)}`
												: ""}
										</span>
									</li>
								);
							})}
						</ul>
					</div>
				))}
				{rows.length === 0 && models.data && (
					<p className="text-sm text-muted-foreground">No models match.</p>
				)}
				<p className="text-xs text-muted-foreground">
					Prices are US dollars per million tokens, input / output, from a
					public price list. Context is the most the model can read at once.
				</p>
			</div>
		</div>
	);
}
