import {
	BookmarkSimpleIcon,
	CheckIcon,
	CopyIcon,
	GitForkIcon,
} from "@phosphor-icons/react";
import { math } from "@streamdown/math";
import { useQueryClient } from "@tanstack/react-query";
import { memo, type RefObject, useRef, useState } from "react";
import { Streamdown } from "streamdown";
import { useDialogs } from "../../components/dialogs";
import { SelectionPill } from "../../components/SelectionPill";
import { IconButton } from "../../components/ui";
import { api } from "../../lib/api";
import type { Citation, Message } from "../../lib/bindings";
import { keys, useApiMutation, useSaved } from "../../lib/queries";
import {
	CiteProvider,
	citeComponents,
	citePlugins,
	citeTags,
	SourcesList,
} from "./citations";
import { cleanCopy, writeRich } from "./copy";

const plugins = { math };
const NO_CITATIONS: Citation[] = [];

export const MessageItem = memo(function MessageItem({
	message,
	liveText,
	liveStatus,
	onFork,
}: {
	message: Message;
	/** Copies the conversation up to and including this message into a new one. */
	onFork?: (messageId: string) => void;
	/** Text received so far while this reply is streaming; overrides the saved text. */
	liveText?: string;
	/** What the model is doing while it streams, such as a library search. */
	liveStatus?: string | null;
}) {
	const proseRef = useRef<HTMLDivElement>(null);
	if (message.role === "user") {
		return (
			<div
				data-message-id={message.id}
				className="group flex flex-col items-end gap-1"
			>
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
							<GitForkIcon />
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
		<div data-message-id={message.id} className="group flex flex-col gap-1.5">
			{text ? (
				<div
					ref={proseRef}
					className="selectable chat-prose"
					onCopy={(e) => {
						const sel = window.getSelection();
						if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
						const range = sel.getRangeAt(0);
						if (!proseRef.current?.contains(range.commonAncestorContainer))
							return;
						const { html, text } = cleanCopy(range.cloneContents());
						e.clipboardData.setData("text/html", html);
						e.clipboardData.setData("text/plain", text);
						e.preventDefault();
					}}
				>
					<CiteProvider
						citations={message.citations ?? NO_CITATIONS}
						conversationId={message.conversationId}
					>
						<Streamdown
							plugins={plugins}
							remarkPlugins={citePlugins}
							components={citeComponents}
							allowedTags={citeTags}
							isAnimating={streaming}
						>
							{text}
						</Streamdown>
					</CiteProvider>
				</div>
			) : streaming ? (
				<p className="text-sm text-muted-foreground" aria-live="polite">
					{liveStatus ?? "Thinking…"}
				</p>
			) : null}
			{!streaming && text && message.status === "complete" && (
				<SaveSelection messageId={message.id} proseRef={proseRef} />
			)}
			{text && streaming && liveStatus && (
				<p className="text-sm text-muted-foreground" aria-live="polite">
					{liveStatus}
				</p>
			)}

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

			{!streaming && text && (message.citations?.length ?? 0) > 0 && (
				<SourcesList
					citations={message.citations ?? NO_CITATIONS}
					conversationId={message.conversationId}
				/>
			)}

			{!streaming && text && (
				<div className="flex items-center gap-2 text-xs text-muted-foreground">
					<CopyButton
						text={text}
						rich={() => proseRef.current && cleanCopy(proseRef.current)}
					/>
					{message.status === "complete" && (
						<BookmarkButton messageId={message.id} />
					)}
					{onFork && message.status === "complete" && (
						<IconButton
							label="Fork conversation from here"
							onPress={() => onFork(message.id)}
						>
							<GitForkIcon />
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

/** The Save pill for text selected in a reply. Remembers which sources the selection cites. */
function SaveSelection({
	messageId,
	proseRef,
}: {
	messageId: string;
	proseRef: RefObject<HTMLDivElement | null>;
}) {
	const qc = useQueryClient();
	return (
		<SelectionPill
			containerRef={proseRef}
			onSave={async (range) => {
				const fragment = range.cloneContents();
				const handles = [...fragment.querySelectorAll("[data-cite]")]
					.map((c) => /^S(\d+)$/.exec(c.textContent?.trim() ?? "")?.[1])
					.filter((h): h is string => h !== undefined)
					.map(Number);
				await api.saveSelection(messageId, cleanCopy(fragment).text, handles);
				qc.invalidateQueries({ queryKey: keys.saved });
			}}
		/>
	);
}

/** Saves the reply to its project's Saved page, or removes it again. */
function BookmarkButton({ messageId }: { messageId: string }) {
	const saved = useSaved({ kind: "all" });
	const item = saved.data?.find(
		(i) => i.kind === "reply" && i.messageId === messageId,
	);
	const { confirm } = useDialogs();
	const toggle = useApiMutation(
		() =>
			item
				? api.deleteSavedItems([item.id]).then(() => null)
				: api.saveReply(messageId),
		[keys.saved],
	);
	return (
		<IconButton
			label={item ? "Remove from Saved" : "Save reply"}
			isDisabled={!saved.isSuccess || toggle.isPending}
			onPress={async () => {
				// Removing it also drops the notes written on the Saved page.
				if (
					item &&
					item.notes.length > 0 &&
					!(await confirm({
						title: "Remove from Saved?",
						message:
							item.notes.length === 1
								? "The note you wrote on this item is removed with it."
								: `The ${item.notes.length} notes you wrote on this item are removed with it.`,
						confirmLabel: "Remove",
						danger: true,
					}))
				)
					return;
				toggle.mutate(undefined);
			}}
		>
			<BookmarkSimpleIcon weight={item ? "fill" : "regular"} />
		</IconButton>
	);
}

function CopyButton({
	text,
	label = "Copy reply",
	rich,
}: {
	text: string;
	label?: string;
	/** Clean HTML and text of the rendered reply; without it the raw text is copied. */
	rich?: () => { html: string; text: string } | null;
}) {
	const [done, setDone] = useState(false);
	return (
		<IconButton
			label={done ? "Copied" : label}
			onPress={async () => {
				try {
					const copy = rich?.();
					if (copy) await writeRich(copy);
					else await navigator.clipboard.writeText(text);
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
	const time = d.toLocaleTimeString(undefined, {
		hour: "2-digit",
		minute: "2-digit",
	});
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
