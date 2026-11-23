import { CheckIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { Checkbox, Label, TextArea, TextField } from "react-aria-components";
import { Button, cx, focus, Modal_, Spinner } from "../../components/ui";
import { api } from "../../lib/api";

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** A manual bug report. Shows the exact JSON that will be sent before it goes. */
export function BugReportDialog({
	isOpen,
	onClose,
}: {
	isOpen: boolean;
	onClose: () => void;
}) {
	const [description, setDescription] = useState("");
	const [includeLog, setIncludeLog] = useState(true);
	const [preview, setPreview] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [sent, setSent] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const close = () => {
		setDescription("");
		setPreview(null);
		setSent(false);
		setError(null);
		onClose();
	};

	const run = async (f: () => Promise<void>) => {
		setBusy(true);
		setError(null);
		try {
			await f();
		} catch (e) {
			setError(message(e));
		} finally {
			setBusy(false);
		}
	};

	const togglePreview = () =>
		run(async () => {
			setPreview(
				preview === null
					? await api.previewBugReport(description, includeLog)
					: null,
			);
		});

	const send = () =>
		run(async () => {
			await api.sendBugReport(description, includeLog);
			setSent(true);
		});

	return (
		<Modal_
			isOpen={isOpen}
			onOpenChange={(open) => !open && close()}
			title="Send a bug report"
			width="max-w-xl"
		>
			{sent ? (
				<div className="flex flex-col gap-4">
					<p className="text-sm text-muted-foreground">Sent. Thank you.</p>
					<div className="flex justify-end">
						<Button variant="primary" autoFocus onPress={close}>
							Close
						</Button>
					</div>
				</div>
			) : (
				<div className="flex flex-col gap-4">
					<TextField
						value={description}
						onChange={(v) => {
							setDescription(v);
							setPreview(null);
						}}
						className="flex flex-col gap-1.5"
					>
						<Label className="text-[13px] font-medium">What went wrong?</Label>
						<TextArea
							autoFocus
							rows={4}
							className={cx(
								"rounded-lg border border-input bg-card px-3 py-2 text-sm",
								focus,
							)}
						/>
					</TextField>
					<Checkbox
						isSelected={includeLog}
						onChange={(v) => {
							setIncludeLog(v);
							setPreview(null);
						}}
						className={cx(
							"group flex cursor-pointer items-center gap-2.5 text-[13px]",
							focus,
						)}
					>
						<span className="flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[selected]:border-primary group-data-[selected]:bg-primary">
							<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
						</span>
						Include the last 500 lines of the app log
					</Checkbox>
					{preview !== null && (
						<pre className="max-h-64 overflow-auto rounded-lg border border-border bg-muted p-3 text-xs whitespace-pre-wrap">
							{preview}
						</pre>
					)}
					{error && (
						<p role="alert" className="text-sm text-problem">
							{error}
						</p>
					)}
					<div className="flex justify-between gap-2">
						<Button
							variant="ghost"
							onPress={togglePreview}
							isDisabled={busy || !description.trim()}
						>
							{preview === null ? "Show what will be sent" : "Hide"}
						</Button>
						<div className="flex gap-2">
							<Button variant="ghost" onPress={close}>
								Cancel
							</Button>
							<Button
								variant="primary"
								onPress={send}
								isDisabled={busy || !description.trim()}
							>
								{busy && <Spinner decorative />}
								Send
							</Button>
						</div>
					</div>
				</div>
			)}
		</Modal_>
	);
}
