import { CheckIcon, MinusIcon, PlusIcon } from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { Checkbox } from "react-aria-components";
import { useDialogs } from "../../components/dialogs";
import { Button, cx, focus, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
import type { Document } from "../../lib/bindings";
import { keys, useProjects } from "../../lib/queries";

/**
 * Choose which projects one or more documents belong to. Each toggle applies right away.
 * With several documents a project can be in some of them, which shows as a mixed box.
 */
export function ProjectsDialog({
	docs,
	onClose,
	onError,
}: {
	/** Read from the live list by the caller, so the boxes follow each change. */
	docs: Document[];
	onClose: () => void;
	onError: (message: string) => void;
}) {
	const qc = useQueryClient();
	const { prompt } = useDialogs();
	const projects = useProjects();
	const ids = docs.map((d) => d.id);

	const fail = (e: unknown) =>
		onError(e instanceof Error ? e.message : String(e));
	const refresh = () => qc.invalidateQueries({ queryKey: keys.documents });

	const toggle = async (projectId: string, on: boolean) => {
		try {
			await api.setProjectDocuments(projectId, ids, on);
		} catch (e) {
			fail(e);
		}
		refresh();
	};

	const create = async () => {
		const name = await prompt({
			title: "New project",
			label: "Project name",
			submitLabel: "Create",
		});
		if (!name) return;
		try {
			const p = await api.createProject(name);
			await api.setProjectDocuments(p.id, ids, true);
			qc.invalidateQueries({ queryKey: keys.projects });
		} catch (e) {
			fail(e);
		}
		refresh();
	};

	return (
		<Modal_
			isOpen
			title={
				docs.length === 1 ? "Projects" : `Projects for ${docs.length} documents`
			}
			onOpenChange={(open) => !open && onClose()}
		>
			<p className="mb-3 text-xs text-muted-foreground">
				A document can be in several projects. Taking it out of a project keeps
				it in the library.
			</p>
			<div className="mb-3 flex max-h-64 flex-col gap-1 overflow-y-auto">
				{projects.data?.map((p) => {
					const n = docs.filter((d) => d.projectIds.includes(p.id)).length;
					return (
						<Checkbox
							key={p.id}
							isSelected={docs.length > 0 && n === docs.length}
							isIndeterminate={n > 0 && n < docs.length}
							onChange={(on) => toggle(p.id, on)}
							className={cx(
								"group flex cursor-pointer data-[disabled]:cursor-default items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] data-[hovered]:bg-muted",
								focus,
							)}
						>
							<span className="relative flex size-4 shrink-0 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[indeterminate]:border-primary group-data-[selected]:border-primary group-data-[indeterminate]:bg-primary group-data-[selected]:bg-primary">
								<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
								<MinusIcon className="absolute size-4 opacity-0 group-data-[indeterminate]:opacity-100" />
							</span>
							<span className="truncate">{p.name}</span>
							{n > 0 && n < docs.length && (
								<span className="shrink-0 text-xs text-muted-foreground">
									{n} of {docs.length}
								</span>
							)}
						</Checkbox>
					);
				})}
				{projects.isSuccess && projects.data.length === 0 && (
					<p className="text-sm text-muted-foreground">No projects yet.</p>
				)}
			</div>
			<div className="flex items-center justify-between">
				<Button variant="ghost" onPress={create}>
					<PlusIcon className="size-4" /> New project…
				</Button>
				<Button variant="primary" onPress={onClose}>
					Done
				</Button>
			</div>
		</Modal_>
	);
}
