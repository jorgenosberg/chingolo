import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ModelSelect, pickDefaultModel } from "../../components/ModelSelect";
import { Button, Spinner, TextInput } from "../../components/ui";
import { api } from "../../lib/api";
import type { ModelInfo } from "../../lib/bindings";
import { keys } from "../../lib/queries";
import { PrivacyNotice } from "./PrivacyNotice";

type Step = "welcome" | "key" | "model" | "privacy";

export function Onboarding() {
	const qc = useQueryClient();
	const [step, setStep] = useState<Step>("welcome");
	const [key, setKey] = useState("");
	const [models, setModels] = useState<ModelInfo[]>([]);
	const [model, setModel] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const finish = async (acknowledged: boolean) => {
		setBusy(true);
		try {
			await api.updateSettings({
				onboarded: true,
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
			const list = await api.saveApiKey(key);
			qc.setQueryData(keys.models, list);
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
		if (model) await api.updateSettings({ defaultModel: model });
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
							<h1 className="text-2xl font-semibold tracking-tight">Chingui</h1>
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
						<h2 className="text-lg font-semibold">Connect Gemini</h2>
						<TextInput
							label="Gemini API key"
							type="password"
							value={key}
							onChange={setKey}
							autoFocus
							isInvalid={!!error}
							description="Create a key at aistudio.google.com/apikey. Chingui stores it in the macOS Keychain."
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
