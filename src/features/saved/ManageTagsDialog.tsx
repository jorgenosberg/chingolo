import { PencilSimpleIcon, TrashIcon } from "@phosphor-icons/react";
import { useDialogs } from "../../components/dialogs";
import { TagColorButton } from "../../components/TagColorButton";
import { Button, IconButton, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
import type { SavedTagInfo } from "../../lib/bindings";
import { useSavedTags } from "../../lib/queries";

/** Rename, recolour and delete tags. Renaming to a name another tag has merges the two. */
export function ManageTagsDialog({
	onClose,
	onChanged,
}: {
	onClose: () => void;
	/** Called after every change, to refresh the lists. */
	onChanged: () => void;
}) {
	const tags = useSavedTags();
	const { prompt, confirm, alert } = useDialogs();

	const attempt = async (title: string, fn: () => Promise<unknown>) => {
		try {
			await fn();
		} catch (e) {
			await alert({
				title,
				message: e instanceof Error ? e.message : String(e),
			});
		}
		onChanged();
	};

	const rename = async (t: SavedTagInfo) => {
		const name = await prompt({
			title: "Rename tag",
			label: "Tag name",
			initial: t.name,
			submitLabel: "Rename",
		});
		if (!name || name === t.name) return;
		const same = tags.data?.find(
			(x) => x.id !== t.id && x.name.toLowerCase() === name.toLowerCase(),
		);
		if (
			same &&
			!(await confirm({
				title: `Merge into “${same.name}”?`,
				message: `“${t.name}” is removed. Its items get “${same.name}” instead.`,
				confirmLabel: "Merge",
			}))
		)
			return;
		await attempt("Couldn't rename", () => api.renameSavedTag(t.id, name));
	};

	const remove = async (t: SavedTagInfo) => {
		const ok = await confirm({
			title: `Delete “${t.name}”?`,
			message:
				t.itemCount === 0
					? "No saved item has this tag."
					: `It is taken off ${t.itemCount === 1 ? "1 saved item" : `${t.itemCount} saved items`}. The items stay.`,
			confirmLabel: "Delete",
			danger: true,
		});
		if (!ok) return;
		await attempt("Couldn't delete", () => api.deleteSavedTag(t.id));
	};

	return (
		<Modal_
			isOpen
			title="Manage tags"
			onOpenChange={(open) => !open && onClose()}
		>
			<div className="mb-4 flex max-h-80 flex-col gap-0.5 overflow-y-auto">
				{tags.data?.map((t) => (
					<div
						key={t.id}
						className="flex items-center gap-2 rounded-md px-1 py-1 hover:bg-muted"
					>
						<TagColorButton
							color={t.color}
							label={`Colour of ${t.name}`}
							onChange={(c) =>
								attempt("Couldn't change the colour", () =>
									api.setSavedTagColor(t.id, c),
								)
							}
						/>
						<span className="min-w-0 flex-1 truncate text-[13px]">
							{t.name}
						</span>
						<span className="shrink-0 text-xs text-muted-foreground">
							{t.itemCount === 1 ? "1 item" : `${t.itemCount} items`}
						</span>
						<IconButton label="Rename" onPress={() => rename(t)}>
							<PencilSimpleIcon />
						</IconButton>
						<IconButton label="Delete" onPress={() => remove(t)}>
							<TrashIcon />
						</IconButton>
					</div>
				))}
				{tags.isSuccess && tags.data.length === 0 && (
					<p className="text-sm text-muted-foreground">
						No tags yet. Add tags from a saved item's menu.
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
