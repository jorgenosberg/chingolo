import {
	ArrowClockwiseIcon,
	ArrowLeftIcon,
	ChartBarIcon,
	CheckIcon,
	KeyIcon,
	LockSimpleIcon,
	MagnifyingGlassIcon,
	PauseIcon,
	PlayIcon,
	SlidersHorizontalIcon,
	TrashIcon,
} from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import {
	Checkbox,
	Radio,
	RadioGroup,
	Tab,
	TabList,
	TabPanel,
	Tabs,
} from "react-aria-components";
import { useNav } from "../../app/nav";
import { useDialogs } from "../../components/dialogs";
import { ModelSelect } from "../../components/ModelSelect";
import { Button, cx, focus, Spinner, TextInput } from "../../components/ui";
import { api } from "../../lib/api";
import type { ModelState } from "../../lib/bindings";
import { useEmbeddingStatus } from "../../lib/library";
import {
	type ModelRef,
	PROVIDERS,
	type Provider,
	providerName,
} from "../../lib/providers";
import { keys, useModels, useSettings } from "../../lib/queries";
import { setTheme, type ThemePref, useTheme } from "../../lib/theme";
import { mb } from "../library/EmbeddingNotice";
import { UsageSection } from "./UsageSection";

const THEMES: { id: ThemePref; label: string }[] = [
	{ id: "system", label: "System" },
	{ id: "light", label: "Light" },
	{ id: "dark", label: "Dark" },
];

const TAB_KEY = "chingui.settingsTab";
const TABS = ["general", "search", "usage", "privacy"];

function savedTab(): string {
	try {
		const t = localStorage.getItem(TAB_KEY);
		return t && TABS.includes(t) ? t : "general";
	} catch {
		return "general";
	}
}

const tabClass = cx(
	"-mb-px flex cursor-default items-center gap-1.5 border-b-2 border-transparent px-3 py-2 text-[13px] text-muted-foreground",
	"data-[hovered]:text-foreground data-[selected]:border-foreground data-[selected]:font-medium data-[selected]:text-foreground",
	focus,
);

function Section({
	title,
	children,
}: {
	title: string;
	children: React.ReactNode;
}) {
	return (
		<section className="flex flex-col gap-2.5 border-t border-border py-6 first:border-t-0 first:pt-2">
			<h3 className="text-[13px] font-semibold">{title}</h3>
			{children}
		</section>
	);
}

type BackendId = "gemini" | "local";

const BACKENDS: {
	id: BackendId;
	title: string;
	tag: string;
	blurb: string;
	note: string;
}[] = [
	{
		id: "gemini",
		title: "Gemini",
		tag: "Online",
		blurb:
			"Quick, and light on this Mac. Passages of your papers go to Google to be indexed, using your API key.",
		note: "Costs well under a dollar for a few hundred papers.",
	},
	{
		id: "local",
		title: "On this Mac",
		tag: "Private",
		blurb:
			"Nothing leaves your computer. Needs a one-time 200 MB download from Hugging Face.",
		note: "Indexing is slower and uses more of this Mac's processor.",
	},
];

function ProgressBar({
	value,
	max,
	label,
}: {
	value: number;
	max: number;
	label: string;
}) {
	return (
		<div
			role="progressbar"
			aria-label={label}
			aria-valuemin={0}
			aria-valuemax={max}
			aria-valuenow={value}
			className="h-1.5 w-full overflow-hidden rounded-full bg-border"
		>
			<div
				className="h-full bg-primary transition-[width]"
				style={{ width: `${max ? Math.min(100, (value / max) * 100) : 0}%` }}
			/>
		</div>
	);
}

/** The text recognition models that read scanned pages. They stay on this Mac. */
function ScannedPages() {
	const [model, setModel] = useState<ModelState | null>(null);
	useEffect(() => {
		api.ocrStatus().then(setModel);
	}, []);
	const downloading = model?.kind === "downloading";
	useEffect(() => {
		if (!downloading) return;
		const timer = setInterval(() => api.ocrStatus().then(setModel), 1000);
		return () => clearInterval(timer);
	}, [downloading]);
	if (!model) return null;
	const start = () => api.downloadOcrModel().then(setModel);

	return (
		<div
			className="flex flex-col gap-2.5 rounded-xl border border-border bg-muted/40 p-4"
			aria-live="polite"
		>
			<div className="flex items-center justify-between gap-3">
				<div className="text-[13px] font-medium">
					Text recognition
					<span className="font-normal text-muted-foreground">
						{" "}
						·{" "}
						{model.kind === "ready"
							? "ready"
							: model.kind === "downloading"
								? "downloading"
								: model.kind === "failed"
									? "not working"
									: "not downloaded"}
					</span>
				</div>
				{model.kind === "failed" && (
					<Button variant="primary" onPress={start}>
						<ArrowClockwiseIcon className="size-4" />
						Try again
					</Button>
				)}
				{model.kind === "missing" && (
					<Button variant="primary" onPress={start}>
						Download 31 MB
					</Button>
				)}
			</div>
			{model.kind === "downloading" && (
				<ProgressBar
					value={model.done ?? 0}
					max={model.total ?? 0}
					label="Text recognition download"
				/>
			)}
			<p className="text-[13px] text-muted-foreground">
				{model.kind === "ready"
					? "Pages without text, like scans, are read automatically when a paper is added. To read a paper you added earlier, use Read again in the library."
					: model.kind === "failed"
						? model.message
						: "Reads pages that have no text, like scans. It runs on this Mac and nothing is sent anywhere. Needs a one-time 31 MB download from GitHub."}
			</p>
		</div>
	);
}

/** Lets the chat model put the best of the top search results first. Sends those passages to its provider. */
function SharpenResults() {
	const { data: settings } = useSettings();
	const qc = useQueryClient();
	const hasKeys = (settings?.apiKeys.length ?? 0) > 0;
	const models = useModels(hasKeys);
	if (!settings) return null;
	const toggle = async (on: boolean) => {
		await api.updateSettings({ rerankEnabled: on });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};
	const chosen: ModelRef | null =
		settings.rerankModel && settings.rerankProvider
			? { provider: settings.rerankProvider, id: settings.rerankModel }
			: null;
	const choose = async (m: ModelRef | null) => {
		await api.updateSettings({
			rerankModel: m?.id ?? "",
			rerankProvider: m?.provider ?? "",
		});
		await qc.invalidateQueries({ queryKey: keys.settings });
	};
	return (
		<div className="flex flex-col gap-2">
			<Checkbox
				isSelected={settings.rerankEnabled}
				onChange={toggle}
				className={cx(
					"group flex cursor-default items-center gap-2.5 text-[13px]",
					focus,
				)}
			>
				<span className="flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[selected]:border-primary group-data-[selected]:bg-primary">
					<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
				</span>
				Sharpen search results with the chat model
			</Checkbox>
			<p className="text-sm leading-relaxed text-muted-foreground">
				When the model searches your library, it first reads the 20 best matches
				and puts the most relevant first. This finds the right passage more
				often, especially for Norwegian papers. The 20 passages go to the
				provider of the model you chat with, using your API key, and each search
				takes a few seconds longer and costs a little more. A small, fast model
				is enough and keeps both down.
			</p>
			{settings.rerankEnabled && (
				<div className="flex items-center gap-2">
					<ModelSelect
						label="Model for sharpening"
						models={models.data?.models ?? []}
						value={chosen}
						onChange={choose}
						isDisabled={!hasKeys}
						placeholder="Same model as the chat"
					/>
					{chosen && (
						<Button variant="ghost" onPress={() => choose(null)}>
							Use the chat model
						</Button>
					)}
				</div>
			)}
		</div>
	);
}

/** Looks a paper's DOI up in Crossref for the exact title, authors, year and journal. Sends the DOI only. */
function DetailsLookup() {
	const { data: settings } = useSettings();
	const qc = useQueryClient();
	if (!settings) return null;
	const toggle = async (on: boolean) => {
		await api.updateSettings({ crossrefEnabled: on });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};
	return (
		<div className="flex flex-col gap-2">
			<Checkbox
				isSelected={settings.crossrefEnabled}
				onChange={toggle}
				className={cx(
					"group flex cursor-default items-center gap-2.5 text-[13px]",
					focus,
				)}
			>
				<span className="flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[selected]:border-primary group-data-[selected]:bg-primary">
					<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
				</span>
				Look up papers in Crossref
			</Checkbox>
			<p className="text-sm leading-relaxed text-muted-foreground">
				For a paper with a DOI, Chingui asks Crossref (api.crossref.org) for the
				exact title, authors, year and journal, and uses them only if the title
				is found in the paper itself. Only the DOI is sent, never your text or
				your keys. Papers you already added are updated with Refresh all details
				in the library. Details you edited are kept.
			</p>
		</div>
	);
}

/** Lets a model read the first pages of papers whose title, authors or year are missing. Sends those pages to its provider. */
function DetailsModel() {
	const { data: settings } = useSettings();
	const qc = useQueryClient();
	const hasKeys = (settings?.apiKeys.length ?? 0) > 0;
	const models = useModels(hasKeys);
	if (!settings) return null;
	const toggle = async (on: boolean) => {
		await api.updateSettings({ metadataAiEnabled: on });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};
	const chosen: ModelRef | null =
		settings.metadataModel && settings.metadataProvider
			? { provider: settings.metadataProvider, id: settings.metadataModel }
			: null;
	const choose = async (m: ModelRef | null) => {
		await api.updateSettings({
			metadataModel: m?.id ?? "",
			metadataProvider: m?.provider ?? "",
		});
		await qc.invalidateQueries({ queryKey: keys.settings });
	};
	return (
		<div className="flex flex-col gap-2">
			<Checkbox
				isSelected={settings.metadataAiEnabled}
				onChange={toggle}
				className={cx(
					"group flex cursor-default items-center gap-2.5 text-[13px]",
					focus,
				)}
			>
				<span className="flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[selected]:border-primary group-data-[selected]:bg-primary">
					<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
				</span>
				Fill in missing details with a model
			</Checkbox>
			<p className="text-sm leading-relaxed text-muted-foreground">
				For a paper or report whose title, authors or year are still missing,
				the text of its first pages goes to the provider of the model below,
				using your API key, and costs a little. The model's answer is kept only
				where the same words are printed in the paper, and never replaces
				something you edited. Papers you already added are covered by Refresh
				all details in the library.
			</p>
			{settings.metadataAiEnabled && (
				<div className="flex items-center gap-2">
					<ModelSelect
						label="Model for details"
						models={models.data?.models ?? []}
						value={chosen}
						onChange={choose}
						isDisabled={!hasKeys}
						placeholder="Same model as new chats"
					/>
					{chosen && (
						<Button variant="ghost" onPress={() => choose(null)}>
							Use the default model
						</Button>
					)}
				</div>
			)}
		</div>
	);
}

/** Two option cards for the model behind meaning search, and a status panel for the chosen one. */
function SearchModel() {
	const status = useEmbeddingStatus();
	const { data: settings } = useSettings();
	const { confirm } = useDialogs();
	const [error, setError] = useState<string | null>(null);
	if (!status) return null;
	const { backend, model, indexed, total, paused } = status;
	const active = BACKENDS.find((b) => b.id === backend) ?? BACKENDS[0];
	const fail = (e: { message?: string }) => setError(String(e.message ?? e));

	const start = () => {
		setError(null);
		(backend === "gemini"
			? api.allowCloudEmbedding()
			: api.downloadEmbeddingModel()
		).catch(fail);
	};

	const choose = async (next: string) => {
		const id = next as BackendId;
		if (id === backend) return;
		setError(null);
		if (total > 0) {
			const ok = await confirm({
				title: `Switch to ${BACKENDS.find((b) => b.id === id)?.title}?`,
				message:
					id === "gemini"
						? `Your ${total.toLocaleString()} passages will be indexed again with Gemini. They are sent to Google to do that. Keyword search keeps working meanwhile.`
						: `Your ${total.toLocaleString()} passages will be indexed again on this Mac, after a one-time 200 MB download. Keyword search keeps working meanwhile.`,
				confirmLabel: "Switch",
			});
			if (!ok) return;
		}
		api.setEmbeddingBackend(id).catch(fail);
	};

	const needsKey =
		backend === "gemini" && !settings?.apiKeys.includes("gemini");
	const indexing = model.kind === "ready" && indexed < total;

	return (
		<div className="flex flex-col gap-3">
			<RadioGroup
				aria-label="Search model"
				value={backend}
				onChange={choose}
				className="grid gap-3 sm:grid-cols-2"
			>
				{BACKENDS.map((b) => (
					<Radio
						key={b.id}
						value={b.id}
						className={cx(
							"group flex cursor-default flex-col gap-2 rounded-xl border border-border bg-card p-4 transition-colors",
							"data-[hovered]:border-foreground/30 data-[selected]:border-primary data-[selected]:bg-primary/5",
							"data-[focus-visible]:ring-2 data-[focus-visible]:ring-primary/60",
						)}
					>
						<div className="flex items-center gap-2">
							<span className="text-sm font-semibold">{b.title}</span>
							<span className="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
								{b.tag}
							</span>
							<span className="ml-auto grid size-5 place-items-center rounded-full border border-border text-primary-foreground group-data-[selected]:border-primary group-data-[selected]:bg-primary">
								<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
							</span>
						</div>
						<p className="text-[13px] leading-relaxed">{b.blurb}</p>
						<p className="text-[13px] leading-relaxed text-muted-foreground">
							{b.note}
						</p>
					</Radio>
				))}
			</RadioGroup>

			<div
				className="flex flex-col gap-2.5 rounded-xl border border-border bg-muted/40 p-4"
				aria-live="polite"
			>
				<div className="flex items-center justify-between gap-3">
					<div className="text-[13px] font-medium">
						{active.title}
						<span className="font-normal text-muted-foreground">
							{" "}
							·{" "}
							{model.kind === "ready"
								? indexing
									? paused
										? "indexing paused"
										: "indexing"
									: "ready"
								: model.kind === "downloading"
									? "downloading"
									: model.kind === "failed"
										? "not working"
										: "not set up"}
						</span>
					</div>
					{indexing && (
						<Button onPress={() => api.setEmbeddingPaused(!paused)}>
							{paused ? (
								<PlayIcon className="size-4" />
							) : (
								<PauseIcon className="size-4" />
							)}
							{paused ? "Resume" : "Pause"}
						</Button>
					)}
					{model.kind === "failed" && (
						<Button variant="primary" onPress={start}>
							<ArrowClockwiseIcon className="size-4" />
							Try again
						</Button>
					)}
					{model.kind === "missing" && (
						<Button variant="primary" onPress={start} isDisabled={needsKey}>
							{backend === "gemini" ? "Allow and index" : "Download 200 MB"}
						</Button>
					)}
				</div>

				{model.kind === "ready" && (
					<>
						{total > 0 && (
							<ProgressBar
								value={indexed}
								max={total}
								label="Indexed passages"
							/>
						)}
						<p className="text-[13px] text-muted-foreground">
							{total === 0
								? "Ready. Add papers to the library and they are indexed for meaning search."
								: `${indexed.toLocaleString()} of ${total.toLocaleString()} passages are indexed for meaning search.`}
						</p>
					</>
				)}
				{model.kind === "downloading" && (
					<>
						<ProgressBar
							value={model.done ?? 0}
							max={model.total ?? 0}
							label="Search model download"
						/>
						<p className="text-[13px] text-muted-foreground">
							Downloading the search model: {mb(model.done)} of{" "}
							{mb(model.total)}. Keyword search works meanwhile.
						</p>
					</>
				)}
				{model.kind === "missing" && (
					<p className="text-[13px] text-muted-foreground">
						{backend === "gemini"
							? "Not turned on yet. Allowing it sends passages of your papers to Gemini to be indexed. Keyword search works without it."
							: "Not downloaded yet. Keyword search works without it."}
						{needsKey && " Add a Gemini API key on the General tab first."}
					</p>
				)}
				{model.kind === "failed" && (
					<p role="alert" className="text-[13px] text-problem">
						{model.message}
					</p>
				)}
				{error && (
					<p role="alert" className="text-[13px] text-problem">
						{error}
					</p>
				)}
			</div>
		</div>
	);
}

/** Key entry for one provider. The key is checked by listing models before it is saved. */
function ApiKeySection({ provider }: { provider: Provider }) {
	const qc = useQueryClient();
	const { confirm } = useDialogs();
	const { data: settings } = useSettings();
	const hasKey = settings?.apiKeys.includes(provider.id) ?? false;
	const [replacing, setReplacing] = useState(false);
	const [key, setKey] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const saveKey = async () => {
		setBusy(true);
		setError(null);
		try {
			await api.saveApiKey(provider.id, key);
			await qc.invalidateQueries({ queryKey: keys.settings });
			await qc.invalidateQueries({ queryKey: keys.models });
			setKey("");
			setReplacing(false);
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
		} finally {
			setBusy(false);
		}
	};

	const removeKey = async () => {
		const ok = await confirm({
			title: `Remove the ${provider.name} key?`,
			message: `The key is deleted from the macOS Keychain. Your chats stay, but you can't send messages with ${provider.name} models until you add a key again.`,
			confirmLabel: "Remove key",
			danger: true,
		});
		if (!ok) return;
		await api.deleteApiKey(provider.id);
		await qc.invalidateQueries({ queryKey: keys.settings });
		await qc.invalidateQueries({ queryKey: keys.models });
	};

	return (
		<Section title={`${provider.name} API key`}>
			{hasKey && !replacing ? (
				<div className="flex items-center justify-between gap-3">
					<p className="text-sm text-muted-foreground">
						Saved in the macOS Keychain.
					</p>
					<div className="flex gap-2">
						<Button onPress={() => setReplacing(true)}>
							<KeyIcon className="size-4" />
							Replace
						</Button>
						<Button variant="ghost" onPress={removeKey}>
							<TrashIcon className="size-4" />
							Remove
						</Button>
					</div>
				</div>
			) : (
				<form
					className="flex flex-col gap-3"
					onSubmit={(e) => {
						e.preventDefault();
						if (key.trim()) saveKey();
					}}
				>
					<TextInput
						label="API key"
						type="password"
						value={key}
						onChange={setKey}
						isInvalid={!!error}
						description={`Checked with ${provider.company} before it is saved. Create one at ${provider.keyUrl}.`}
					/>
					{error && (
						<p role="alert" className="text-sm text-problem">
							{error}
						</p>
					)}
					<div className="flex justify-end gap-2">
						{replacing && (
							<Button
								variant="ghost"
								onPress={() => {
									setReplacing(false);
									setError(null);
									setKey("");
								}}
							>
								Cancel
							</Button>
						)}
						<Button
							variant="primary"
							type="submit"
							isDisabled={busy || !key.trim()}
						>
							{busy ? <Spinner /> : null}
							Check and save
						</Button>
					</div>
				</form>
			)}
		</Section>
	);
}

export function SettingsView() {
	const { closeSettings } = useNav();
	const [tab, setTab] = useState(savedTab);
	const chooseTab = (t: string) => {
		setTab(t);
		try {
			localStorage.setItem(TAB_KEY, t);
		} catch {
			// remembering the tab is a convenience only
		}
	};
	const qc = useQueryClient();
	const { data: settings } = useSettings();
	const hasKeys = (settings?.apiKeys.length ?? 0) > 0;
	const models = useModels(hasKeys);
	const theme = useTheme();
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

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center gap-3 px-6"
			>
				<Button variant="ghost" className="-ml-2 px-2" onPress={closeSettings}>
					<ArrowLeftIcon className="size-[17px]" /> Back
				</Button>
				<h1 className="text-[15px] font-semibold">Settings</h1>
			</header>
			<Tabs
				selectedKey={tab}
				onSelectionChange={(k) => chooseTab(String(k))}
				className="flex min-h-0 flex-1 flex-col"
			>
				<div className="border-b border-border px-6">
					<TabList
						aria-label="Settings sections"
						className="mx-auto flex w-full max-w-2xl gap-1"
					>
						<Tab id="general" className={tabClass}>
							<SlidersHorizontalIcon className="size-4" />
							General
						</Tab>
						<Tab id="search" className={tabClass}>
							<MagnifyingGlassIcon className="size-4" />
							Search
						</Tab>
						<Tab id="usage" className={tabClass}>
							<ChartBarIcon className="size-4" />
							Usage
						</Tab>
						<Tab id="privacy" className={tabClass}>
							<LockSimpleIcon className="size-4" />
							Privacy
						</Tab>
					</TabList>
				</div>
				<div className="min-h-0 flex-1 overflow-y-auto">
					<div className="mx-auto flex w-full max-w-2xl flex-col px-6 pt-2 pb-12">
						<TabPanel id="general" className="outline-none">
							{PROVIDERS.map((p) => (
								<ApiKeySection key={p.id} provider={p} />
							))}

							<Section title="Default model">
								<ModelSelect
									label="Default model"
									models={models.data?.models ?? []}
									value={defaultModel}
									onChange={setDefaultModel}
									isDisabled={!hasKeys}
									placeholder={
										hasKeys ? "Choose a model" : "Add an API key first"
									}
								/>
								{models.isError && (
									<p role="alert" className="text-sm text-problem">
										Couldn't load models: {models.error.message}
									</p>
								)}
								{models.data?.errors.map((e) => (
									<p
										key={e.provider}
										role="alert"
										className="text-sm text-problem"
									>
										Couldn't load {providerName(e.provider)} models: {e.message}
									</p>
								))}
							</Section>

							<Section title="Appearance">
								<RadioGroup
									aria-label="Theme"
									orientation="horizontal"
									value={theme}
									onChange={(v) => setTheme(v as ThemePref)}
									className="inline-flex gap-1 self-start rounded-lg bg-muted p-1"
								>
									{THEMES.map((t) => (
										<Radio
											key={t.id}
											value={t.id}
											className={cx(
												"cursor-default rounded-md px-3 py-1 text-[13px] text-muted-foreground",
												"data-[selected]:bg-card data-[selected]:font-medium data-[selected]:text-foreground data-[selected]:shadow-sm",
												focus,
											)}
										>
											{t.label}
										</Radio>
									))}
								</RadioGroup>
							</Section>
						</TabPanel>
						<TabPanel id="search" className="outline-none">
							<Section title="Search model">
								<SearchModel />
							</Section>
							<Section title="Search results">
								<SharpenResults />
							</Section>
							<Section title="Scanned pages">
								<ScannedPages />
							</Section>
							<Section title="Paper details">
								<div className="flex flex-col gap-6">
									<DetailsLookup />
									<DetailsModel />
								</div>
							</Section>
						</TabPanel>
						<TabPanel id="usage" className="outline-none">
							<Section title="Usage and cost">
								<UsageSection />
							</Section>
						</TabPanel>
						<TabPanel id="privacy" className="outline-none">
							<Section title="Privacy">
								<p className="text-sm leading-relaxed text-muted-foreground">
									Projects and chats are stored on this Mac. Messages you send
									go to the provider of the model you chose, Google (Gemini) or
									OpenAI, with your own API key for it. Meaning search sends
									passages of your papers to Gemini to be indexed, and only if
									you agree. Sharpening search results, if you turn it on, sends
									the top 20 passages of each search to your chat model's
									provider. The local search model keeps them on this Mac, after
									a one-time download from Hugging Face. About once a week the
									app also downloads a public price list from models.dev for
									cost estimates, which sends nothing of yours.
								</p>
							</Section>
						</TabPanel>
					</div>
				</div>
			</Tabs>
		</div>
	);
}
