import { type ReactNode, useEffect, useRef, useState } from "react";
import { Button as RacButton } from "react-aria-components";
import { SendIcon, StopIcon } from "../../components/icons";
import { cx, focus } from "../../components/ui";

// Unsent text per conversation, kept for the session.
const drafts = new Map<string, string>();

/** Puts text in a conversation's composer before it is opened. */
export function setDraft(conversationId: string, text: string) {
	if (text) drafts.set(conversationId, text);
	else drafts.delete(conversationId);
}

export function Composer({
	conversationId,
	busy,
	canStop,
	onSend,
	onStop,
	extra,
}: {
	conversationId: string;
	busy: boolean;
	canStop: boolean;
	/** Return false if the message wasn't sent, so the text stays in the box. */
	onSend: (text: string) => Promise<boolean>;
	onStop: () => void;
	/** Controls shown next to the send button, such as the model picker. */
	extra?: ReactNode;
}) {
	const [text, setText] = useState(() => drafts.get(conversationId) ?? "");
	const ref = useRef<HTMLTextAreaElement>(null);

	// Switching chats swaps the draft.
	useEffect(() => {
		setText(drafts.get(conversationId) ?? "");
		ref.current?.focus();
	}, [conversationId]);

	// Grow with the text, up to about eight lines.
	// biome-ignore lint/correctness/useExhaustiveDependencies: resize whenever the text changes
	useEffect(() => {
		const el = ref.current;
		if (!el) return;
		el.style.height = "auto";
		el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
	}, [text]);

	const update = (v: string) => {
		setText(v);
		if (v) drafts.set(conversationId, v);
		else drafts.delete(conversationId);
	};

	const submit = async () => {
		const value = text.trim();
		if (!value || busy) return;
		update("");
		const sent = await onSend(value);
		if (!sent) update(value);
	};

	return (
		<form
			onSubmit={(e) => {
				e.preventDefault();
				submit();
			}}
			className="mx-auto flex w-full max-w-3xl items-end gap-2 rounded-2xl border border-input bg-card px-3 py-2 shadow-sm focus-within:border-primary/60"
		>
			<textarea
				ref={ref}
				value={text}
				rows={1}
				aria-label="Message"
				placeholder="Ask a question"
				onChange={(e) => update(e.target.value)}
				onKeyDown={(e) => {
					if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
						e.preventDefault();
						submit();
					}
				}}
				className="max-h-[200px] min-h-7 flex-1 resize-none bg-transparent py-1 text-[15px] leading-relaxed outline-none placeholder:text-muted-foreground/70"
			/>
			{extra}
			{busy ? (
				<RacButton
					aria-label="Stop"
					isDisabled={!canStop}
					onPress={onStop}
					className={cx(
						"flex size-8 shrink-0 items-center justify-center rounded-full bg-foreground text-background data-[disabled]:opacity-40",
						focus,
					)}
				>
					<StopIcon />
				</RacButton>
			) : (
				<RacButton
					type="submit"
					aria-label="Send"
					isDisabled={!text.trim()}
					className={cx(
						"flex size-8 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground data-[disabled]:opacity-30",
						focus,
					)}
				>
					<SendIcon />
				</RacButton>
			)}
		</form>
	);
}
