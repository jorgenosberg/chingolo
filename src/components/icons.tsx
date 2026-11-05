import type { SVGProps } from "react";

type P = SVGProps<SVGSVGElement>;

function Icon({ children, ...p }: P) {
	return (
		<svg
			width="16"
			height="16"
			viewBox="0 0 16 16"
			fill="none"
			stroke="currentColor"
			strokeWidth="1.5"
			strokeLinecap="round"
			strokeLinejoin="round"
			aria-hidden="true"
			{...p}
		>
			{children}
		</svg>
	);
}

export const PlusIcon = (p: P) => (
	<Icon {...p}>
		<path d="M8 3v10M3 8h10" />
	</Icon>
);
export const SearchIcon = (p: P) => (
	<Icon {...p}>
		<circle cx="7" cy="7" r="4.25" />
		<path d="m10.25 10.25 3 3" />
	</Icon>
);
export const SettingsIcon = (p: P) => (
	<Icon {...p}>
		<circle cx="8" cy="8" r="2" />
		<path d="M8 1.75v1.5M8 12.75v1.5M1.75 8h1.5M12.75 8h1.5M3.6 3.6l1.05 1.05M11.35 11.35l1.05 1.05M3.6 12.4l1.05-1.05M11.35 4.65l1.05-1.05" />
	</Icon>
);
export const ChevronIcon = (p: P) => (
	<Icon {...p}>
		<path d="m6 4 4 4-4 4" />
	</Icon>
);
export const MoreIcon = (p: P) => (
	<Icon {...p} fill="currentColor" stroke="none">
		<circle cx="3.5" cy="8" r="1.1" />
		<circle cx="8" cy="8" r="1.1" />
		<circle cx="12.5" cy="8" r="1.1" />
	</Icon>
);
export const SidebarRightIcon = (p: P) => (
	<Icon {...p}>
		<rect x="2" y="3" width="12" height="10" rx="2" />
		<path d="M10 3v10" />
	</Icon>
);
export const SendIcon = (p: P) => (
	<Icon {...p}>
		<path d="M8 13V3M3.75 7.25 8 3l4.25 4.25" />
	</Icon>
);
export const StopIcon = (p: P) => (
	<Icon {...p} fill="currentColor" stroke="none">
		<rect x="4" y="4" width="8" height="8" rx="1.5" />
	</Icon>
);
export const CopyIcon = (p: P) => (
	<Icon {...p}>
		<rect x="5.5" y="5.5" width="8" height="8" rx="1.75" />
		<path d="M10.5 3.5v-.25A1.25 1.25 0 0 0 9.25 2h-5A1.25 1.25 0 0 0 3 3.25v5A1.25 1.25 0 0 0 4.25 9.5" />
	</Icon>
);
export const FolderIcon = (p: P) => (
	<Icon {...p}>
		<path d="M2 4.75A1.75 1.75 0 0 1 3.75 3h2.2c.4 0 .78.16 1.06.44L7.5 4h4.75A1.75 1.75 0 0 1 14 5.75v5.5A1.75 1.75 0 0 1 12.25 13h-8.5A1.75 1.75 0 0 1 2 11.25z" />
	</Icon>
);
export const ChatIcon = (p: P) => (
	<Icon {...p}>
		<path d="M3.5 3h9A1.5 1.5 0 0 1 14 4.5v5a1.5 1.5 0 0 1-1.5 1.5H8l-3 2.5V11h-1.5A1.5 1.5 0 0 1 2 9.5v-5A1.5 1.5 0 0 1 3.5 3Z" />
	</Icon>
);
export const ForkIcon = (p: P) => (
	<Icon {...p}>
		<circle cx="4.5" cy="3.5" r="1.5" />
		<circle cx="4.5" cy="12.5" r="1.5" />
		<circle cx="11.5" cy="5.5" r="1.5" />
		<path d="M4.5 5v6M11.5 7c0 2-2 2.5-7 3.5" />
	</Icon>
);
export const SparkleIcon = (p: P) => (
	<Icon {...p}>
		<path d="M8 2.5c.45 3 1.6 4.05 4.5 5.5-2.9 1.45-4.05 2.5-4.5 5.5-.45-3-1.6-4.05-4.5-5.5C6.4 6.55 7.55 5.5 8 2.5Z" />
	</Icon>
);
export const InfoIcon = (p: P) => (
	<Icon {...p}>
		<circle cx="8" cy="8" r="6" />
		<path d="M8 7.25v3.5M8 5.2v.05" />
	</Icon>
);
export const CheckIcon = (p: P) => (
	<Icon {...p}>
		<path d="m3.5 8.5 3 3 6-7" />
	</Icon>
);
export const LibraryIcon = (p: P) => (
	<Icon {...p}>
		<path d="M3.5 2.5h3v11h-3zM6.5 2.5h3v11h-3z" />
		<path d="m10.6 3.4 2.8-.7 2.1 9.7-2.8.7z" transform="translate(-1 .4)" />
	</Icon>
);
