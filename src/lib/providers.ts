import type { ModelInfo } from "./bindings";

/** The AI providers the app can talk to. Ids match the Rust provider ids. */
export const PROVIDERS = [
	{
		id: "gemini",
		name: "Gemini",
		company: "Google",
		keyUrl: "aistudio.google.com/apikey",
	},
	{
		id: "openai",
		name: "OpenAI",
		company: "OpenAI",
		keyUrl: "platform.openai.com/api-keys",
	},
] as const;

export type Provider = (typeof PROVIDERS)[number];

export function providerInfo(id: string): Provider | undefined {
	return PROVIDERS.find((p) => p.id === id);
}

export function providerName(id: string): string {
	return providerInfo(id)?.name ?? id;
}

/** A model is identified by its provider and its id. */
export type ModelRef = { provider: string; id: string };

export const modelKey = (m: ModelRef) => `${m.provider}/${m.id}`;

export const sameModel = (a: ModelRef | null, b: ModelRef | null) =>
	!!a && !!b && a.provider === b.provider && a.id === b.id;

/** Keeps a saved model selectable even if its provider no longer lists it. */
export function withSavedModel(
	models: ModelInfo[],
	value: ModelRef | null,
): ModelInfo[] {
	if (!value || models.some((m) => sameModel(m, value))) return models;
	return [
		{
			provider: value.provider,
			id: value.id,
			displayName: value.id,
			inputTokenLimit: null,
			outputTokenLimit: null,
			facts: null,
		},
		...models,
	];
}

/** Models grouped by provider, in the order of `PROVIDERS`. */
export function groupModels(
	models: ModelInfo[],
): { provider: string; models: ModelInfo[] }[] {
	const order = (id: string) => {
		const i = PROVIDERS.findIndex((p) => p.id === id);
		return i < 0 ? PROVIDERS.length : i;
	};
	const groups = new Map<string, ModelInfo[]>();
	for (const m of models)
		groups.set(m.provider, [...(groups.get(m.provider) ?? []), m]);
	return [...groups]
		.sort(([a], [b]) => order(a) - order(b))
		.map(([provider, models]) => ({ provider, models }));
}
