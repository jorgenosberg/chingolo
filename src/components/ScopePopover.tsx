import { useMemo, useState } from "react";
import {
	Dialog,
	DialogTrigger,
	Input,
	ListBox,
	ListBoxItem,
	Popover,
	Button as RacButton,
	type Selection,
} from "react-aria-components";
import type { Document, Scope } from "../lib/bindings";
import { CheckIcon, LibraryIcon } from "./icons";
import { cx, focus } from "./ui";

type Kind = "project" | "library" | "documents";

const STATUS_NOTE: Record<Document["status"], string> = {
	queued: "Waiting",
	processing: "Reading",
	ready: "",
	no_text: "No text found",
	error: "Failed",
	cancelled: "Cancelled",
};

/**
 * What the conversation may read: its project, the whole library or chosen documents.
 * `value` is the stored scope, null meaning the default (the project if it has ready
 * documents, else the library).
 */
export function ScopePopover({
	projectId,
	value,
	documents,
	onChange,
	isDisabled,
}: {
	projectId: string | null;
	value: Scope | null;
	documents: Document[];
	onChange: (scope: Scope | null) => void;
	isDisabled?: boolean;
}) {
	const [open, setOpen] = useState(false);
	const [picking, setPicking] = useState(false);
	const [draft, setDraft] = useState<Set<string>>(new Set());
	const [filter, setFilter] = useState("");

	const ready = (d: Document) => d.status === "ready";
	const projectHasReady =
		!!projectId &&
		documents.some((d) => ready(d) && d.projectIds.includes(projectId));
	const kind: Kind = value
		? value.kind
		: projectHasReady
			? "project"
			: "library";
	const known = new Set(documents.map((d) => d.id));
	const chosen =
		value?.kind === "documents" ? value.ids.filter((i) => known.has(i)) : [];

	const label =
		kind === "project"
			? "This project"
			: kind === "library"
				? "Whole library"
				: `${chosen.length} ${chosen.length === 1 ? "document" : "documents"}`;

	const shown = useMemo(() => {
		const q = filter.trim().toLowerCase();
		return documents.filter((d) => {
			if (!q) return true;
			return `${d.title ?? ""} ${d.originalName} ${d.authors.join(" ")}`
				.toLowerCase()
				.includes(q);
		});
	}, [documents, filter]);

	const startPicking = () => {
		setDraft(new Set(chosen));
		setFilter("");
		setPicking(true);
	};

	const choose = (next: Scope | null) => {
		setOpen(false);
		onChange(next);
	};

	const options: { kind: Kind; name: string; note: string }[] = [
		...(projectId
			? [
					{
						kind: "project" as const,
						name: "This project",
						note: "Documents attached to this project",
					},
				]
			: []),
		{
			kind: "library",
			name: "Whole library",
			note: "Every document that has been read",
		},
		{
			kind: "documents",
			name: "Chosen documents",
			note: "Pick the papers to use",
		},
	];

	return (
		<DialogTrigger
			isOpen={open}
			onOpenChange={(o) => {
				setOpen(o);
				if (!o) setPicking(false);
			}}
		>
			<RacButton
				aria-label={`Sources: ${label}`}
				isDisabled={isDisabled}
				className={cx(
					"inline-flex h-8 items-center gap-1.5 rounded-full px-2.5 text-xs text-muted-foreground transition-colors",
					"data-[hovered]:bg-muted data-[hovered]:text-foreground data-[disabled]:opacity-40",
					focus,
				)}
			>
				<LibraryIcon className="size-3.5" />
				<span className="max-w-32 truncate">{label}</span>
			</RacButton>
			<Popover
				placement="top end"
				offset={10}
				className="w-80 rounded-xl border border-border bg-card p-1.5 shadow-lg"
			>
				<Dialog aria-label="Sources" className="outline-none">
					{picking ? (
						<div className="flex flex-col gap-1.5">
							<Input
								aria-label="Search documents"
								placeholder="Search documents"
								value={filter}
								onChange={(e) => setFilter(e.target.value)}
								className={cx(
									"mx-1 mt-1 rounded-md border border-input bg-background px-2 py-1 text-[13px]",
									focus,
								)}
							/>
							<ListBox
								aria-label="Documents"
								selectionMode="multiple"
								selectedKeys={draft}
								disabledKeys={documents
									.filter((d) => !ready(d))
									.map((d) => d.id)}
								onSelectionChange={(keys: Selection) => {
									if (keys === "all") return;
									setDraft(new Set([...keys].map(String)));
								}}
								renderEmptyState={() => (
									<p className="px-2.5 py-2 text-xs text-muted-foreground">
										No documents match.
									</p>
								)}
								className="max-h-64 overflow-auto outline-none"
							>
								{shown.map((d) => (
									<ListBoxItem
										key={d.id}
										id={d.id}
										textValue={d.title || d.originalName}
										className="flex cursor-default items-center justify-between gap-3 rounded-md px-2.5 py-1.5 text-[13px] outline-none data-[disabled]:opacity-50 data-[focused]:bg-muted"
									>
										{({ isSelected }) => (
											<>
												<span className="truncate">
													{d.title || d.originalName}
												</span>
												<span className="flex shrink-0 items-center gap-2 text-xs text-muted-foreground">
													{STATUS_NOTE[d.status]}
													{isSelected && (
														<CheckIcon className="size-3.5 text-primary" />
													)}
												</span>
											</>
										)}
									</ListBoxItem>
								))}
							</ListBox>
							<div className="flex items-center justify-between gap-2 px-1 pb-1">
								<RacButton
									onPress={() => setPicking(false)}
									className={cx(
										"rounded-md px-2 py-1 text-xs text-muted-foreground data-[hovered]:text-foreground",
										focus,
									)}
								>
									Back
								</RacButton>
								<RacButton
									isDisabled={draft.size === 0}
									onPress={() => choose({ kind: "documents", ids: [...draft] })}
									className={cx(
										"rounded-md bg-primary px-2.5 py-1 text-xs text-primary-foreground data-[disabled]:opacity-40",
										focus,
									)}
								>
									{draft.size === 0
										? "Choose documents"
										: `Use ${draft.size} ${draft.size === 1 ? "document" : "documents"}`}
								</RacButton>
							</div>
						</div>
					) : (
						<>
							<p className="px-2.5 pb-1 pt-1 text-xs font-medium text-muted-foreground">
								Sources for answers
							</p>
							<ListBox
								aria-label="Sources"
								selectionMode="single"
								disallowEmptySelection
								selectedKeys={[kind]}
								onSelectionChange={(keys) => {
									if (keys === "all") return;
									const next = String([...keys][0]) as Kind;
									if (next === "documents") return startPicking();
									if (next === kind && value) return;
									choose({ kind: next });
								}}
								className="outline-none"
							>
								{options.map((o) => (
									<ListBoxItem
										key={o.kind}
										id={o.kind}
										textValue={o.name}
										className="flex cursor-default items-start justify-between gap-3 rounded-md px-2.5 py-1.5 outline-none data-[focused]:bg-muted"
									>
										{({ isSelected }) => (
											<>
												<span className="flex flex-col">
													<span
														className={cx(
															"text-[13px]",
															isSelected && "font-medium",
														)}
													>
														{o.kind === "documents" && chosen.length > 0
															? `Chosen documents (${chosen.length})`
															: o.name}
													</span>
													<span className="text-xs text-muted-foreground">
														{o.note}
													</span>
												</span>
												{isSelected && (
													<CheckIcon className="mt-0.5 size-3.5 shrink-0 text-primary" />
												)}
											</>
										)}
									</ListBoxItem>
								))}
							</ListBox>
							{kind === "documents" && (
								<RacButton
									onPress={startPicking}
									className={cx(
										"mx-2.5 mb-1.5 mt-1 text-xs text-primary data-[hovered]:underline",
										focus,
									)}
								>
									Change the documents
								</RacButton>
							)}
						</>
					)}
				</Dialog>
			</Popover>
		</DialogTrigger>
	);
}
