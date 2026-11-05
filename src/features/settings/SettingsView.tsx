import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import {
	Radio,
	RadioGroup,
	Tab,
	TabList,
	TabPanel,
	Tabs,
} from "react-aria-components";
import { useNav } from "../../app/nav";
import { useDialogs } from "../../components/dialogs";
import { ChevronIcon } from "../../components/icons";
import { ModelSelect } from "../../components/ModelSelect";
import { Button, cx, focus, Spinner, TextInput } from "../../components/ui";
import { api } from "../../lib/api";
import { useEmbeddingStatus } from "../../lib/library";
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
	"-mb-px cursor-default border-b-2 border-transparent px-3 py-2 text-[13px] text-muted-foreground",
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

/** Which model makes the meaning-search index, and its state. */
function SearchModel() {
	const status = useEmbeddingStatus();
	const [error, setError] = useState<string | null>(null);
	if (!status) return null;
	const { backend, model, indexed, total } = status;
	const fail = (e: { message?: string }) => setError(String(e.message ?? e));
	const start = () => {
		setError(null);
		(backend === "gemini"
			? api.allowCloudEmbedding()
			: api.downloadEmbeddingModel()
		).catch(fail);
	};
	const choose = (next: string) => {
		setError(null);
		api.setEmbeddingBackend(next as "gemini" | "local").catch(fail);
	};

	let body: React.ReactNode;
	if (model.kind === "ready") {
		body = (
			<p>
				Ready. {indexed.toLocaleString()} of {total.toLocaleString()} passages
				are indexed for meaning search.
			</p>
		);
	} else if (model.kind === "downloading") {
		body = (
			<p>
				Downloading: {mb(model.done)} of {mb(model.total)}.
			</p>
		);
	} else {
		return (
			<div className="flex flex-col gap-3">
				<Backends value={backend} onChange={choose} />
				<div className="flex items-start justify-between gap-3 text-sm text-muted-foreground">
					<div>
						{model.kind === "failed" ? (
							<p role="alert" className="text-problem">
								{model.message}
							</p>
						) : backend === "gemini" ? (
							<p>
								Not turned on. Passages of your papers would be sent to Gemini
								to be indexed, using your API key. Keyword search works without
								it.
							</p>
						) : (
							<p>
								Not downloaded. Meaning search needs a one-time download of
								about 200 MB from Hugging Face. Keyword search works without it.
							</p>
						)}
						{error && <p className="text-problem">{error}</p>}
					</div>
					<Button className="shrink-0" onPress={start}>
						{model.kind === "failed"
							? "Try again"
							: backend === "gemini"
								? "Allow"
								: "Download"}
					</Button>
				</div>
			</div>
		);
	}
	return (
		<div className="flex flex-col gap-3">
			<Backends value={backend} onChange={choose} />
			<div className="text-sm text-muted-foreground">
				{body}
				{error && <p className="text-problem">{error}</p>}
			</div>
		</div>
	);
}

function Backends({
	value,
	onChange,
}: {
	value: string;
	onChange: (v: string) => void;
}) {
	return (
		<RadioGroup
			aria-label="Search model"
			value={value}
			onChange={onChange}
			className="flex flex-col gap-1.5 text-sm"
		>
			<Radio value="gemini" className={cx("cursor-default", focus)}>
				<span className="font-medium">Gemini (online).</span>{" "}
				<span className="text-muted-foreground">
					Fast and light on this Mac. Sends passages to Google.
				</span>
			</Radio>
			<Radio value="local" className={cx("cursor-default", focus)}>
				<span className="font-medium">On this Mac.</span>{" "}
				<span className="text-muted-foreground">
					Nothing leaves your computer. 200 MB download, slower to index.
				</span>
			</Radio>
			<p className="text-[13px] text-muted-foreground">
				Switching rebuilds the index with the new model.
			</p>
		</RadioGroup>
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
	const { confirm } = useDialogs();
	const { data: settings } = useSettings();
	const models = useModels(!!settings?.hasApiKey);
	const theme = useTheme();
	const [replacing, setReplacing] = useState(false);
	const [key, setKey] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const saveKey = async () => {
		setBusy(true);
		setError(null);
		try {
			const list = await api.saveApiKey(key);
			qc.setQueryData(keys.models, list);
			await qc.invalidateQueries({ queryKey: keys.settings });
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
			title: "Remove API key?",
			message:
				"The key is deleted from the macOS Keychain. Your chats stay, but you can't send messages until you add a key again.",
			confirmLabel: "Remove key",
			danger: true,
		});
		if (!ok) return;
		await api.deleteApiKey();
		qc.removeQueries({ queryKey: keys.models });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	const setDefaultModel = async (id: string) => {
		await api.updateSettings({ defaultModel: id });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center gap-3 px-6"
			>
				<Button variant="ghost" className="-ml-2 px-2" onPress={closeSettings}>
					<ChevronIcon className="size-3.5 rotate-180" /> Back
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
							General
						</Tab>
						<Tab id="search" className={tabClass}>
							Search
						</Tab>
						<Tab id="usage" className={tabClass}>
							Usage
						</Tab>
						<Tab id="privacy" className={tabClass}>
							Privacy
						</Tab>
					</TabList>
				</div>
				<div className="min-h-0 flex-1 overflow-y-auto">
					<div className="mx-auto flex w-full max-w-2xl flex-col px-6 pt-2 pb-12">
						<TabPanel id="general" className="outline-none">
							<Section title="Gemini API key">
								{settings?.hasApiKey && !replacing ? (
									<div className="flex items-center justify-between gap-3">
										<p className="text-sm text-muted-foreground">
											Saved in the macOS Keychain.
										</p>
										<div className="flex gap-2">
											<Button onPress={() => setReplacing(true)}>
												Replace
											</Button>
											<Button variant="ghost" onPress={removeKey}>
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
											description="Checked with Google before it is saved. Create one at aistudio.google.com/apikey."
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

							<Section title="Default model">
								<ModelSelect
									label="Default model"
									models={models.data ?? []}
									value={settings?.defaultModel ?? null}
									onChange={setDefaultModel}
									isDisabled={!settings?.hasApiKey}
									placeholder={
										settings?.hasApiKey
											? "Choose a model"
											: "Add an API key first"
									}
								/>
								{models.isError && (
									<p role="alert" className="text-sm text-problem">
										Couldn't load models: {models.error.message}
									</p>
								)}
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
									go to Google's Gemini service with your own API key. Meaning
									search sends passages of your papers to Gemini to be indexed,
									and only if you agree. The local search model keeps them on
									this Mac, after a one-time download from Hugging Face. About
									once a week the app also downloads a public price list from
									models.dev for cost estimates, which sends nothing of yours.
								</p>
							</Section>
						</TabPanel>
					</div>
				</div>
			</Tabs>
		</div>
	);
}
