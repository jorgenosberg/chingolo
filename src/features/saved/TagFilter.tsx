import { CaretDownIcon, GearSixIcon, TagIcon } from "@phosphor-icons/react";
import { useState } from "react";
import {
	Checkbox,
	Dialog,
	DialogTrigger,
	Popover,
	Button as RacButton,
} from "react-aria-components";
import { TagDot } from "../../components/TagChip";
import { cx, focus, POPOVER } from "../../components/ui";
import { useSavedTags } from "../../lib/queries";
import { ManageTagsDialog } from "./ManageTagsDialog";

/** Pick tags to show. An item with any of the picked tags matches. */
export function TagFilter({
	value,
	onChange,
	onChanged,
}: {
	/** Tag ids. */
	value: string[];
	onChange: (ids: string[]) => void;
	/** Called after tags are renamed, recoloured or deleted, to refresh the lists. */
	onChanged: () => void;
}) {
	const tags = useSavedTags();
	const [managing, setManaging] = useState(false);
	const list = tags.data ?? [];
	const names = list.filter((t) => value.includes(t.id)).map((t) => t.name);
	const label =
		names.length === 0
			? "Any tag"
			: names.length === 1
				? names[0]
				: `${names.length} tags`;

	return (
		<>
			<DialogTrigger>
				<RacButton
					aria-label="Tags"
					className={cx(
						"flex h-8 items-center gap-2 rounded-lg border border-input bg-card px-2.5 text-[13px] data-[hovered]:bg-muted",
						focus,
					)}
				>
					<TagIcon className="size-4 shrink-0 text-muted-foreground" />
					<span className="max-w-48 truncate">{label}</span>
					<CaretDownIcon className="size-4 shrink-0 text-muted-foreground" />
				</RacButton>
				<Popover
					placement="bottom start"
					className={`w-64 max-w-[90vw] p-1 ${POPOVER}`}
				>
					<Dialog aria-label="Filter by tag" className="outline-none">
						<div className="flex max-h-64 flex-col overflow-y-auto">
							{list.map((t) => (
								<Checkbox
									key={t.id}
									isSelected={value.includes(t.id)}
									onChange={(on) =>
										onChange(
											on ? [...value, t.id] : value.filter((id) => id !== t.id),
										)
									}
									className={cx(
										"flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-[13px] data-[hovered]:bg-muted data-[selected]:font-medium",
										focus,
									)}
								>
									<TagDot color={t.color} />
									<span className="min-w-0 flex-1 truncate">{t.name}</span>
									<span className="shrink-0 text-xs font-normal text-muted-foreground">
										{t.itemCount}
									</span>
								</Checkbox>
							))}
							{tags.isSuccess && list.length === 0 && (
								<p className="px-2 py-1.5 text-[13px] text-muted-foreground">
									No tags yet.
								</p>
							)}
						</div>
						<div className="mt-1 flex items-center justify-between border-t border-border px-1 pt-1">
							<RacButton
								onPress={() => setManaging(true)}
								isDisabled={list.length === 0}
								className={cx(
									"flex items-center gap-1.5 rounded px-1.5 py-1 text-xs text-muted-foreground data-[hovered]:text-foreground data-[disabled]:opacity-40",
									focus,
								)}
							>
								<GearSixIcon className="size-3.5" /> Manage tags…
							</RacButton>
							{value.length > 0 && (
								<RacButton
									onPress={() => onChange([])}
									className={cx(
										"rounded px-1.5 py-1 text-xs text-primary underline-offset-2 hover:underline",
										focus,
									)}
								>
									Clear
								</RacButton>
							)}
						</div>
					</Dialog>
				</Popover>
			</DialogTrigger>
			{managing && (
				<ManageTagsDialog
					onClose={() => setManaging(false)}
					onChanged={onChanged}
				/>
			)}
		</>
	);
}
