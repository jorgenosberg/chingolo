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
import { keys, useModels, useSettings } from "../../lib/queries";
import { setTheme, type ThemePref, useTheme } from "../../lib/theme";

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
					Google's Gemini service with your own API key. Nothing else is sent
					anywhere.
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
