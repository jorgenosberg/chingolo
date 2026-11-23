import { useQuery } from "@tanstack/react-query";
import { api } from "./api";
import type { Count, Lane, LaneSnapshot } from "./bindings";

/** Filled by the `jobsChanged` event (see `useLibraryEvents`) and seeded by the `jobs` command. */
export const jobsKey = ["jobs"] as const;

const NONE: LaneSnapshot[] = [];

/** Every kind of background work and where it stands. Empty until the first answer. */
export function useJobs(): LaneSnapshot[] {
	const q = useQuery({
		queryKey: jobsKey,
		queryFn: api.jobs,
		staleTime: Number.POSITIVE_INFINITY,
	});
	return q.data ?? NONE;
}

export const LANE_TITLE: Record<Lane, string> = {
	read: "Reading documents",
	layout: "Page layout and tables",
	details: "Paper details",
	index: "Indexing for meaning search",
	downloadOcr: "Text recognition model",
	downloadLayout: "Page layout model",
	downloadTables: "Table model",
	downloadEmbedding: "Search model",
};

export const LANE_HINT: Record<Lane, string> = {
	read: "Turns imported files into searchable text. Scans are read with text recognition.",
	layout:
		"Removes headers and footers, puts columns in order, and reads tables.",
	details:
		"Finds title, authors and year, and looks papers up online if you allowed it.",
	index: "Makes passages searchable by meaning.",
	downloadOcr: "Reads pages that have no text.",
	downloadLayout: "Finds headers, columns and tables on a page.",
	downloadTables: "Reads the rows and columns of tables.",
	downloadEmbedding: "Runs meaning search on this Mac.",
};

const mb = (bytes: number) => `${Math.round(bytes / 1_000_000)} MB`;
const n = (x: number) => Math.round(x).toLocaleString();

/** "12 of 40 pages", "30 MB of 130 MB". */
export function formatCount(c: Count): string {
	switch (c.unit) {
		case "bytes":
			return `${mb(c.done)} of ${mb(c.total)}`;
		case "pages":
			return `${n(c.done)} of ${n(c.total)} pages`;
		case "passages":
			return `${n(c.done)} of ${n(c.total)} passages`;
		case "tables":
			return `${n(c.done)} of ${n(c.total)} tables`;
	}
}

const plural = (count: number, one: string, many: string) =>
	`${count.toLocaleString()} ${count === 1 ? one : many}`;

/** What a busy lane is doing, in a few words. */
function busyPhrase(l: LaneSnapshot): string {
	switch (l.lane) {
		case "read":
			return `Reading ${plural(l.waiting + (l.state === "running" ? 1 : 0), "document", "documents")}`;
		case "layout":
			return "Cleaning up page layout";
		case "details":
			return `Updating details for ${plural(l.waiting, "document", "documents")}`;
		case "index":
			return "Indexing";
		default:
			return "Downloading a model";
	}
}

const lowerFirst = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);

export type JobsSummary = {
	/** `problem` only shows when nothing is running or paused. */
	state: "idle" | "busy" | "paused" | "problem";
	text: string;
};

/** One short line for the sidebar indicator and the library banner. */
export function summarizeJobs(lanes: LaneSnapshot[]): JobsSummary {
	const busy = lanes.filter(
		(l) => l.state === "waiting" || l.state === "running",
	);
	if (busy.length > 0) {
		const phrases = [...new Set(busy.map(busyPhrase))];
		const [first, second] = phrases;
		const text =
			phrases.length === 1
				? first
				: phrases.length === 2
					? `${first} and ${lowerFirst(second)}`
					: `${first} and ${phrases.length - 1} more`;
		return { state: "busy", text };
	}
	if (lanes.some((l) => l.state === "paused")) {
		return { state: "paused", text: "Paused" };
	}
	const problems = lanes.reduce((sum, l) => sum + l.failed.length, 0);
	if (problems > 0) {
		return { state: "problem", text: plural(problems, "problem", "problems") };
	}
	return { state: "idle", text: "" };
}
