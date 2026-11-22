import { DotsThreeIcon } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import { Menu, MenuItem, MenuTrigger, Popover } from "react-aria-components";
import { cx, IconButton, POPOVER } from "./ui";

export type Action = {
	id: string;
	label: string;
	run: () => void;
	danger?: boolean;
	icon?: ReactNode;
};

export function ActionMenu({
	label,
	actions,
}: {
	label: string;
	actions: Action[];
}) {
	return (
		<MenuTrigger>
			<IconButton label={label} className="size-6">
				<DotsThreeIcon />
			</IconButton>
			<Popover placement="bottom end" className={`min-w-44 p-1 ${POPOVER}`}>
				<Menu
					aria-label={label}
					onAction={(k) => actions.find((a) => a.id === k)?.run()}
					className="outline-none"
				>
					{actions.map((a) => (
						<MenuItem
							key={a.id}
							id={a.id}
							className={cx(
								"flex cursor-default items-center gap-2 rounded-md px-2.5 py-1.5 text-[13px] outline-none data-[focused]:bg-muted",
								a.danger && "text-problem",
							)}
						>
							{a.icon}
							{a.label}
						</MenuItem>
					))}
				</Menu>
			</Popover>
		</MenuTrigger>
	);
}
