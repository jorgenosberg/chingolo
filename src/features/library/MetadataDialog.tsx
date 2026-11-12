import { useState } from "react";
import { Label, TextArea, TextField } from "react-aria-components";
import { Button, cx, focus, Modal_, TextInput } from "../../components/ui";
import { api } from "../../lib/api";
import type { Document, MetadataPatch } from "../../lib/bindings";

const EXTRA_FIELDS = [
	{ key: "journal", label: "Journal", placeholder: "Harmful Algae" },
	{ key: "publisher", label: "Publisher", placeholder: "NIVA" },
	{ key: "series", label: "Report series", placeholder: "NIVA-rapport 7132-2017" },
	{ key: "volume", label: "Volume", placeholder: "118" },
	{ key: "issue", label: "Issue", placeholder: "3" },
	{ key: "pages", label: "Pages", placeholder: "105-114" },
	{ key: "isbn", label: "ISBN", placeholder: "978-82-577-6867-6" },
	{ key: "issn", label: "ISSN", placeholder: "1894-7948" },
] as const;

type Form = {
	title: string;
	authors: string;
	year: string;
	doi: string;
} & Record<(typeof EXTRA_FIELDS)[number]["key"], string>;

const SOURCE_NAMES: Record<string, string> = {
	file: "the file",
	crossref: "Crossref",
	ai: "a model",
};

/** Where the detected values came from, for the intro line. */
function sourceNote(doc: Document): string {
	const used = new Set(Object.values(doc.metaSource));
	const names = ["file", "crossref", "ai"]
		.filter((s) => used.has(s))
		.map((s) => SOURCE_NAMES[s]);
	return names.length ? names.join(", ") : "the file";
}

/** Builds a patch with only the fields the user changed. Empty text clears a field. */
function buildPatch(doc: Document, form: Form): Partial<MetadataPatch> {
	const patch: Partial<MetadataPatch> = {};
	for (const { key } of EXTRA_FIELDS) {
		const v = form[key].trim();
		if (v !== (doc[key] ?? "")) patch[key] = v;
	}
	const title = form.title.trim();
	if (title !== (doc.title ?? "")) patch.title = title;
	const authors = form.authors
		.split("\n")
		.map((a) => a.trim())
		.filter(Boolean);
	if (authors.join("\n") !== doc.authors.join("\n")) patch.authors = authors;
	const year = form.year.trim() === "" ? 0 : Number(form.year);
	if (year !== (doc.year ?? 0)) patch.year = year;
	const doi = form.doi.trim();
	if (doi !== (doc.doi ?? "")) patch.doi = doi;
	return patch;
}

export function MetadataDialog({
	doc,
	onClose,
	onSaved,
}: {
	doc: Document;
	onClose: () => void;
	onSaved: (d: Document) => void;
}) {
	const [form, setForm] = useState({
		title: doc.title ?? "",
		authors: doc.authors.join("\n"),
		year: doc.year ? String(doc.year) : "",
		doi: doc.doi ?? "",
		...(Object.fromEntries(
			EXTRA_FIELDS.map(({ key }) => [key, doc[key] ?? ""]),
		) as Record<(typeof EXTRA_FIELDS)[number]["key"], string>),
	});
	const [error, setError] = useState<string | null>(null);
	const [saving, setSaving] = useState(false);
	const set = (k: keyof Form) => (v: string) =>
		setForm((f) => ({ ...f, [k]: v }));

	const yearOk = form.year.trim() === "" || /^\d{4}$/.test(form.year.trim());

	const save = async () => {
		const patch = buildPatch(doc, form);
		if (Object.keys(patch).length === 0) return onClose();
		setSaving(true);
		setError(null);
		try {
			onSaved(await api.updateDocumentMetadata(doc.id, patch));
			onClose();
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
			setSaving(false);
		}
	};

	return (
		<Modal_
			isOpen
			title="Edit details"
			onOpenChange={(open) => !open && onClose()}
		>
			<form
				className="flex flex-col gap-4"
				onSubmit={(e) => {
					e.preventDefault();
					if (yearOk && !saving) save();
				}}
			>
				<p className="text-xs text-muted-foreground">
					Detected from {sourceNote(doc)} ({doc.originalName}). Fix anything
					that's wrong. Fields you change are kept when details are refreshed
					or the file is read again.
				</p>
				<TextInput
					label="Title"
					value={form.title}
					onChange={set("title")}
					placeholder={doc.originalName}
					autoFocus
				/>
				<TextField
					value={form.authors}
					onChange={set("authors")}
					className="flex flex-col gap-1.5"
				>
					<Label className="text-[13px] font-medium">Authors</Label>
					<TextArea
						rows={3}
						spellCheck={false}
						className={cx(
							"resize-none rounded-lg border border-input bg-card px-3 py-2 text-sm",
							focus,
						)}
					/>
					<p className="text-xs text-muted-foreground">One name per line.</p>
				</TextField>
				<div className="grid grid-cols-[6rem_1fr] gap-3">
					<TextInput
						label="Year"
						value={form.year}
						onChange={set("year")}
						placeholder="2004"
						isInvalid={!yearOk}
					/>
					<TextInput
						label="DOI"
						value={form.doi}
						onChange={set("doi")}
						placeholder="10.1000/xyz123"
					/>
				</div>
				<details className="group rounded-lg border border-border px-3 py-2">
					<summary className="cursor-default text-[13px] font-medium outline-none">
						More details
					</summary>
					<div className="mt-3 grid grid-cols-2 gap-3">
						{EXTRA_FIELDS.map(({ key, label, placeholder }) => (
							<TextInput
								key={key}
								label={label}
								value={form[key]}
								onChange={set(key)}
								placeholder={placeholder}
							/>
						))}
					</div>
				</details>
				{error && (
					<p role="alert" className="text-sm text-problem">
						{error}
					</p>
				)}
				<div className="flex justify-end gap-2">
					<Button variant="ghost" onPress={onClose}>
						Cancel
					</Button>
					<Button
						variant="primary"
						type="submit"
						isDisabled={!yearOk || saving}
					>
						Save
					</Button>
				</div>
			</form>
		</Modal_>
	);
}
