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

const MAX_AGE_MS = 183 * 24 * 3600 * 1000;
const PREVIEW_ID = /(^|[-_.])(preview|exp|experimental)($|[-_.])/;
const LATEST_ID = /-latest$/;
/** A date or build number on the end of an id: `-2025-06-01`, `-20250601`, `-0613`, `-001`. */
const SNAPSHOT_SUFFIX = /-(\d{4}-\d{2}-\d{2}|\d{8}|\d{3,4})$/;

const releaseTime = (m: ModelInfo) => {
	const t = m.facts?.releaseDate ? Date.parse(m.facts.releaseDate) : Number.NaN;
	return Number.isNaN(t) ? null : t;
};

/** Newest release first, models with no date last. */
function newestFirst(models: ModelInfo[]): ModelInfo[] {
	return [...models].sort((a, b) => {
		const [ta, tb] = [releaseTime(a), releaseTime(b)];
		if (ta !== tb) return (tb ?? -Infinity) - (ta ?? -Infinity);
		return a.displayName.localeCompare(b.displayName);
	});
}

/**
 * The short default list, used until the user stars models. Works from the data the price table
 * gives and the shape of ids, never from model names. Hides, per provider: retired models,
 * previews, "-latest" aliases, dated copies of a model that is listed without the date, models
 * with no price (when the provider has priced ones), and anything over about six months older than the
 * provider's newest model. A provider is never emptied.
 */
export function curateModels(models: ModelInfo[]): ModelInfo[] {
	const byProvider = new Map<string, ModelInfo[]>();
	for (const m of models)
		byProvider.set(m.provider, [...(byProvider.get(m.provider) ?? []), m]);
	const out: ModelInfo[] = [];
	for (const group of byProvider.values()) {
		const ids = new Set(group.map((m) => m.id));
		const anyPriced = group.some((m) => m.facts);
		const live = group.filter((m) => !m.facts?.deprecated);
		const isDatedCopy = (id: string) => {
			const base = id.replace(SNAPSHOT_SUFFIX, "");
			return base !== id && ids.has(base);
		};
		let kept = live.filter(
			(m) =>
				(!anyPriced || m.facts) &&
				!PREVIEW_ID.test(m.id) &&
				!LATEST_ID.test(m.id) &&
				!isDatedCopy(m.id),
		);
		const newest = Math.max(...kept.map((m) => releaseTime(m) ?? -Infinity));
		if (Number.isFinite(newest))
			kept = kept.filter((m) => {
				const t = releaseTime(m);
				return t === null || newest - t <= MAX_AGE_MS;
			});
		out.push(...(kept.length ? kept : live.length ? live : group));
	}
	return out;
}

/**
 * What a model picker lists. The user's favorites if there are any, else the curated default,
 * or every model when `showAll`. The current value is always included (with its real details when
 * the provider still lists it), so a conversation never loses its model. `hidden` counts the
 * listed models left out.
 */
export function pickerModels(
	models: ModelInfo[],
	favorites: string[],
	value: ModelRef | null,
	showAll: boolean,
): { items: ModelInfo[]; hidden: number } {
	const rank = new Map(favorites.map((k, i) => [k, i]));
	let base: ModelInfo[];
	if (showAll) base = newestFirst(models);
	else {
		base = models
			.filter((m) => rank.has(modelKey(m)))
			.sort(
				(a, b) => (rank.get(modelKey(a)) ?? 0) - (rank.get(modelKey(b)) ?? 0),
			);
		// No favorites, or none of them are listed any more (a key was removed): the default list.
		if (base.length === 0) base = newestFirst(curateModels(models));
	}
	const hidden = models.length - base.length;
	if (value && !base.some((m) => sameModel(m, value))) {
		const listed = models.find((m) => sameModel(m, value));
		base = [
			listed ?? {
				provider: value.provider,
				id: value.id,
				displayName: value.id,
				inputTokenLimit: null,
				outputTokenLimit: null,
				facts: null,
			},
			...base,
		];
		return { items: base, hidden: listed ? hidden - 1 : hidden };
	}
	return { items: base, hidden };
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
