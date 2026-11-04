import { Menu, MenuItem, MenuTrigger, Popover } from "react-aria-components";
import { MoreIcon } from "./icons";
import { cx, IconButton } from "./ui";

export type Action = {
	id: string;
	label: string;
	run: () => void;
	danger?: boolean;
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
				<MoreIcon />
			</IconButton>
			<Popover
				placement="bottom end"
				className="min-w-44 rounded-xl border border-border bg-card p-1 shadow-lg"
			>
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
								"cursor-default rounded-md px-2.5 py-1.5 text-[13px] outline-none data-[focused]:bg-muted",
								a.danger && "text-problem",
							)}
						>
							{a.label}
						</MenuItem>
					))}
				</Menu>
			</Popover>
		</MenuTrigger>
	);
}
