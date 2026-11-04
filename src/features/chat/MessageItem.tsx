import { math } from "@streamdown/math";
import { memo, useState } from "react";
import { Streamdown } from "streamdown";
import { CheckIcon, CopyIcon, ForkIcon } from "../../components/icons";
import { IconButton } from "../../components/ui";
import type { Message } from "../../lib/bindings";

const plugins = { math };

export const MessageItem = memo(function MessageItem({
	message,
	liveText,
	onFork,
}: {
	message: Message;
	/** Copies the conversation up to and including this message into a new one. */
	onFork?: (messageId: string) => void;
	/** Text received so far while this reply is streaming; overrides the saved text. */
	liveText?: string;
}) {
	if (message.role === "user") {
		return (
			<div className="group flex flex-col items-end gap-1">
				<div className="selectable max-w-[85%] whitespace-pre-wrap rounded-2xl bg-accent-soft px-4 py-2.5 text-[15px] leading-relaxed">
					{message.content}
				</div>
				{/* Shown on hover or keyboard focus; the space is always reserved. */}
				<div className="flex items-center gap-1 text-muted-foreground opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
					<SentTime ms={message.createdAt} />
					<CopyButton text={message.content} label="Copy message" />
					{onFork && (
						<IconButton
							label="Fork conversation from here"
							onPress={() => onFork(message.id)}
						>
							<ForkIcon />
						</IconButton>
					)}
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
				<div className="flex items-center gap-2 text-xs text-muted-foreground">
					<CopyButton text={text} />
					{onFork && message.status === "complete" && (
						<IconButton
							label="Fork conversation from here"
							onPress={() => onFork(message.id)}
						>
							<ForkIcon />
						</IconButton>
					)}
					<span className="opacity-0 transition-opacity group-hover:opacity-100">
						<SentTime ms={message.createdAt} />
					</span>
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

function CopyButton({
	text,
	label = "Copy reply",
}: {
	text: string;
	label?: string;
}) {
	const [done, setDone] = useState(false);
	return (
		<IconButton
			label={done ? "Copied" : label}
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

/** "16:20" for today, "Nov 3, 16:20" for earlier days; the full date is in the tooltip. */
function SentTime({ ms }: { ms: number }) {
	const d = new Date(ms);
	const time = d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
	const today = d.toDateString() === new Date().toDateString();
	const label = today
		? time
		: `${d.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${time}`;
	return (
		<time
			dateTime={d.toISOString()}
			title={d.toLocaleString()}
			className="px-1 text-xs tabular-nums"
		>
			{label}
		</time>
	);
}
