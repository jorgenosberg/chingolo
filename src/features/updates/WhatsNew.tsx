import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Streamdown } from "streamdown";
import changelog from "../../../CHANGELOG.md?raw";
import flag from "../../assets/mascot/whats-new.svg";
import { Button, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
import { keys, useSettings } from "../../lib/queries";

type Section = { version: string; body: string };

/** Splits `CHANGELOG.md` at its `## X.Y.Z` headings, newest first as written. */
function sections(md: string): Section[] {
	const out: Section[] = [];
	for (const line of md.split("\n")) {
		const m = /^## (\d+\.\d+\.\d+)\s*$/.exec(line);
		if (m) out.push({ version: m[1], body: "" });
		else if (out.length) out[out.length - 1].body += `${line}\n`;
	}
	return out.map((s) => ({ ...s, body: s.body.trim() }));
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
 * After an update, shows the changelog entries newer than the version the user last saw. A fresh
 * install never sees it: onboarding stores the running version. Installs from before the setting
 * existed count as 0.1.0.
 */
export function WhatsNew() {
	const qc = useQueryClient();
	const settings = useSettings();
	const [closed, setClosed] = useState(false);
	const app = settings.data?.appVersion;
	const seen = settings.data?.lastSeenVersion ?? "0.1.0";

	const due = useMemo(
		() =>
			app
				? SECTIONS.filter(
						(s) => compare(s.version, seen) > 0 && compare(s.version, app) <= 0,
					)
				: [],
		[app, seen],
	);
	const updated = app !== undefined && compare(app, seen) > 0;

	const remember = async () => {
		if (!app) return;
		await api.updateSettings({ lastSeenVersion: app });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	// Updated, but nothing was written for it: note the version so this stays quiet.
	// biome-ignore lint/correctness/useExhaustiveDependencies: `remember` only reads `app`
	useEffect(() => {
		if (updated && due.length === 0) void remember().catch(() => {});
	}, [updated, due.length, app]);

	if (!updated || due.length === 0) return null;

	const close = () => {
		setClosed(true);
		void remember().catch(() => {});
	};

	return (
		<Modal_
			isOpen={!closed}
			onOpenChange={(open) => !open && close()}
			title={`What's new in Chingolo ${app}`}
			width="max-w-lg"
		>
			<div className="max-h-[70vh] overflow-y-auto">
				<img
					src={flag}
					alt=""
					aria-hidden="true"
					className="mx-auto mb-2 h-44 w-auto"
				/>
				{due.map((s) => (
					<section key={s.version} className="selectable chat-prose">
						{due.length > 1 && (
							<h3 className="text-sm font-semibold">{s.version}</h3>
						)}
						<Streamdown>{s.body}</Streamdown>
					</section>
				))}
			</div>
			<div className="mt-4 flex justify-end">
				<Button variant="primary" onPress={close}>
					Got it
				</Button>
			</div>
		</Modal_>
	);
}
