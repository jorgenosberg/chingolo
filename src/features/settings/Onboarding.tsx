import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Radio, RadioGroup } from "react-aria-components";
import { ModelSelect, pickDefaultModel } from "../../components/ModelSelect";
import { Button, cx, focus, Spinner, TextInput } from "../../components/ui";
import { api } from "../../lib/api";
import type { ModelInfo } from "../../lib/bindings";
import {
	type ModelRef,
	PROVIDERS,
	providerInfo,
	providerName,
} from "../../lib/providers";
import { keys, useSettings } from "../../lib/queries";
import { PrivacyNotice } from "./PrivacyNotice";

type Step = "welcome" | "key" | "model" | "privacy";

export function Onboarding() {
	const qc = useQueryClient();
	const settings = useSettings();
	const [step, setStep] = useState<Step>("welcome");
	const [provider, setProvider] = useState<string>(PROVIDERS[0].id);
	const [key, setKey] = useState("");
	const [models, setModels] = useState<ModelInfo[]>([]);
	const [model, setModel] = useState<ModelRef | null>(null);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const finish = async (acknowledged: boolean) => {
		setBusy(true);
		try {
			await api.updateSettings({
				onboarded: true,
				// A new install has nothing to catch up on, so no "What's new" on first launch.
				lastSeenVersion: settings.data?.appVersion ?? null,
				privacyAcknowledged: acknowledged ? true : null,
			});
			await qc.invalidateQueries({ queryKey: keys.settings });
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
			setBusy(false);
		}
	};

	const saveKey = async () => {
		setBusy(true);
		setError(null);
		try {
			const list = await api.saveApiKey(provider, key);
			await qc.invalidateQueries({ queryKey: keys.models });
			setModels(list);
			setModel(pickDefaultModel(list));
			setStep("model");
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
		} finally {
			setBusy(false);
		}
	};

	const saveModel = async () => {
		if (model) {
			await api.updateSettings({
				defaultModel: model.id,
				defaultProvider: model.provider,
			});
		}
		setStep("privacy");
	};

	return (
		<div className="flex h-full items-center justify-center bg-background p-8">
			{/* Lets the window be dragged by its hidden title bar. */}
			<div data-tauri-drag-region className="fixed inset-x-0 top-0 h-10" />
			<div className="w-full max-w-md">
				{step === "welcome" && (
					<div className="flex flex-col gap-5">
						<div>
							<h1 className="text-2xl font-semibold tracking-tight">
								Chingolo
							</h1>
							<p className="mt-1 text-muted-foreground">
								A little help for big ideas.
							</p>
						</div>
						<p className="text-sm leading-relaxed text-muted-foreground">
							A research companion that keeps your work on your Mac. Bring your
							own AI key and start a conversation. Importing papers comes next.
						</p>
						<div>
							<Button variant="primary" onPress={() => setStep("key")}>
								Get started
							</Button>
						</div>
					</div>
				)}

				{step === "key" && (
					<form
						className="flex flex-col gap-4"
						onSubmit={(e) => {
							e.preventDefault();
							if (key.trim()) saveKey();
						}}
					>
						<h2 className="text-lg font-semibold">Connect an AI provider</h2>
						<p className="text-sm text-muted-foreground">
							One is enough to start. You can add the other in Settings.
						</p>
						<RadioGroup
							aria-label="Provider"
							orientation="horizontal"
							value={provider}
							onChange={(v) => {
								setProvider(v);
								setError(null);
							}}
							className="inline-flex gap-1 self-start rounded-lg bg-muted p-1"
						>
							{PROVIDERS.map((p) => (
								<Radio
									key={p.id}
									value={p.id}
									className={cx(
										"cursor-pointer data-[disabled]:cursor-default rounded-md px-3 py-1 text-[13px] text-muted-foreground",
										"data-[selected]:bg-card data-[selected]:font-medium data-[selected]:text-foreground data-[selected]:shadow-sm",
										focus,
									)}
								>
									{p.name}
								</Radio>
							))}
						</RadioGroup>
						<TextInput
							key={provider}
							label={`${providerName(provider)} API key`}
							type="password"
							value={key}
							onChange={setKey}
							autoFocus
							isInvalid={!!error}
							description={`Create a key at ${providerInfo(provider)?.keyUrl}. Chingolo stores it in the macOS Keychain.`}
						/>
						{error && (
							<p role="alert" className="text-sm text-problem">
								{error}
							</p>
						)}
						<div className="flex items-center justify-between">
							<Button
								variant="ghost"
								onPress={() => finish(false)}
								isDisabled={busy}
							>
								Skip for now
							</Button>
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

				{step === "model" && (
					<div className="flex flex-col gap-4">
						<h2 className="text-lg font-semibold">Choose a model</h2>
						<p className="text-sm text-muted-foreground">
							You can change this for any conversation later.
						</p>
						<ModelSelect
							label="Default model"
							models={models}
							value={model}
							onChange={setModel}
							all
						/>
						{error && (
							<p role="alert" className="text-sm text-problem">
								{error}
							</p>
						)}
						<div className="flex justify-end">
							<Button variant="primary" onPress={saveModel} isDisabled={!model}>
								Continue
							</Button>
						</div>
					</div>
				)}

				{step === "privacy" && (
					<div className="flex flex-col gap-4">
						<h2 className="text-lg font-semibold">Before you start</h2>
						<PrivacyNotice onAcknowledge={() => finish(true)} pending={busy} />
						{error && (
							<p role="alert" className="text-sm text-problem">
								{error}
							</p>
						)}
					</div>
				)}
			</div>
		</div>
	);
}
