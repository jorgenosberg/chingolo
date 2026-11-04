import { useQueryClient } from "@tanstack/react-query";
import { Fragment, useCallback, useLayoutEffect, useRef, useState } from "react";
import { useNav } from "../../app/nav";
import { ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import { SidebarRightIcon } from "../../components/icons";
import { ModelPopover } from "../../components/ModelPopover";
import { Button, IconButton, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
import type { Message } from "../../lib/bindings";
import {
	keys,
	sendMessage,
	useConversations,
	useMessages,
	useModels,
	useSettings,
} from "../../lib/queries";
import { useLiveReply } from "../../lib/streams";
import { PrivacyNotice } from "../settings/PrivacyNotice";
import { Composer } from "./Composer";
import { useConversationFlows } from "./flows";
import { MessageItem } from "./MessageItem";

export function ChatView({ conversationId }: { conversationId: string }) {
	const qc = useQueryClient();
	const { openSettings, toggleContext } = useNav();
	const { prompt, confirm } = useDialogs();
	const { fork, handoff } = useConversationFlows();
	const conversations = useConversations();
	const conversation = conversations.data?.find((c) => c.id === conversationId);
	const messages = useMessages(conversationId);
	const live = useLiveReply(conversationId);
	const settings = useSettings();
	const models = useModels(!!settings.data?.hasApiKey);

	// Stable callback so finished messages don't re-render while a reply streams.
	const forkRef = useRef<(messageId: string) => void>(() => {});
	forkRef.current = (messageId) => {
		if (conversation) fork(conversation, messageId);
	};
	const onFork = useCallback(
		(messageId: string) => forkRef.current(messageId),
		[],
	);

	const [sending, setSending] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [askPrivacy, setAskPrivacy] = useState(false);

	const scroller = useRef<HTMLDivElement>(null);
	const stick = useRef(true);

	// Follow new text unless the reader scrolled up.
	// biome-ignore lint/correctness/useExhaustiveDependencies: run when content changes
	useLayoutEffect(() => {
		const el = scroller.current;
		if (el && stick.current) el.scrollTop = el.scrollHeight;
	}, [messages.data, live?.text]);

	// biome-ignore lint/correctness/useExhaustiveDependencies: reset when the chat changes
	useLayoutEffect(() => {
		stick.current = true;
		setError(null);
	}, [conversationId]);

	if (conversations.isSuccess && !conversation) {
		return (
			<p className="p-8 text-sm text-muted-foreground">
				This conversation no longer exists.
			</p>
		);
	}

	const hasKey = settings.data?.hasApiKey ?? false;
	const busy = sending || live !== undefined;
	const switchCost = conversation?.switchCostTokens ?? null;
	const modelId = conversation?.modelId ?? settings.data?.defaultModel ?? null;

	const doSend = async (text: string): Promise<boolean> => {
		setError(null);
		setSending(true);
		try {
			await sendMessage(qc, conversationId, text);
			stick.current = true;
			return true;
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
			return false;
		} finally {
			setSending(false);
		}
	};

	const onSend = async (text: string): Promise<boolean> => {
		if (!hasKey) {
			openSettings();
			return false;
		}
		if (!settings.data?.privacyAcknowledged) {
			// The text goes back into the composer; the user sends again once they've read it.
			setAskPrivacy(true);
			return false;
		}
		return doSend(text);
	};

	const acknowledgePrivacy = async () => {
		setAskPrivacy(false);
		await api.updateSettings({ privacyAcknowledged: true });
		await qc.invalidateQueries({ queryKey: keys.settings });
	};

	const changeModel = async (id: string) => {
		if (id === modelId) return;
		if (switchCost) {
			const name = models.data?.find((m) => m.id === id)?.displayName ?? id;
			const ok = await confirm({
				title: "Switch model?",
				message: `${name} has to read this whole conversation (about ${formatTokens(switchCost)} tokens) as new input, with no cached input from the current model. Cached input usually costs about a tenth as much, so the next message costs more. Messages after that are cached again. To continue with less context, use "Hand off to a new conversation" from the ⋯ menu instead.`,
				confirmLabel: "Switch model",
			});
			if (!ok) return;
		}
		await api.setConversationModel(conversationId, id);
		await qc.invalidateQueries({ queryKey: keys.conversations });
	};

	const rename = async () => {
		if (!conversation) return;
		const title = await prompt({
			title: "Rename conversation",
			label: "Title",
			initial: conversation.title,
		});
		if (title) {
			await api.renameConversation(conversationId, title);
			await qc.invalidateQueries({ queryKey: keys.conversations });
		}
	};

	const list = messages.data ?? [];
	const modelName = (id: string) =>
		models.data?.find((m) => m.id === id)?.displayName ?? id;
	const modelChanges = findModelChanges(list);
	// The model the conversation began with: its first reply's model, else the current choice.
	const startModel =
		list.find((m) => m.role === "assistant" && m.modelId)?.modelId ?? modelId;

	return (
		<div className="flex h-full min-h-0 flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-border px-5"
			>
					<div className="flex min-w-0 items-center gap-1">
						<button
							type="button"
							onDoubleClick={rename}
							title="Double-click to rename"
							className="min-w-0 truncate rounded-md text-left text-sm font-medium outline-none focus-visible:ring-2 focus-visible:ring-primary/60"
						>
							{conversation?.title ?? ""}
						</button>
						{conversation && (
							<ActionMenu
								label="Conversation actions"
								actions={[
									{ id: "rename", label: "Rename…", run: rename },
									...(list.length > 0
										? [
												{
													id: "fork",
													label: "Fork conversation",
													run: () => fork(conversation),
												},
												{
													id: "handoff",
													label: "Hand off to a new conversation",
													run: () => handoff(conversation),
												},
											]
										: []),
								]}
							/>
						)}
					</div>
					<IconButton label="Toggle context pane" onPress={toggleContext}>
						<SidebarRightIcon />
					</IconButton>
			</header>

			<div
				ref={scroller}
				onScroll={(e) => {
					const el = e.currentTarget;
					stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
				}}
				className="min-h-0 flex-1 overflow-y-auto"
			>
				<div className="mx-auto flex max-w-3xl flex-col gap-6 px-6 py-8">
					{!hasKey && settings.isSuccess && (
						<div className="flex items-center justify-between gap-4 rounded-xl bg-muted px-4 py-3 text-sm">
							<span>Add your Gemini API key to start chatting.</span>
							<Button onPress={openSettings}>Open settings</Button>
						</div>
					)}
					{conversation && startModel && (
						<p className="text-center text-xs text-muted-foreground">
							Started {formatDay(conversation.createdAt)} with{" "}
							{modelName(startModel)}
						</p>
					)}
					{messages.isError && (
						<p role="alert" className="text-sm text-problem">
							Couldn't load messages: {messages.error.message}
						</p>
					)}
					{messages.isSuccess && list.length === 0 && (
						<p className="py-16 text-center text-muted-foreground">
							Ask a question to get started.
						</p>
					)}
					{list.map((m) => {
						const change = modelChanges.get(m.id);
						return (
							<Fragment key={m.id}>
								{change && (
									<div className="flex items-center gap-3 text-xs text-muted-foreground">
										<span className="h-px flex-1 bg-border" />
										<span>Switched to {modelName(change)}</span>
										<span className="h-px flex-1 bg-border" />
									</div>
								)}
								<MessageItem
									message={m}
									onFork={onFork}
									liveText={
										live && live.messageId === m.id ? live.text : undefined
									}
								/>
							</Fragment>
						);
					})}
				</div>
			</div>

			<div className="shrink-0 px-6 pb-5 pt-2">
				{error && (
					<p
						role="alert"
						className="mx-auto mb-2 max-w-3xl text-sm text-problem"
					>
						{error}
					</p>
				)}
				<Composer
					conversationId={conversationId}
					busy={busy}
					canStop={!!live?.messageId}
					onSend={onSend}
					onStop={() => live?.messageId && api.cancelGeneration(live.messageId)}
					extra={
						<ModelPopover
							models={models.data ?? []}
							value={modelId}
							onChange={changeModel}
							isDisabled={!hasKey || busy}
						/>
					}
				/>
			</div>

			<Modal_
				isOpen={askPrivacy}
				onOpenChange={setAskPrivacy}
				title="Before your first message"
			>
				<PrivacyNotice onAcknowledge={acknowledgePrivacy} />
			</Modal_>
		</div>
	);
}

function formatTokens(n: number) {
	return n >= 1000 ? `${Math.round(n / 100) / 10}k` : String(n);
}

/**
 * Where the model changed: maps the id of the message to draw a divider before, to the new model.
 * The divider goes before the user message that led to the first reply from the new model.
 */
function findModelChanges(list: Message[]): Map<string, string> {
	const out = new Map<string, string>();
	let previous: string | null = null;
	list.forEach((m, i) => {
		if (m.role !== "assistant" || !m.modelId) return;
		if (previous && m.modelId !== previous) {
			const before = list[i - 1];
			out.set(before?.role === "user" ? before.id : m.id, m.modelId);
		}
		previous = m.modelId;
	});
	return out;
}

function formatDay(ms: number) {
	return new Date(ms).toLocaleDateString(undefined, {
		month: "short",
		day: "numeric",
	});
}
