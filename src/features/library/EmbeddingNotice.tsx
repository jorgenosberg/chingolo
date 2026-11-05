import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "../../components/ui";
import { api } from "../../lib/api";
import { useEmbeddingStatus } from "../../lib/library";
import { keys, useSettings } from "../../lib/queries";

export const mb = (bytes: number | null) =>
	`${Math.round((bytes ?? 0) / 1_000_000)} MB`;

function Banner({
	children,
	actions,
	role = "status",
}: {
	children: React.ReactNode;
	actions?: React.ReactNode;
	role?: "status" | "alert";
}) {
	return (
		<div
			role={role}
			className="mx-6 mb-3 flex items-center justify-between gap-3 rounded-lg border border-border bg-muted px-3 py-2 text-[13px]"
		>
			<div className="min-w-0">{children}</div>
			{actions && <div className="flex shrink-0 gap-2">{actions}</div>}
		</div>
	);
}

/**
 * Tells the user what the search model is doing: an offer to download it, the download,
 * indexing progress, or a failure. Keyword search works the whole time, and the notice
 * says so where it matters.
 */
export function EmbeddingNotice() {
	const qc = useQueryClient();
	const status = useEmbeddingStatus();
	const { data: settings } = useSettings();
	const [error, setError] = useState<string | null>(null);

	if (!status || !settings) return null;
	const { model, indexed, total, paused } = status;

	const download = () => {
		setError(null);
		api.downloadEmbeddingModel().catch((e) => setError(String(e.message ?? e)));
	};
	const dismiss = async () => {
		await api.updateSettings({ embeddingNoticeDismissed: true });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	if (model.kind === "missing") {
		// Nothing to search yet, or the user said no.
		if (total === 0 || settings.embeddingNoticeDismissed) return null;
		return (
			<Banner
				actions={
					<>
						<Button variant="ghost" onPress={dismiss}>
							Not now
						</Button>
						<Button variant="primary" onPress={download}>
							Download
						</Button>
					</>
				}
			>
				<p>
					Meaning search needs a one-time 200 MB model download from Hugging
					Face. Keyword search works without it.
				</p>
				{error && <p className="text-problem">{error}</p>}
			</Banner>
		);
	}

	if (model.kind === "downloading") {
		const pct = model.total ? ((model.done ?? 0) / model.total) * 100 : 0;
		return (
			<Banner>
				<p>
					Downloading the search model: {mb(model.done)} of {mb(model.total)}.
					Keyword search works meanwhile.
				</p>
				<div
					role="progressbar"
					aria-label="Search model download"
					aria-valuemin={0}
					aria-valuemax={100}
					aria-valuenow={Math.round(pct)}
					className="mt-1.5 h-1 w-56 overflow-hidden rounded-full bg-border"
				>
					<div
						className="h-full bg-primary transition-[width]"
						style={{ width: `${pct}%` }}
					/>
				</div>
			</Banner>
		);
	}

	if (model.kind === "failed") {
		return (
			<Banner
				role="alert"
				actions={
					<Button variant="primary" onPress={download}>
						Try again
					</Button>
				}
			>
				<p className="text-problem">
					The search model isn't working: {model.message}
				</p>
				<p className="text-muted-foreground">Keyword search still works.</p>
			</Banner>
		);
	}

	if (indexed >= total) return null;
	return (
		<Banner
			actions={
				<Button variant="ghost" onPress={() => api.setEmbeddingPaused(!paused)}>
					{paused ? "Resume" : "Pause"}
				</Button>
			}
		>
			<p>
				{paused ? "Indexing paused" : "Indexing for meaning search"}:{" "}
				{indexed.toLocaleString()} of {total.toLocaleString()} passages.
			</p>
			<div
				role="progressbar"
				aria-label="Indexing passages"
				aria-valuemin={0}
				aria-valuemax={total}
				aria-valuenow={indexed}
				className="mt-1.5 h-1 w-56 overflow-hidden rounded-full bg-border"
			>
				<div
					className="h-full bg-primary transition-[width]"
					style={{ width: `${(indexed / total) * 100}%` }}
				/>
			</div>
		</Banner>
	);
}
