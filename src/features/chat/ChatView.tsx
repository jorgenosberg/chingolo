import { useQueryClient } from "@tanstack/react-query";
import { useLayoutEffect, useRef, useState } from "react";
import { useNav } from "../../app/nav";
import { useDialogs } from "../../components/dialogs";
import { SidebarRightIcon } from "../../components/icons";
import { ModelSelect } from "../../components/ModelSelect";
import { Button, IconButton, Modal_ } from "../../components/ui";
import { api } from "../../lib/api";
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
import { MessageItem } from "./MessageItem";

export function ChatView({ conversationId }: { conversationId: string }) {
	const qc = useQueryClient();
	const { openSettings, toggleContext } = useNav();
	const { prompt } = useDialogs();
	const conversations = useConversations();
	const conversation = conversations.data?.find((c) => c.id === conversationId);
	const messages = useMessages(conversationId);
	const live = useLiveReply(conversationId);
	const settings = useSettings();
	const models = useModels(!!settings.data?.hasApiKey);

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

	return (
		<div className="flex h-full min-h-0 flex-col">
			<header
				data-tauri-drag-region
				className="flex h-12 shrink-0 items-center justify-between gap-4 border-b border-border px-5"
			>
				<button
					type="button"
					onDoubleClick={rename}
					title="Double-click to rename"
					className="min-w-0 truncate rounded-md text-left text-sm font-medium outline-none focus-visible:ring-2 focus-visible:ring-primary/60"
				>
					{conversation?.title ?? ""}
				</button>
				<ModelSelect
					label="Model"
					models={models.data ?? []}
					value={modelId}
					onChange={changeModel}
					isDisabled={!hasKey || busy}
					placeholder={hasKey ? "Choose a model" : "No API key"}
					className="w-56 shrink-0"
				/>
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
					{list.map((m) => (
						<MessageItem
							key={m.id}
							message={m}
							liveText={live && live.messageId === m.id ? live.text : undefined}
						/>
					))}
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
