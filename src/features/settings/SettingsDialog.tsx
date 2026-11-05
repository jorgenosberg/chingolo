import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Radio, RadioGroup } from "react-aria-components";
import { useDialogs } from "../../components/dialogs";
import { ModelSelect } from "../../components/ModelSelect";
import {
	Button,
	cx,
	focus,
	Modal_,
	Spinner,
	TextInput,
} from "../../components/ui";
import { api } from "../../lib/api";
import { useEmbeddingStatus } from "../../lib/library";
import { keys, useModels, useSettings } from "../../lib/queries";
import { setTheme, type ThemePref, useTheme } from "../../lib/theme";
import { mb } from "../library/EmbeddingNotice";

const THEMES: { id: ThemePref; label: string }[] = [
	{ id: "system", label: "System" },
	{ id: "light", label: "Light" },
	{ id: "dark", label: "Dark" },
];

function Section({
	title,
	children,
}: {
	title: string;
	children: React.ReactNode;
}) {
	return (
		<section className="flex flex-col gap-2.5 border-t border-border py-4 first:border-t-0 first:pt-0">
			<h3 className="text-[13px] font-semibold">{title}</h3>
			{children}
		</section>
	);
}

/** State of the local model behind meaning search, with the download button. */
function SearchModel() {
	const status = useEmbeddingStatus();
	const [error, setError] = useState<string | null>(null);
	if (!status) return null;
	const { model, indexed, total } = status;
	const download = () => {
		setError(null);
		api.downloadEmbeddingModel().catch((e) => setError(String(e.message ?? e)));
	};

	if (model.kind === "ready") {
		return (
			<p className="text-sm text-muted-foreground">
				Ready. {indexed.toLocaleString()} of {total.toLocaleString()} passages
				are indexed for meaning search.
			</p>
		);
	}
	if (model.kind === "downloading") {
		return (
			<p className="text-sm text-muted-foreground">
				Downloading: {mb(model.done)} of {mb(model.total)}.
			</p>
		);
	}
	return (
		<div className="flex items-start justify-between gap-3">
			<div className="text-sm text-muted-foreground">
				{model.kind === "failed" ? (
					<p role="alert" className="text-problem">
						{model.message}
					</p>
				) : (
					<p>
						Not downloaded. Meaning search needs a one-time download of about
						200 MB from Hugging Face. Keyword search works without it.
					</p>
				)}
				{error && <p className="text-problem">{error}</p>}
			</div>
			<Button className="shrink-0" onPress={download}>
				{model.kind === "failed" ? "Try again" : "Download"}
			</Button>
		</div>
	);
}

export function SettingsDialog({
	isOpen,
	onOpenChange,
}: {
	isOpen: boolean;
	onOpenChange: (open: boolean) => void;
}) {
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
		<Modal_
			isOpen={isOpen}
			onOpenChange={onOpenChange}
			title="Settings"
			width="max-w-lg"
		>
			<Section title="Gemini API key">
				{settings?.hasApiKey && !replacing ? (
					<div className="flex items-center justify-between gap-3">
						<p className="text-sm text-muted-foreground">
							Saved in the macOS Keychain.
						</p>
						<div className="flex gap-2">
							<Button onPress={() => setReplacing(true)}>Replace</Button>
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
						settings?.hasApiKey ? "Choose a model" : "Add an API key first"
					}
				/>
				{models.isError && (
					<p role="alert" className="text-sm text-problem">
						Couldn't load models: {models.error.message}
					</p>
				)}
			</Section>

			<Section title="Search model">
				<SearchModel />
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

			<Section title="Privacy">
				<p className="text-sm leading-relaxed text-muted-foreground">
					Projects and chats are stored on this Mac. Messages you send go to
					Google's Gemini service with your own API key. The only other network
					use is the one-time search model download from Hugging Face, and only
					if you agree. Your documents are never sent for it.
				</p>
			</Section>

			<div className="mt-1 flex justify-end">
				<Button variant="primary" onPress={() => onOpenChange(false)}>
					Done
				</Button>
			</div>
		</Modal_>
	);
}
