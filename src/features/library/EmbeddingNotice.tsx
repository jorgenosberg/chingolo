import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "../../components/ui";
import { api } from "../../lib/api";
import type { LayoutProgress } from "../../lib/bindings";
import { useEmbeddingStatus, useLayoutProgress } from "../../lib/library";
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
	const status = useEmbeddingStatus();
	const layout = useLayoutProgress().data;
	// Offers and failures of the search model come first. Layout shows by itself then.
	const embeddingReady = status?.model.kind === "ready";
	return (
		<>
			<EmbeddingBanner layout={embeddingReady ? layout : undefined} />
			{!embeddingReady && <LayoutBanner layout={layout} />}
		</>
	);
}

const layoutBusy = (l?: LayoutProgress) =>
	Boolean(l?.available && (l.done < l.total || l.tablesDone < l.tablesTotal));

function ProgressLine({
	label,
	value,
	max,
}: {
	label: string;
	value: number;
	max: number;
}) {
	return (
		<div
			role="progressbar"
			aria-label={label}
			aria-valuemin={0}
			aria-valuemax={max}
			aria-valuenow={value}
			className="mt-1.5 h-1 w-56 overflow-hidden rounded-full bg-border"
		>
			<div
				className="h-full bg-primary transition-[width]"
				style={{ width: `${max ? (value / max) * 100 : 0}%` }}
			/>
		</div>
	);
}

const layoutText = (l: LayoutProgress) =>
	l.done < l.total
		? `Cleaning up page layout (headers, footers, columns): ${l.done.toLocaleString()} of ${l.total.toLocaleString()} pages. Search works meanwhile.`
		: `Reading tables: ${l.tablesDone.toLocaleString()} of ${l.tablesTotal.toLocaleString()}. Search works meanwhile.`;

function LayoutBanner({ layout }: { layout?: LayoutProgress }) {
	if (!layout || !layoutBusy(layout)) return null;
	return (
		<Banner>
			<p>{layoutText(layout)}</p>
			<ProgressLine
				label={layout.done < layout.total ? "Page layout" : "Tables"}
				value={layout.done < layout.total ? layout.done : layout.tablesDone}
				max={layout.done < layout.total ? layout.total : layout.tablesTotal}
			/>
		</Banner>
	);
}

function EmbeddingBanner({ layout }: { layout?: LayoutProgress }) {
	const qc = useQueryClient();
	const status = useEmbeddingStatus();
	const { data: settings } = useSettings();
	const [error, setError] = useState<string | null>(null);

	if (!status || !settings) return null;
	const { model, indexed, total, paused } = status;

	const download = () => {
		setError(null);
		const start =
			status.backend === "gemini"
				? api.allowCloudEmbedding()
				: api.downloadEmbeddingModel();
		start.catch((e) => setError(String(e.message ?? e)));
	};
	const switchToLocal = () => {
		setError(null);
		api
			.setEmbeddingBackend("local")
			.catch((e) => setError(String(e.message ?? e)));
	};
	const dismiss = async () => {
		await api.updateSettings({ embeddingNoticeDismissed: true });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	if (model.kind === "missing" && status.backend === "gemini") {
		if (total === 0 || settings.embeddingNoticeDismissed) return null;
		return (
			<Banner
				actions={
					<>
						<Button variant="ghost" onPress={switchToLocal}>
							Use a local model
						</Button>
						<Button variant="ghost" onPress={dismiss}>
							Not now
						</Button>
						<Button variant="primary" onPress={download}>
							Allow
						</Button>
					</>
				}
			>
				<p>
					Meaning search sends the text of your papers to Google's Gemini
					service to be indexed, using your API key. It costs well under a
					dollar for a few hundred papers. Keyword search works without it.
				</p>
				{error && <p className="text-problem">{error}</p>}
			</Banner>
		);
	}

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

	const indexing = indexed < total;
	const laying = layoutBusy(layout);
	if (!indexing && !laying) return null;
	if (!indexing && layout) return <LayoutBanner layout={layout} />;
	const pause = (
		<Button variant="ghost" onPress={() => api.setEmbeddingPaused(!paused)}>
			{paused ? "Resume indexing" : "Pause indexing"}
		</Button>
	);
	// Two things at once: say what is going on, without numbers.
	if (laying) {
		return (
			<Banner actions={pause}>
				<p>
					Getting your library ready:{" "}
					{paused ? "indexing is paused" : "indexing for meaning search"} and
					cleaning up page layout and tables. Search works meanwhile.
				</p>
			</Banner>
		);
	}
	return (
		<Banner actions={pause}>
			<p>
				{paused ? "Indexing paused" : "Indexing for meaning search"}:{" "}
				{indexed.toLocaleString()} of {total.toLocaleString()} passages.
			</p>
			<ProgressLine label="Indexing passages" value={indexed} max={total} />
		</Banner>
	);
}
