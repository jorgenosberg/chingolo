import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Dialog, Heading, Modal, ModalOverlay } from "react-aria-components";
import { Streamdown } from "streamdown";
import changelog from "../../../CHANGELOG.md?raw";
import flag from "../../assets/mascot/whats-new.svg";
import { Button, cx } from "../../components/ui";
import { api } from "../../lib/api";
import { keys, useSettings } from "../../lib/queries";

type Section = {
	version: string;
	/** The opening paragraph of a version's notes, shown under the title on the intro page. */
	summary: string;
	body: string;
};
type Page = {
	key: string;
	version: string;
	/** From a `### Heading` in the changelog. Null for text with no heading. */
	heading: string | null;
	body: string;
};

/** Splits `CHANGELOG.md` at its `## X.Y.Z` headings, newest first as written. */
function sections(md: string): Section[] {
	const out: Section[] = [];
	for (const line of md.split("\n")) {
		const m = /^## (\d+\.\d+\.\d+)\s*$/.exec(line);
		if (m) out.push({ version: m[1], summary: "", body: "" });
		else if (out.length) out[out.length - 1].body += `${line}\n`;
	}
	return out.map((s) => {
		// A first paragraph that isn't a list or heading is the summary, if notes follow it.
		const [head, ...rest] = s.body.trim().split(/\n\s*\n/);
		if (rest.length > 0 && !/^(-|\*|#|\d+\.)/.test(head)) {
			return {
				...s,
				summary: head.replace(/\s+/g, " ").trim(),
				body: rest.join("\n\n").trim(),
			};
		}
		return { ...s, body: s.body.trim() };
	});
}

/**
 * One page per `### Heading` in a version's notes, plus one for any text before the first
 * heading. Notes with no headings are a single page.
 */
function pagesOf(s: Section): Page[] {
	// With a capture group, split gives [text before, heading, text, heading, text, ...].
	const parts = s.body.split(/^### (.+)$/m);
	const pages: Page[] = [];
	const first = parts[0].trim();
	if (first) {
		pages.push({
			key: `${s.version}-0`,
			version: s.version,
			heading: null,
			body: first,
		});
	}
	for (let i = 1; i < parts.length; i += 2) {
		const body = (parts[i + 1] ?? "").trim();
		if (body) {
			pages.push({
				key: `${s.version}-${i}`,
				version: s.version,
				heading: parts[i].trim(),
				body,
			});
		}
	}
	return pages;
}

/** Negative, zero or positive, like a sort comparator. */
function compare(a: string, b: string): number {
	const pa = a.split(".").map(Number);
	const pb = b.split(".").map(Number);
	for (let i = 0; i < 3; i++) {
		const d = (pa[i] ?? 0) - (pb[i] ?? 0);
		if (d !== 0) return d;
	}
	return 0;
}

const SECTIONS = sections(changelog);

/**
 * After an update: an intro page with the mascot, then the changelog entries newer than the
 * version the user last saw, one page per heading. "Skip" closes it at any point.
 *
 * A fresh install never sees it, because onboarding stores the running version. Installs from
 * before the setting existed count as 0.1.0.
 */
export function WhatsNew() {
	const qc = useQueryClient();
	const settings = useSettings();
	const [closed, setClosed] = useState(false);
	const [step, setStep] = useState(0);
	const app = settings.data?.appVersion;
	const seen = settings.data?.lastSeenVersion ?? "0.1.0";

	// Newest first, as written in the changelog.
	const due = useMemo(
		() =>
			app
				? SECTIONS.filter(
						(s) => compare(s.version, seen) > 0 && compare(s.version, app) <= 0,
					)
				: [],
		[app, seen],
	);
	const pages = useMemo(() => due.flatMap(pagesOf), [due]);
	// After a skipped release the intro shows the newest summary. The pages cover the rest.
	const summary = due[0]?.summary;
	const updated = app !== undefined && compare(app, seen) > 0;

	const remember = async () => {
		if (!app) return;
		await api.updateSettings({ lastSeenVersion: app });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	// Updated, but nothing was written for it: note the version so this stays quiet.
	// biome-ignore lint/correctness/useExhaustiveDependencies: `remember` only reads `app`
	useEffect(() => {
		if (updated && pages.length === 0) void remember().catch(() => {});
	}, [updated, pages.length, app]);

	if (!updated || pages.length === 0) return null;

	const close = () => {
		setClosed(true);
		void remember().catch(() => {});
	};

	const total = pages.length + 1; // the intro, then the notes
	const last = step === total - 1;
	const page = step > 0 ? pages[step - 1] : null;
	// Name the version on each page only when the user skipped over several releases.
	const manyVersions = new Set(pages.map((p) => p.version)).size > 1;

	return (
		<ModalOverlay
			isOpen={!closed}
			onOpenChange={(open) => !open && close()}
			isDismissable
			className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-4 backdrop-blur-[2px]"
		>
			<Modal className="w-full max-w-md">
				<Dialog className="rounded-2xl border border-border bg-card p-6 shadow-xl outline-none">
					{/* A fixed height so the buttons stay put from page to page. */}
					<div className="flex h-96 flex-col">
						{page ? (
							<>
								{manyVersions && (
									<p className="mb-1 text-xs text-muted-foreground">
										v{page.version}
									</p>
								)}
								<Heading slot="title" className="mb-3 text-base font-semibold">
									{page.heading ?? "What's changed"}
								</Heading>
								<div className="selectable chat-prose min-h-0 flex-1 overflow-y-auto">
									<Streamdown>{page.body}</Streamdown>
								</div>
							</>
						) : (
							<div className="flex flex-1 flex-col items-center justify-center gap-3 text-center">
								<img
									src={flag}
									alt=""
									aria-hidden="true"
									className="h-52 w-auto"
								/>
								<Heading slot="title" className="text-lg font-semibold">
									What's new in Chingolo v{app}
								</Heading>
								{summary && (
									<p className="max-w-sm text-sm text-muted-foreground">
										{summary}
									</p>
								)}
							</div>
						)}
					</div>

					<div className="mt-4 flex items-center justify-between">
						<div className="flex gap-1.5" aria-hidden="true">
							{Array.from({ length: total }, (_, i) => (
								<span
									// biome-ignore lint/suspicious/noArrayIndexKey: fixed list of dots
									key={i}
									className={cx(
										"size-1.5 rounded-full",
										i === step ? "bg-primary" : "bg-border",
									)}
								/>
							))}
						</div>
						<span className="sr-only">
							Page {step + 1} of {total}
						</span>
						<div className="flex gap-2">
							{!last && <Button onPress={close}>Skip</Button>}
							<Button
								variant="primary"
								autoFocus
								onPress={last ? close : () => setStep((s) => s + 1)}
							>
								{last ? "Done" : "Continue"}
							</Button>
						</div>
					</div>
				</Dialog>
			</Modal>
		</ModalOverlay>
	);
}
