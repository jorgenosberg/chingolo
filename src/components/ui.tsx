import { InfoIcon } from "@phosphor-icons/react";
import type { ReactNode } from "react";
import {
	Dialog,
	Heading,
	Input,
	Label,
	Modal,
	ModalOverlay,
	Button as RacButton,
	type ButtonProps as RacButtonProps,
	TextField,
	Tooltip,
	TooltipTrigger,
} from "react-aria-components";

const cx = (...parts: (string | false | null | undefined)[]) =>
	parts.filter(Boolean).join(" ");

const focus =
	"outline-none data-[focus-visible]:ring-2 data-[focus-visible]:ring-primary/60";

type Variant = "primary" | "secondary" | "ghost" | "danger";

const variants: Record<Variant, string> = {
	primary:
		"bg-primary text-primary-foreground data-[hovered]:opacity-90 data-[pressed]:opacity-80",
	secondary:
		"bg-muted text-foreground data-[hovered]:bg-border data-[pressed]:bg-border/80",
	ghost: "text-foreground data-[hovered]:bg-muted data-[pressed]:bg-border/70",
	danger:
		"bg-problem text-white data-[hovered]:opacity-90 data-[pressed]:opacity-80",
};

export function Button({
	variant = "secondary",
	className,
	...props
}: RacButtonProps & { variant?: Variant; className?: string }) {
	return (
		<RacButton
			{...props}
			className={cx(
				"inline-flex h-8 items-center justify-center gap-1.5 rounded-lg px-3 text-[13px] font-medium transition-colors",
				"data-[disabled]:opacity-40",
				focus,
				variants[variant],
				className,
			)}
		/>
	);
}

/** App shortcuts, all ⌘ + key. Keys are lowercase `KeyboardEvent.key` values. */
export const SHORTCUTS = {
	newChat: "n",
	search: "k",
	settings: ",",
	context: "j",
	library: "l",
	sidebar: "b",
	/** Find in the open chat. Only the chat view listens for it. */
	find: "f",
} as const;

/** Shows ⌘ + key as a small keycap, e.g. `<Kbd keys="k" />` renders "⌘K". `onPrimary` is for use on a primary-colored button. */
export function Kbd({
	keys,
	className,
	onPrimary,
}: {
	keys: string;
	className?: string;
	onPrimary?: boolean;
}) {
	return (
		<kbd
			className={cx(
				"inline-flex h-[18px] min-w-[18px] items-center justify-center gap-px rounded-[5px] border border-b-2 px-1 font-sans text-[11px] leading-none",
				onPrimary
					? "border-primary-foreground/25 bg-primary-foreground/10 text-primary-foreground/80"
					: "border-border bg-card text-muted-foreground",
				className,
			)}
		>
			⌘{keys.toUpperCase()}
		</kbd>
	);
}

/**
 * Tooltip timing for the whole app. The cursor has to rest on something for a moment before a
 * tooltip opens, so sweeping across a toolbar shows nothing. It closes quickly, so it doesn't
 * linger over the next thing. (react-aria keeps its own 500 ms warm-up after a tooltip closes,
 * so moving to a neighbour right after one showed opens that one at once.)
 */
export const TOOLTIP_DELAY = 600;
export const TOOLTIP_CLOSE_DELAY = 100;

/** Shows `label` (and a ⌘ shortcut, if any) in a tooltip when the trigger is hovered or focused. */
export function Tip({
	label,
	shortcut,
	isDisabled,
	children,
}: {
	label: string;
	shortcut?: string;
	isDisabled?: boolean;
	/** The trigger: a react-aria button. */
	children: ReactNode;
}) {
	return (
		<TooltipTrigger
			delay={TOOLTIP_DELAY}
			closeDelay={TOOLTIP_CLOSE_DELAY}
			isDisabled={isDisabled}
		>
			{children}
			<Tooltip offset={6} className={`flex items-center gap-2 ${TOOLTIP}`}>
				{label}
				{shortcut && <Kbd keys={shortcut} />}
			</Tooltip>
		</TooltipTrigger>
	);
}

/** Icon-only button. `label` names it for screen readers and shows as its tooltip. */
export function IconButton({
	label,
	shortcut,
	tooltipDisabled,
	className,
	children,
	...props
}: Omit<RacButtonProps, "children"> & {
	label: string;
	/** Key of a ⌘ shortcut. Shown in the tooltip. */
	shortcut?: string;
	/** For a button whose popover is open, where a tooltip would get in the way. */
	tooltipDisabled?: boolean;
	className?: string;
	children: ReactNode;
}) {
	const button = (
		<RacButton
			{...props}
			aria-label={label}
			className={cx(
				"inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors",
				"data-[hovered]:bg-muted data-[hovered]:text-foreground data-[pressed]:bg-border/70",
				"data-[disabled]:opacity-40",
				focus,
				className,
			)}
		>
			{children}
		</RacButton>
	);
	return (
		<Tip label={label} shortcut={shortcut} isDisabled={tooltipDisabled}>
			{button}
		</Tip>
	);
}

/** Surface of every popover and menu. Add padding at the use site. */
export const POPOVER = "rounded-xl border border-border bg-card shadow-lg";
/**
 * A row in a menu or list popover. Add `flex` and alignment at the use site. It lights up under
 * the pointer and on keyboard focus only, so nothing stays highlighted after a click.
 */
export const ITEM =
	"cursor-pointer rounded-md px-2.5 py-1.5 text-[13px] outline-none data-[hovered]:bg-muted data-[focus-visible]:bg-muted data-[disabled]:cursor-default data-[disabled]:opacity-50";
/** A short label that appears on hover. */
export const TOOLTIP =
	"rounded-md border border-border bg-card px-2 py-1 text-xs shadow-md";

export function TextInput({
	label,
	value,
	onChange,
	type = "text",
	placeholder,
	autoFocus,
	description,
	isInvalid,
}: {
	label: string;
	value: string;
	onChange: (v: string) => void;
	type?: string;
	placeholder?: string;
	autoFocus?: boolean;
	description?: string;
	isInvalid?: boolean;
}) {
	return (
		<TextField
			value={value}
			onChange={onChange}
			type={type}
			autoFocus={autoFocus}
			isInvalid={isInvalid}
			className="flex flex-col gap-1.5"
		>
			<Label className="text-[13px] font-medium">{label}</Label>
			<Input
				placeholder={placeholder}
				autoComplete="off"
				spellCheck={false}
				className={cx(
					"h-9 rounded-lg border border-input bg-card px-3 text-sm",
					"placeholder:text-muted-foreground/70 data-[invalid]:border-problem",
					focus,
				)}
			/>
			{description && (
				<p className="text-xs text-muted-foreground">{description}</p>
			)}
		</TextField>
	);
}

export function Modal_({
	isOpen,
	onOpenChange,
	title,
	children,
	width = "max-w-md",
	dismissable = true,
}: {
	isOpen: boolean;
	onOpenChange: (open: boolean) => void;
	title: string;
	children: ReactNode;
	width?: string;
	dismissable?: boolean;
}) {
	return (
		<ModalOverlay
			isOpen={isOpen}
			onOpenChange={onOpenChange}
			isDismissable={dismissable}
			isKeyboardDismissDisabled={!dismissable}
			className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-4 backdrop-blur-[2px]"
		>
			<Modal className={cx("w-full", width)}>
				<Dialog className="rounded-2xl border border-border bg-card p-6 shadow-xl outline-none">
					<Heading slot="title" className="mb-3 text-base font-semibold">
						{title}
					</Heading>
					{children}
				</Dialog>
			</Modal>
		</ModalOverlay>
	);
}

/** A neutral info box: one short message, with an optional title line. */
export function Note({
	title,
	children,
	className,
}: {
	title?: string;
	children: ReactNode;
	className?: string;
}) {
	return (
		<div
			role="status"
			className={cx(
				"flex items-start gap-2.5 rounded-lg border border-border bg-muted px-3 py-2.5 text-[13px] text-muted-foreground",
				className,
			)}
		>
			<InfoIcon className="mt-px size-5 shrink-0" />
			<div className="min-w-0">
				{title && <p className="font-medium text-foreground">{title}</p>}
				<p>{children}</p>
			</div>
		</div>
	);
}

/** A thin bar for work with a known size. Pass `max` of 0 for an empty bar. */
export function ProgressBar({
	value,
	max,
	label,
	className,
}: {
	value: number;
	max: number;
	label: string;
	className?: string;
}) {
	return (
		<div
			role="progressbar"
			aria-label={label}
			aria-valuemin={0}
			aria-valuemax={max}
			aria-valuenow={value}
			className={cx(
				"h-1.5 w-full overflow-hidden rounded-full bg-border",
				className,
			)}
		>
			<div
				className="h-full bg-primary transition-[width]"
				style={{ width: `${max ? Math.min(100, (value / max) * 100) : 0}%` }}
			/>
		</div>
	);
}

export function Spinner({
	className,
	decorative,
}: {
	className?: string;
	/** Hidden from screen readers, for when text next to it says what is going on. */
	decorative?: boolean;
}) {
	const ring = cx(
		"inline-block size-4 animate-spin rounded-full border-2 border-border border-t-primary motion-reduce:animate-none",
		className,
	);
	if (decorative) return <span aria-hidden="true" className={ring} />;
	return <span role="status" aria-label="Loading" className={ring} />;
}

export { cx, focus };
