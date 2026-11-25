import { CheckIcon, MinusIcon, PlusIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { Checkbox } from "react-aria-components";
import { TagDot } from "../../components/TagChip";
import { Button, cx, focus, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
import type { SavedItem } from "../../lib/bindings";
import { useSavedTags } from "../../lib/queries";

/**
 * Choose the tags of one or more saved items. Each change applies right away. With several items
 * a tag can be on some of them, which shows as a mixed box. A name that is new makes a tag.
 */
export function TagsDialog({
	items,
	onClose,
	onChanged,
	onError,
}: {
	/** Read from the live list by the caller, so the boxes follow each change. */
	items: SavedItem[];
	onClose: () => void;
	/** Called after every change, to refresh the lists. */
	onChanged: () => void;
	onError: (message: string) => void;
}) {
	const tags = useSavedTags();
	const [name, setName] = useState("");
	const ids = items.map((i) => i.id);

	const run = async (add: string[], remove: string[]) => {
		try {
			await api.tagSavedItems(ids, add, remove);
		} catch (e) {
			onError(e instanceof Error ? e.message : String(e));
		}
		onChanged();
	};

	const create = async () => {
		const n = name.trim();
		if (!n) return;
		setName("");
		await run([n], []);
	};

	return (
		<Modal_
			isOpen
			title={
				items.length === 1 ? "Tags" : `Tags for ${items.length} saved items`
			}
			onOpenChange={(open) => !open && onClose()}
		>
			<div className="mb-3 flex max-h-64 flex-col gap-1 overflow-y-auto">
				{tags.data?.map((t) => {
					const n = items.filter((i) =>
						i.tags.some((x) => x.id === t.id),
					).length;
					return (
						<Checkbox
							key={t.id}
							isSelected={items.length > 0 && n === items.length}
							isIndeterminate={n > 0 && n < items.length}
							onChange={(on) => run(on ? [t.name] : [], on ? [] : [t.id])}
							className={cx(
								"group flex cursor-pointer items-center gap-2.5 rounded-md px-2 py-1.5 text-[13px] data-[hovered]:bg-muted",
								focus,
							)}
						>
							<span className="relative flex size-4 shrink-0 items-center justify-center rounded border border-input bg-card text-primary-foreground group-data-[indeterminate]:border-primary group-data-[selected]:border-primary group-data-[indeterminate]:bg-primary group-data-[selected]:bg-primary">
								<CheckIcon className="size-4 opacity-0 group-data-[selected]:opacity-100" />
								<MinusIcon className="absolute size-4 opacity-0 group-data-[indeterminate]:opacity-100" />
							</span>
							<TagDot color={t.color} />
							<span className="truncate">{t.name}</span>
							{n > 0 && n < items.length && (
								<span className="shrink-0 text-xs text-muted-foreground">
									{n} of {items.length}
								</span>
							)}
						</Checkbox>
					);
				})}
				{tags.isSuccess && tags.data.length === 0 && (
					<p className="text-sm text-muted-foreground">
						No tags yet. Add the first one below.
					</p>
				)}
			</div>
			<form
				className="mb-4 flex items-center gap-2"
				onSubmit={(e) => {
					e.preventDefault();
					create();
				}}
			>
				<input
					aria-label="New tag"
					placeholder="New tag"
					value={name}
					maxLength={40}
					onChange={(e) => setName(e.target.value)}
					className="h-8 min-w-0 flex-1 rounded-lg border border-transparent bg-muted px-3 text-[13px] outline-none placeholder:text-muted-foreground focus:border-primary/50"
				/>
				<Button type="submit" isDisabled={!name.trim()}>
					<PlusIcon className="size-4" /> Add
				</Button>
			</form>
			<div className="flex justify-end">
				<Button variant="primary" onPress={onClose}>
					Done
				</Button>
			</div>
		</Modal_>
	);
}
