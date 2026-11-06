import { CheckIcon } from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { Checkbox } from "react-aria-components";
import { Button, cx, focus, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
import type { Document } from "../../lib/bindings";
import { keys, useProjects } from "../../lib/queries";

/** Choose which projects a document belongs to. Each toggle applies right away. */
export function ProjectsDialog({
	doc,
	onClose,
	onError,
}: {
	doc: Document;
	onClose: () => void;
	onError: (message: string) => void;
}) {
	const qc = useQueryClient();
	const projects = useProjects();

	const toggle = async (projectId: string, on: boolean) => {
		try {
			if (on) await api.attachDocument(projectId, doc.id);
			else await api.detachDocument(projectId, doc.id);
		} catch (e) {
			onError(e instanceof Error ? e.message : String(e));
		}
		qc.invalidateQueries({ queryKey: keys.documents });
	};

	return (
		<Modal_ isOpen title="Projects" onOpenChange={(open) => !open && onClose()}>
			<p className="mb-3 text-xs text-muted-foreground">
				A document can be in several projects. Taking it out of a project keeps
				it in the library.
			</p>
			<div className="mb-5 flex max-h-64 flex-col gap-1 overflow-y-auto">
				{projects.data?.map((p) => (
					<Checkbox
						key={p.id}
						isSelected={doc.projectIds.includes(p.id)}
						onChange={(on) => toggle(p.id, on)}
						className={cx(
							"group flex cursor-default items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] data-[hovered]:bg-muted",
							focus,
						)}
					>
						<span className="flex size-4 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[selected]:border-primary group-data-[selected]:bg-primary">
							<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
						</span>
						<span className="truncate">{p.name}</span>
					</Checkbox>
				))}
				{projects.isSuccess && projects.data.length === 0 && (
					<p className="text-sm text-muted-foreground">
						No projects yet. Create one in the sidebar first.
					</p>
				)}
			</div>
			<div className="flex justify-end">
				<Button variant="primary" onPress={onClose}>
					Done
				</Button>
			</div>
		</Modal_>
	);
}
