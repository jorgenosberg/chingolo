import { CheckIcon } from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Checkbox } from "react-aria-components";
import { useDialogs } from "../../components/dialogs";
import { Button, cx, focus, Spinner, TextInput } from "../../components/ui";
import { api } from "../../lib/api";
import { keys, useSettings } from "../../lib/queries";
import { BugReportDialog } from "./BugReportDialog";

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function Toggle({
	label,
	text,
	isSelected,
	isDisabled,
	onChange,
}: {
	label: string;
	text: string;
	isSelected: boolean;
	isDisabled: boolean;
	onChange: (on: boolean) => void;
}) {
	return (
		<div className="flex flex-col gap-1">
			<Checkbox
				isSelected={isSelected}
				isDisabled={isDisabled}
				onChange={onChange}
				className={cx(
					"group flex cursor-pointer data-[disabled]:cursor-default data-[disabled]:opacity-60 items-center gap-2.5 text-[13px]",
					focus,
				)}
			>
				<span className="flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[selected]:border-primary group-data-[selected]:bg-primary">
					<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
				</span>
				{label}
			</Checkbox>
			<p className="pl-[26px] text-sm leading-relaxed text-muted-foreground">
				{text}
			</p>
		</div>
	);
}

/** Server address and token, and what to send. Everything starts off. */
export function ReportingSection() {
	const qc = useQueryClient();
	const { confirm } = useDialogs();
	const { data: settings } = useSettings();
	const [url, setUrl] = useState("");
	const [token, setToken] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [tested, setTested] = useState<string | null>(null);
	const [reporting, setReporting] = useState(false);
	const [changing, setChanging] = useState(false);
	if (!settings) return null;
	const connected = !!settings.reportingUrl && settings.hasReportingToken;
	const refresh = () => qc.invalidateQueries({ queryKey: keys.settings });

	const run = async (f: () => Promise<unknown>) => {
		setBusy(true);
		setError(null);
		setTested(null);
		try {
			await f();
		} catch (e) {
			setError(message(e));
		} finally {
			setBusy(false);
		}
	};

	const connect = () =>
		run(async () => {
			await api.connectReporting(url, token);
			setUrl("");
			setToken("");
			setChanging(false);
			await refresh();
		});

	const test = () =>
		run(async () => {
			await api.testReportingConnection();
			setTested("Connected.");
			await refresh();
		});

	const disconnect = async () => {
		const ok = await confirm({
			title: "Disconnect from the server?",
			message:
				"The token is deleted from the macOS Keychain and both reports are turned off.",
			confirmLabel: "Disconnect",
			danger: true,
		});
		if (!ok) return;
		await run(async () => {
			await api.disconnectReporting();
			await refresh();
		});
	};

	const set = (patch: { usageStats?: boolean; crashReports?: boolean }) =>
		run(async () => {
			await api.updateSettings(patch);
			await refresh();
		});

	return (
		<div className="flex flex-col gap-4">
			<p className="text-sm leading-relaxed text-muted-foreground">
				Off by default. If someone gave you a server address and a token, enter
				them here to help them find problems. Reports go only to that server.
				They never include your papers, file names, titles, chats or API keys.
			</p>
			{connected && !changing ? (
				<div className="flex flex-col gap-2">
					<div className="flex items-center justify-between gap-3">
						<p className="min-w-0 truncate text-sm text-muted-foreground">
							Connected to {settings.reportingUrl}. The token is in the macOS
							Keychain.
						</p>
						<div className="flex shrink-0 gap-2">
							<Button onPress={test} isDisabled={busy}>
								Test connection
							</Button>
							<Button
								onPress={() => {
									setUrl(settings.reportingUrl ?? "");
									setToken("");
									setError(null);
									setChanging(true);
								}}
								isDisabled={busy}
							>
								Change token
							</Button>
							<Button variant="ghost" onPress={disconnect} isDisabled={busy}>
								Disconnect
							</Button>
						</div>
					</div>
					{settings.reportingError && (
						<p role="alert" className="text-sm text-problem">
							{settings.reportingError} Use Change token to enter it again.
						</p>
					)}
				</div>
			) : (
				<form
					className="flex flex-col gap-3"
					onSubmit={(e) => {
						e.preventDefault();
						if (url.trim() && token.trim()) connect();
					}}
				>
					<TextInput
						label="Server address"
						value={url}
						onChange={setUrl}
						placeholder="https://"
					/>
					<TextInput
						label="Token"
						type="password"
						value={token}
						onChange={setToken}
						isInvalid={!!error}
						description="Checked with the server before it is saved."
					/>
					<div className="flex justify-end gap-2">
						{changing && (
							<Button
								variant="ghost"
								onPress={() => {
									setChanging(false);
									setError(null);
								}}
							>
								Cancel
							</Button>
						)}
						<Button
							type="submit"
							variant="primary"
							isDisabled={busy || !url.trim() || !token.trim()}
						>
							{busy && <Spinner decorative />}
							Connect
						</Button>
					</div>
				</form>
			)}
			{error && (
				<p role="alert" className="text-sm text-problem">
					{error}
				</p>
			)}
			{tested && <p className="text-sm text-muted-foreground">{tested}</p>}
			<Toggle
				label="Send usage statistics"
				isSelected={settings.usageStats}
				isDisabled={!connected || busy}
				onChange={(on) => set({ usageStats: on })}
				text="Once a day, counts such as chats sent, documents imported, errors and how long chats get, with the provider and model name. Never what you wrote or read."
			/>
			<Toggle
				label="Send crash reports"
				isSelected={settings.crashReports}
				isDisabled={!connected || busy}
				onChange={(on) => set({ crashReports: on })}
				text="If the app crashes, the error message and where in the code it happened, sent the next time you open the app."
			/>
			<div className="flex flex-col gap-1">
				<div>
					<Button onPress={() => setReporting(true)} isDisabled={!connected}>
						Send a bug report
					</Button>
				</div>
				<p className="text-sm leading-relaxed text-muted-foreground">
					Works with both boxes above off. You see exactly what is sent before
					it goes.
				</p>
			</div>
			<BugReportDialog isOpen={reporting} onClose={() => setReporting(false)} />
		</div>
	);
}
