import { ArrowUpIcon, StopIcon } from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Button as RacButton } from "react-aria-components";
import { cx, focus, Tip } from "../../components/ui";
import { keys } from "../../lib/queries";
import { flushDraft, getDraft, setDraft } from "./drafts";

function toEnd(el: HTMLTextAreaElement | null) {
	el?.setSelectionRange(el.value.length, el.value.length);
}

export function Composer({
	conversationId,
	savedDraft,
	busy,
	canStop,
	onSend,
	onStop,
	extra,
}: {
	conversationId: string;
	/** The text saved with the conversation, for when nothing was typed this session. */
	savedDraft: string;
	busy: boolean;
	canStop: boolean;
	/** Return false if the message wasn't sent, so the text stays in the box. */
	onSend: (text: string) => Promise<boolean>;
	onStop: () => void;
	/** Controls shown next to the send button, such as the model picker. */
	extra?: ReactNode;
}) {
	const qc = useQueryClient();
	const [text, setText] = useState(() => getDraft(conversationId, savedDraft));
	const ref = useRef<HTMLTextAreaElement>(null);

	const caretToEnd = useRef(false);

	// Switching chats swaps the draft, and a saved one shows once the conversation has loaded.
	useEffect(() => {
		const next = getDraft(conversationId, savedDraft);
		// New text arrives on the next render, so the caret moves then.
		if (ref.current && ref.current.value !== next) caretToEnd.current = true;
		setText(next);
	}, [conversationId, savedDraft]);
	// biome-ignore lint/correctness/useExhaustiveDependencies: runs after each text change
	useEffect(() => {
		if (!caretToEnd.current) return;
		caretToEnd.current = false;
		toEnd(ref.current);
	}, [text]);
	useEffect(() => {
		if (!conversationId) return;
		ref.current?.focus();
		toEnd(ref.current);
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
		setDraft(conversationId, v);
	};

	const submit = async () => {
		const value = text.trim();
		if (!value || busy) return;
		update("");
		// The sidebar marks chats with unsent text from the saved copy, so clear that one now.
		flushDraft(conversationId).then(() =>
			qc.invalidateQueries({ queryKey: keys.conversations }),
		);
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
				<Tip label="Stop">
					<RacButton
						aria-label="Stop"
						isDisabled={!canStop}
						onPress={onStop}
						className={cx(
							"flex size-8 shrink-0 items-center justify-center rounded-full bg-foreground text-background data-[disabled]:opacity-40",
							focus,
						)}
					>
						<StopIcon weight="fill" />
					</RacButton>
				</Tip>
			) : (
				<Tip label="Send">
					<RacButton
						type="submit"
						aria-label="Send"
						isDisabled={!text.trim()}
						className={cx(
							"flex size-8 shrink-0 items-center justify-center rounded-full bg-primary text-primary-foreground data-[disabled]:opacity-30",
							focus,
						)}
					>
						<ArrowUpIcon />
					</RacButton>
				</Tip>
			)}
		</form>
	);
}
