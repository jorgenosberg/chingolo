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

/** Icon-only button. `label` is required so screen readers have a name. */
export function IconButton({
	label,
	className,
	children,
	...props
}: Omit<RacButtonProps, "children"> & {
	label: string;
	className?: string;
	children: ReactNode;
}) {
	return (
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
}

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

export function Spinner({ className }: { className?: string }) {
	return (
		<span
			role="status"
			aria-label="Loading"
			className={cx(
				"inline-block size-4 animate-spin rounded-full border-2 border-border border-t-primary",
				className,
			)}
		/>
	);
}

export { cx, focus };
