import { ProgressBar } from "../../components/ui";
import type { LaneSnapshot, LaneState } from "../../lib/bindings";
import { formatCount, LANE_HINT, LANE_TITLE, useJobs } from "../../lib/jobs";

const STATE_LABEL: Record<LaneState, string> = {
	idle: "Idle",
	waiting: "Waiting",
	running: "Running",
	paused: "Paused",
	failed: "Needs attention",
};

function StatePill({ state }: { state: LaneState }) {
	return (
		<span
			className={
				state === "failed"
					? "rounded-full bg-problem/10 px-2 py-0.5 text-[11px] text-problem"
					: "rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
			}
		>
			{STATE_LABEL[state]}
		</span>
	);
}

function LaneCard({ lane }: { lane: LaneSnapshot }) {
	const { count, tables, current } = lane;
	return (
		<section
			aria-label={LANE_TITLE[lane.lane]}
			className="rounded-lg border border-border bg-muted/40 p-4"
		>
			<div className="flex items-center justify-between gap-3">
				<h2 className="text-[14px] font-medium">{LANE_TITLE[lane.lane]}</h2>
				<StatePill state={lane.state} />
			</div>
			<p className="mt-0.5 text-[13px] text-muted-foreground">
				{LANE_HINT[lane.lane]}
			</p>

			{count && count.total > 0 && (
				<div className="mt-3">
					<ProgressBar
						label={LANE_TITLE[lane.lane]}
						value={count.done}
						max={count.total}
					/>
					<p className="mt-1 text-[12px] text-muted-foreground">
						{formatCount(count)}
					</p>
				</div>
			)}
			{tables && tables.total > 0 && (
				<div className="mt-3">
					<ProgressBar
						label={`${LANE_TITLE[lane.lane]}, tables`}
						value={tables.done}
						max={tables.total}
					/>
					<p className="mt-1 text-[12px] text-muted-foreground">
						{formatCount(tables)}
					</p>
				</div>
			)}

			{current && (
				<p className="mt-3 truncate text-[13px]">
					<span className="text-muted-foreground">Now: </span>
					{current.title}
					{current.progress && (
						<span className="text-muted-foreground">
							{" "}
							({formatCount(current.progress)})
						</span>
					)}
				</p>
			)}
			{lane.waiting > 0 && lane.lane !== "index" && (
				<p className="mt-1 text-[12px] text-muted-foreground">
					{lane.waiting.toLocaleString()} waiting
				</p>
			)}

			{lane.failed.length > 0 && (
				<ul className="mt-3 space-y-1.5 border-t border-border pt-3">
					{lane.failed.map((f) => (
						<li key={f.documentId ?? f.error} className="text-[13px]">
							{f.title && <p className="truncate font-medium">{f.title}</p>}
							<p className="text-problem">{f.error}</p>
						</li>
					))}
				</ul>
			)}
		</section>
	);
}

export default function ActivityView() {
	const lanes = useJobs();
	const active = lanes.filter((l) => l.state !== "idle");
	return (
		<div className="flex h-full flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center px-6"
			>
				<h1 className="text-[15px] font-semibold">Activity</h1>
			</header>
			<div className="min-h-0 flex-1 overflow-y-auto px-6 pb-8">
				<div className="mx-auto flex w-full max-w-2xl flex-col gap-3">
					{active.map((l) => (
						<LaneCard key={l.lane} lane={l} />
					))}
					{active.length === 0 && (
						<p className="text-[13px] text-muted-foreground">
							Nothing is running.
						</p>
					)}
				</div>
			</div>
		</div>
	);
}
