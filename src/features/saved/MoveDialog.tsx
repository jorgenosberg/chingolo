import {
	CheckIcon,
	FolderIcon,
	FolderMinusIcon,
	FolderSimpleIcon,
} from "@phosphor-icons/react";
import { Button as RacButton } from "react-aria-components";
import { Button, cx, focus, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
import type { SavedItem } from "../../lib/bindings";
import { useAllChapters, useProjects } from "../../lib/queries";

function Row({
	icon,
	label,
	indent,
	current,
	onPress,
}: {
	icon: React.ReactNode;
	label: string;
	indent?: boolean;
	current: boolean;
	onPress: () => void;
}) {
	return (
		<RacButton
			onPress={onPress}
			className={cx(
				"flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] data-[hovered]:bg-muted",
				indent && "pl-8",
				focus,
			)}
		>
			<span className="shrink-0 text-muted-foreground">{icon}</span>
			<span className="min-w-0 flex-1 truncate">{label}</span>
			{current && <CheckIcon className="size-4 shrink-0 text-primary" />}
		</RacButton>
	);
}

/**
 * Choose a project, and a folder in it, for one or more saved items. Picking a row moves them
 * and closes. A check marks where all of them already are.
 */
export function MoveDialog({
	items,
	onClose,
	onMoved,
	onError,
}: {
	items: SavedItem[];
	onClose: () => void;
	onMoved: () => void;
	onError: (message: string) => void;
}) {
	const projects = useProjects();
	const chapters = useAllChapters();

	const move = async (projectId: string | null, chapterId: string | null) => {
		try {
			await api.moveSavedItems(
				items.map((i) => i.id),
				projectId,
				chapterId,
			);
			onMoved();
			onClose();
		} catch (e) {
			onError(e instanceof Error ? e.message : String(e));
		}
	};

	const here = (projectId: string | null, chapterId: string | null) =>
		items.length > 0 &&
		items.every((i) => i.projectId === projectId && i.chapterId === chapterId);

	return (
		<Modal_
			isOpen
			title={
				items.length === 1 ? "Move to" : `Move ${items.length} saved items to`
			}
			onOpenChange={(open) => !open && onClose()}
		>
			<div className="mb-3 flex max-h-80 flex-col gap-0.5 overflow-y-auto">
				<Row
					icon={<FolderMinusIcon className="size-4" />}
					label="No project"
					current={here(null, null)}
					onPress={() => move(null, null)}
				/>
				{projects.data?.map((p) => (
					<div key={p.id} className="flex flex-col gap-0.5">
						<Row
							icon={<FolderSimpleIcon className="size-4" />}
							label={p.name}
							current={here(p.id, null)}
							onPress={() => move(p.id, null)}
						/>
						{chapters
							.filter((c) => c.projectId === p.id)
							.map((c) => (
								<Row
									key={c.id}
									indent
									icon={<FolderIcon className="size-4" />}
									label={c.title}
									current={here(p.id, c.id)}
									onPress={() => move(p.id, c.id)}
								/>
							))}
					</div>
				))}
			</div>
			<p className="mb-3 text-xs text-muted-foreground">
				The project row keeps items out of any folder.
			</p>
			<div className="flex justify-end">
				<Button onPress={onClose}>Cancel</Button>
			</div>
		</Modal_>
	);
}
