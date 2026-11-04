import {
	createContext,
	type ReactNode,
	useCallback,
	useContext,
	useState,
} from "react";
import { Button, Modal_, Spinner, TextInput } from "./ui";

type PromptOpts = {
	title: string;
	label: string;
	initial?: string;
	submitLabel?: string;
	placeholder?: string;
};
type ConfirmOpts = {
	title: string;
	message: string;
	confirmLabel: string;
	danger?: boolean;
};

type Dialogs = {
	/** Resolves with the trimmed text, or null if cancelled. */
	prompt: (o: PromptOpts) => Promise<string | null>;
	confirm: (o: ConfirmOpts) => Promise<boolean>;
	alert: (o: { title: string; message: string }) => Promise<void>;
	/** Shows a progress dialog while `fn` runs. */
	busy: <T>(message: string, fn: () => Promise<T>) => Promise<T>;
};

const Ctx = createContext<Dialogs | null>(null);

export function useDialogs(): Dialogs {
	const v = useContext(Ctx);
	if (!v) throw new Error("useDialogs needs <DialogProvider>");
	return v;
}

type Active =
	| { kind: "prompt"; opts: PromptOpts; resolve: (v: string | null) => void }
	| { kind: "confirm"; opts: ConfirmOpts; resolve: (v: boolean) => void }
	| {
			kind: "alert";
			opts: { title: string; message: string };
			resolve: () => void;
	  };

export function DialogProvider({ children }: { children: ReactNode }) {
	const [active, setActive] = useState<Active | null>(null);
	const [text, setText] = useState("");
	const [working, setWorking] = useState<string | null>(null);

	const prompt = useCallback(
		(opts: PromptOpts) =>
			new Promise<string | null>((resolve) => {
				setText(opts.initial ?? "");
				setActive({ kind: "prompt", opts, resolve });
			}),
		[],
	);
	const confirm = useCallback(
		(opts: ConfirmOpts) =>
			new Promise<boolean>((resolve) =>
				setActive({ kind: "confirm", opts, resolve }),
			),
		[],
	);

	const alert = useCallback(
		(opts: { title: string; message: string }) =>
			new Promise<void>((resolve) =>
				setActive({ kind: "alert", opts, resolve }),
			),
		[],
	);
	const busy = useCallback(
		async <T,>(message: string, fn: () => Promise<T>) => {
			setWorking(message);
			try {
				return await fn();
			} finally {
				setWorking(null);
			}
		},
		[],
	);

	const close = () => setActive(null);

	return (
		<Ctx.Provider value={{ prompt, confirm, alert, busy }}>
			{children}
			{working && (
				<Modal_
					isOpen
					title={working}
					onOpenChange={() => {}}
					dismissable={false}
				>
					<div className="flex items-center gap-3 text-sm text-muted-foreground">
						<Spinner /> This can take a few seconds.
					</div>
				</Modal_>
			)}
			{active?.kind === "alert" && (
				<Modal_
					isOpen
					title={active.opts.title}
					onOpenChange={() => {
						active.resolve();
						close();
					}}
				>
					<p role="alert" className="mb-5 text-sm text-muted-foreground">
						{active.opts.message}
					</p>
					<div className="flex justify-end">
						<Button
							variant="primary"
							autoFocus
							onPress={() => {
								active.resolve();
								close();
							}}
						>
							OK
						</Button>
					</div>
				</Modal_>
			)}
			{active?.kind === "prompt" && (
				<Modal_
					isOpen
					title={active.opts.title}
					onOpenChange={(open) => {
						if (!open) {
							active.resolve(null);
							close();
						}
					}}
				>
					<form
						className="flex flex-col gap-4"
						onSubmit={(e) => {
							e.preventDefault();
							const v = text.trim();
							if (!v) return;
							active.resolve(v);
							close();
						}}
					>
						<TextInput
							label={active.opts.label}
							value={text}
							onChange={setText}
							placeholder={active.opts.placeholder}
							autoFocus
						/>
						<div className="flex justify-end gap-2">
							<Button
								variant="ghost"
								onPress={() => {
									active.resolve(null);
									close();
								}}
							>
								Cancel
							</Button>
							<Button variant="primary" type="submit" isDisabled={!text.trim()}>
								{active.opts.submitLabel ?? "Save"}
							</Button>
						</div>
					</form>
				</Modal_>
			)}
			{active?.kind === "confirm" && (
				<Modal_
					isOpen
					title={active.opts.title}
					onOpenChange={(open) => {
						if (!open) {
							active.resolve(false);
							close();
						}
					}}
				>
					<p className="mb-5 text-sm text-muted-foreground">
						{active.opts.message}
					</p>
					<div className="flex justify-end gap-2">
						<Button
							variant="ghost"
							autoFocus
							onPress={() => {
								active.resolve(false);
								close();
							}}
						>
							Cancel
						</Button>
						<Button
							variant={active.opts.danger ? "danger" : "primary"}
							onPress={() => {
								active.resolve(true);
								close();
							}}
						>
							{active.opts.confirmLabel}
						</Button>
					</div>
				</Modal_>
			)}
		</Ctx.Provider>
	);
}
