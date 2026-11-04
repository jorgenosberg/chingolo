import { math } from "@streamdown/math";
import { memo, useState } from "react";
import { Streamdown } from "streamdown";
import { CheckIcon, CopyIcon } from "../../components/icons";
import { IconButton } from "../../components/ui";
import type { Message } from "../../lib/bindings";

const plugins = { math };

export const MessageItem = memo(function MessageItem({
	message,
	liveText,
}: {
	message: Message;
	/** Text received so far while this reply is streaming; overrides the saved text. */
	liveText?: string;
}) {
	if (message.role === "user") {
		return (
			<div className="flex justify-end">
				<div className="selectable max-w-[85%] whitespace-pre-wrap rounded-2xl bg-accent-soft px-4 py-2.5 text-[15px] leading-relaxed">
					{message.content}
				</div>
			</div>
		);
	}

	const streaming = liveText !== undefined;
	const text = streaming ? liveText : message.content;
	const note = describeNote(message);

	return (
		<div className="group flex flex-col gap-1.5">
			{text ? (
				<div className="selectable chat-prose">
					<Streamdown plugins={plugins} isAnimating={streaming}>
						{text}
					</Streamdown>
				</div>
			) : streaming ? (
				<p className="text-sm text-muted-foreground" aria-live="polite">
					Thinking…
				</p>
			) : null}

			{note && (
				<p
					role={message.status === "error" ? "alert" : undefined}
					className={
						message.status === "error"
							? "text-[13px] text-problem"
							: "text-[13px] text-muted-foreground"
					}
				>
					{note}
				</p>
			)}

			{!streaming && text && (
				<div className="flex items-center gap-2 text-xs text-muted-foreground opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
					<CopyButton text={text} />
					{message.modelId && (
						<span className="selectable">{message.modelId}</span>
					)}
				</div>
			)}
		</div>
	);
});

function describeNote(m: Message): string | null {
	switch (m.status) {
		case "error":
			return m.error ?? "Something went wrong.";
		case "cancelled":
			return "Stopped.";
		case "interrupted":
			return m.error ?? "This reply was interrupted.";
		case "complete":
			return m.error;
		default:
			return null;
	}
}

function CopyButton({ text }: { text: string }) {
	const [done, setDone] = useState(false);
	return (
		<IconButton
			label={done ? "Copied" : "Copy reply"}
			onPress={async () => {
				try {
					await navigator.clipboard.writeText(text);
					setDone(true);
					setTimeout(() => setDone(false), 1500);
				} catch {
					// clipboard can be unavailable; nothing useful to show
				}
			}}
		>
			{done ? <CheckIcon /> : <CopyIcon />}
		</IconButton>
	);
}
