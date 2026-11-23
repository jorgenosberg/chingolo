import { useState } from "react";
import { Button as RacButton } from "react-aria-components";
import { useDialogs } from "../../components/dialogs";
import { Button, cx, focus, ProgressBar } from "../../components/ui";
import { api } from "../../lib/api";
import type { Lane, LaneSnapshot, LaneState } from "../../lib/bindings";
import { formatCount, LANE_HINT, LANE_TITLE, useJobs } from "../../lib/jobs";
import { Bird, type BirdMood, ReadingBird } from "./Bird";

const STATE_LABEL: Record<LaneState, string> = {
	idle: "Idle",
	waiting: "Waiting",
	running: "Running",
	paused: "Paused",
	failed: "Needs attention",
};

/** A paused lane can still be finishing one job, which it says. */
function stateLabel(lane: LaneSnapshot) {
	if (lane.paused) return lane.state === "running" ? "Pausing" : "Paused";
	return STATE_LABEL[lane.state];
}

function StatePill({ lane }: { lane: LaneSnapshot }) {
	return (
		<span
			className={
				lane.state === "failed"
					? "rounded-full bg-problem/10 px-2 py-0.5 text-[11px] text-problem"
					: "rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
			}
		>
			{stateLabel(lane)}
		</span>
	);
}

const PAUSABLE: Lane[] = ["read", "layout", "details", "index"];
const DOWNLOADS: Lane[] = [
	"downloadOcr",
	"downloadLayout",
	"downloadTables",
	"downloadEmbedding",
];

function SmallButton({
	onPress,
	label,
	children,
}: {
	onPress: () => void;
	label?: string;
	children: React.ReactNode;
}) {
	return (
		<Button
			variant="ghost"
			aria-label={label}
			onPress={onPress}
			className="h-6 shrink-0 px-2 text-xs"
		>
			{children}
		</Button>
	);
}

/** One kind of work: name and what it is, progress in the middle, state and controls at the end. */
function LaneRow({ lane }: { lane: LaneSnapshot }) {
	const { alert } = useDialogs();
	const { count, tables, current } = lane;
	const idle = lane.state === "idle";
	const title = LANE_TITLE[lane.lane];
	// Failures from the backend are plain sentences, so they can go straight to the user.
	const act = (run: () => Promise<unknown>) =>
		run().catch((e) =>
			alert({ title: "That didn't work", message: String(e) }),
		);

	const pausable = PAUSABLE.includes(lane.lane);
	const download = DOWNLOADS.includes(lane.lane);
	const control = pausable ? (
		lane.paused ? (
			<SmallButton
				label={`Resume ${title}`}
				onPress={() => act(() => api.setLanePaused(lane.lane, false))}
			>
				Resume
			</SmallButton>
		) : !idle && lane.state !== "failed" ? (
			<SmallButton
				label={`Pause ${title}`}
				onPress={() => act(() => api.setLanePaused(lane.lane, true))}
			>
				Pause
			</SmallButton>
		) : null
	) : download && lane.state === "running" ? (
		<SmallButton
			label={`Cancel ${title}`}
			onPress={() => act(() => api.cancelJob(lane.lane, null))}
		>
			Cancel
		</SmallButton>
	) : download && lane.state === "failed" ? (
		<SmallButton
			label={`Retry ${title}`}
			onPress={() => act(() => api.retryJob(lane.lane, null))}
		>
			Retry
		</SmallButton>
	) : null;

	// What stops the document on the worker: reading is cancelled, layout is skipped.
	const stopLabel = lane.lane === "read" ? "Cancel" : "Skip";
	const canStop = current?.documentId && ["read", "layout"].includes(lane.lane);

	return (
		<li
			aria-label={title}
			className="grid gap-x-6 gap-y-1.5 px-3 py-2.5 @3xl:grid-cols-[minmax(0,2fr)_minmax(0,3fr)_6.5rem] @3xl:items-center"
		>
			<div className="min-w-0">
				<h3 className="text-[14px] font-medium">{title}</h3>
				<p className="text-[13px] text-muted-foreground">
					{LANE_HINT[lane.lane]}
				</p>
			</div>

			<div className="min-w-0 space-y-2">
				{!idle && count && count.total > 0 && (
					<div>
						<ProgressBar label={title} value={count.done} max={count.total} />
						<p className="mt-1 text-[12px] text-muted-foreground">
							{formatCount(count)}
						</p>
					</div>
				)}
				{!idle && tables && tables.total > 0 && (
					<div>
						<ProgressBar
							label={`${title}, tables`}
							value={tables.done}
							max={tables.total}
						/>
						<p className="mt-1 text-[12px] text-muted-foreground">
							{formatCount(tables)}
						</p>
					</div>
				)}
				{current && (
					<div className="flex items-center gap-2">
						<p className="min-w-0 truncate text-[13px]">
							<span className="text-muted-foreground">Now: </span>
							{current.title}
							{current.progress && (
								<span className="text-muted-foreground">
									{" "}
									({formatCount(current.progress)})
								</span>
							)}
						</p>
						{canStop && (
							<SmallButton
								label={`${stopLabel} ${current.title}`}
								onPress={() =>
									act(() => api.cancelJob(lane.lane, current.documentId))
								}
							>
								{stopLabel}
							</SmallButton>
						)}
					</div>
				)}
				{lane.waiting > 0 && lane.lane !== "index" && (
					<p className="text-[12px] text-muted-foreground">
						{lane.waiting.toLocaleString()} waiting
					</p>
				)}
				{lane.paused && lane.state === "running" && (
					<p className="text-[12px] text-muted-foreground">
						Pauses after this one.
					</p>
				)}
			</div>

			<div className="flex items-start gap-1.5 @3xl:flex-col @3xl:items-end">
				<StatePill lane={lane} />
				{control}
			</div>

			{lane.failed.length > 0 && (
				<ul
					aria-label={`${title}, problems`}
					className="ml-0.5 space-y-2 border-l-2 border-problem/40 pl-3 @3xl:col-span-3"
				>
					{lane.failed.map((f) => (
						<li
							key={f.documentId ?? f.error}
							className="flex min-w-0 items-start justify-between gap-3"
						>
							<div className="min-w-0">
								{f.title && (
									<p className="truncate text-[13px] text-foreground">
										{f.title}
									</p>
								)}
								<p className="text-[12px] text-problem">{f.error}</p>
							</div>
							{pausable && (
								<SmallButton
									label={`Retry ${f.title || title}`}
									onPress={() =>
										act(() => api.retryJob(lane.lane, f.documentId))
									}
								>
									Retry
								</SmallButton>
							)}
						</li>
					))}
				</ul>
			)}
		</li>
	);
}

type Filter = "all" | "running" | "waiting" | "paused" | "failed";

const FILTERS: { id: Filter; name: string }[] = [
	{ id: "all", name: "All" },
	{ id: "running", name: "Running" },
	{ id: "waiting", name: "Waiting" },
	{ id: "paused", name: "Paused" },
	{ id: "failed", name: "Needs attention" },
];

const GROUPS: { name: string; lanes: Lane[] }[] = [
	{ name: "Library work", lanes: ["read", "layout", "details", "index"] },
	{
		name: "Models",
		lanes: [
			"downloadOcr",
			"downloadLayout",
			"downloadTables",
			"downloadEmbedding",
		],
	},
];

function Chip({
	pressed,
	onPress,
	children,
}: {
	pressed: boolean;
	onPress: () => void;
	children: React.ReactNode;
}) {
	return (
		<RacButton
			aria-pressed={pressed}
			onPress={onPress}
			className={cx(
				"h-7 rounded-full border px-3 text-xs transition-colors",
				pressed
					? "border-primary/40 bg-accent-soft font-medium text-foreground"
					: "border-border text-muted-foreground data-[hovered]:bg-muted data-[hovered]:text-foreground",
				focus,
			)}
		>
			{children}
		</RacButton>
	);
}

export default function ActivityView() {
	const lanes = useJobs();
	const [filter, setFilter] = useState<Filter>("all");
	const counts: Record<Filter, number> = {
		all: lanes.filter((l) => l.state !== "idle").length,
		running: 0,
		waiting: 0,
		paused: 0,
		failed: 0,
	};
	for (const l of lanes) if (l.state !== "idle") counts[l.state] += 1;
	// A filter that has emptied out falls back to everything.
	const shown = filter !== "all" && counts[filter] === 0 ? "all" : filter;
	const visible = (l: LaneSnapshot) =>
		shown === "all" ? true : l.state === shown;
	const idleOnly = counts.all === 0;
	const mood: BirdMood = lanes.some((l) => l.state === "running")
		? "busy"
		: lanes.some((l) => l.state === "waiting")
			? "waiting"
			: "still";
	const problem = counts.failed > 0;

	const rowsOf = (g: { lanes: Lane[] }) =>
		g.lanes
			.map((id) => lanes.find((l) => l.lane === id))
			.filter((l): l is LaneSnapshot => !!l && visible(l));
	const first = GROUPS.findIndex((g) => rowsOf(g).length > 0);

	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center px-6"
			>
				<h1 className="text-[15px] font-semibold">Activity</h1>
			</header>
			<div className="flex flex-wrap items-center gap-1.5 px-6 pb-3">
				{FILTERS.filter((f) => f.id === "all" || counts[f.id] > 0).map((f) => (
					<Chip
						key={f.id}
						pressed={shown === f.id}
						onPress={() => setFilter(f.id)}
					>
						{f.name}
						{f.id !== "all" ? ` ${counts[f.id]}` : ""}
					</Chip>
				))}
			</div>
			<div className="@container min-h-0 flex-1 overflow-y-auto px-6 pb-6">
				<div className="flex w-full flex-col gap-5">
					{idleOnly && (
						<div className="flex items-center gap-4">
							<ReadingBird />
							<div>
								<p className="text-[14px] font-medium">Nothing is running.</p>
								<p className="text-[13px] text-muted-foreground">
									Idle work is listed below.
								</p>
							</div>
						</div>
					)}
					{GROUPS.map((g, index) => {
						const rows = rowsOf(g);
						if (rows.length === 0) return null;
						return (
							<section key={g.name} aria-label={g.name}>
								{/* The first group carries the bird, perched on the list below. */}
								<div className="flex items-end justify-between">
									<h2 className="mb-2 text-[14px] font-semibold">{g.name}</h2>
									{index === first && !idleOnly && (
										<Bird mood={mood} problem={problem} />
									)}
								</div>
								<ul className="divide-y divide-border rounded-lg border border-border">
									{rows.map((l) => (
										<LaneRow key={l.lane} lane={l} />
									))}
								</ul>
							</section>
						);
					})}
				</div>
			</div>
		</div>
	);
}
