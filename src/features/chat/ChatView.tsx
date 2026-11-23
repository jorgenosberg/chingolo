import {
	ArrowElbowDownRightIcon,
	GitForkIcon,
	PencilSimpleIcon,
	SidebarSimpleIcon,
} from "@phosphor-icons/react";
import { useQueryClient } from "@tanstack/react-query";
import {
	Fragment,
	useCallback,
	useEffect,
	useLayoutEffect,
	useRef,
	useState,
} from "react";
import { useNav } from "../../app/nav";
import { ActionMenu } from "../../components/ActionMenu";
import { useDialogs } from "../../components/dialogs";
import { ModelPopover } from "../../components/ModelPopover";
import { ScopePopover } from "../../components/ScopePopover";
import { Button, IconButton, Modal_, SHORTCUTS } from "../../components/ui";
import { api } from "../../lib/api";
import type { Message, Scope } from "../../lib/bindings";
import { type ModelRef, providerName, sameModel } from "../../lib/providers";
import {
	keys,
	sendMessage,
	useConversations,
	useDocuments,
	useMessages,
	useModels,
	useSettings,
} from "../../lib/queries";
import { useLiveReply } from "../../lib/streams";
import { PrivacyNotice } from "../settings/PrivacyNotice";
import { Composer } from "./Composer";
import { ContextMeter } from "./ContextMeter";
import { ConversationCost } from "./ConversationCost";
import { FindBar } from "./FindBar";
import { useConversationFlows } from "./flows";
import { MessageItem } from "./MessageItem";
import { PriceJumpWarning } from "./PriceJumpWarning";

export function ChatView({ conversationId }: { conversationId: string }) {
	const qc = useQueryClient();
	const {
		openSettings,
		toggleContext,
		contextOpen,
		messageTarget,
		clearMessageTarget,
	} = useNav();
	const { prompt, confirm } = useDialogs();
	const { fork, handoff } = useConversationFlows();
	const conversations = useConversations();
	const conversation = conversations.data?.find((c) => c.id === conversationId);
	const messages = useMessages(conversationId);
	const live = useLiveReply(conversationId);
	const settings = useSettings();
	const documents = useDocuments(null);
	const models = useModels((settings.data?.apiKeys.length ?? 0) > 0);
	const modelList = models.data?.models ?? [];

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
	// The conversation whose copied messages are expanded. Keyed by id so switching chats
	// collapses them without an effect.
	const [earlierFor, setEarlierFor] = useState<string | null>(null);
	const showEarlier = earlierFor === conversationId;
	const [findOpen, setFindOpen] = useState(false);
	const [findTick, setFindTick] = useState(0);

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
		setFindOpen(false);
	}, [conversationId]);

	// ⌘F. This view is only mounted for chats, so the reader keeps its own find.
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (!e.metaKey || e.altKey || e.ctrlKey || e.key !== SHORTCUTS.find)
				return;
			e.preventDefault();
			setFindOpen(true);
			setFindTick((t) => t + 1);
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, []);

	// Jump to a message a search hit asked for. Declared after the effects above so it has the
	// last word on the scroll position when both run in one commit.
	const targetId =
		messageTarget?.conversationId === conversationId
			? messageTarget.messageId
			: null;
	const targetNonce = messageTarget?.nonce;
	const forkBoundaryId = conversation?.forkBoundaryMessageId;
	// biome-ignore lint/correctness/useExhaustiveDependencies: the nonce re-runs a repeat jump
	useLayoutEffect(() => {
		const el = scroller.current;
		const list = messages.data;
		if (!targetId || !el || !list) return;
		const at = list.findIndex((m) => m.id === targetId);
		if (at < 0) {
			// Deleted since the search. Stay where the chat opens.
			clearMessageTarget();
			return;
		}
		const boundary = forkBoundaryId
			? list.findIndex((m) => m.id === forkBoundaryId)
			: -1;
		if (at <= boundary && !showEarlier) {
			// Behind the collapsed fork row. Expand it; this effect runs again.
			setEarlierFor(conversationId);
			return;
		}
		const node = el.querySelector(
			`[data-message-id="${CSS.escape(targetId)}"]`,
		);
		if (!node) return;
		stick.current = false;
		node.scrollIntoView({ block: "center" });
		const mark = node.querySelector(".selectable") ?? node;
		mark.classList.remove("message-flash");
		void (mark as HTMLElement).offsetWidth; // restart the animation on a repeat jump
		mark.classList.add("message-flash");
		mark.addEventListener(
			"animationend",
			() => mark.classList.remove("message-flash"),
			{ once: true },
		);
		clearMessageTarget();
	}, [
		targetId,
		targetNonce,
		messages.data,
		forkBoundaryId,
		showEarlier,
		conversationId,
		clearMessageTarget,
	]);

	if (conversations.isSuccess && !conversation) {
		return (
			<p className="p-8 text-sm text-muted-foreground">
				This conversation no longer exists.
			</p>
		);
	}

	const hasKey = conversation
		? (settings.data?.apiKeys.includes(conversation.provider) ?? false)
		: false;
	const busy = sending || live !== undefined;
	const switchCost = conversation?.switchCostTokens ?? null;
	const modelId = conversation?.modelId ?? settings.data?.defaultModel ?? null;
	const modelRef: ModelRef | null =
		conversation && modelId
			? { provider: conversation.provider, id: modelId }
			: null;

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

	const changeModel = async (next: ModelRef) => {
		if (sameModel(next, modelRef)) return;
		if (switchCost) {
			const name =
				modelList.find((m) => sameModel(m, next))?.displayName ?? next.id;
			const ok = await confirm({
				title: "Switch model?",
				message: `${name} has to read this whole conversation (about ${formatTokens(switchCost)} tokens) as new input, with no cached input from the current model. Cached input usually costs about a tenth as much, so the next message costs more. Messages after that are cached again. To continue with less context, use "Hand off to a new conversation" from the ⋯ menu instead.`,
				confirmLabel: "Switch model",
			});
			if (!ok) return;
		}
		await api.setConversationModel(conversationId, next.provider, next.id);
		await qc.invalidateQueries({ queryKey: keys.conversations });
	};

	const changeScope = async (next: Scope | null) => {
		try {
			await api.setConversationScope(conversationId, next);
			setError(null);
		} catch (e) {
			setError(e instanceof Error ? e.message : String(e));
		}
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
		modelList.find((m) => m.id === id)?.displayName ?? id;
	const modelInfo = modelList.find((m) => modelRef && sameModel(m, modelRef));
	const lastCached =
		[...list]
			.reverse()
			.find(
				(m) => m.role === "assistant" && m.status !== "error" && m.inputTokens,
			)?.cachedInputTokens ?? null;
	const modelChanges = findModelChanges(list);
	// The model the conversation began with: its first reply's model, else the current choice.
	const startModel =
		list.find((m) => m.role === "assistant" && m.modelId)?.modelId ?? modelId;
	// A fork's copied messages sit behind a collapsed row. They are still sent to the model.
	const boundaryId = conversation?.forkBoundaryMessageId;
	const boundary = boundaryId ? list.findIndex((m) => m.id === boundaryId) : -1;
	const earlier = list.slice(0, boundary + 1);
	const current = list.slice(boundary + 1);

	const renderMessage = (m: Message) => {
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
					liveText={live && live.messageId === m.id ? live.text : undefined}
					liveStatus={live && live.messageId === m.id ? live.status : null}
				/>
			</Fragment>
		);
	};

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
								{
									id: "rename",
									icon: <PencilSimpleIcon className="size-4" />,
									label: "Rename…",
									run: rename,
								},
								...(list.length > 0
									? [
											{
												id: "fork",
												icon: <GitForkIcon className="size-4" />,
												label: "Fork conversation",
												run: () => fork(conversation),
											},
											{
												id: "handoff",
												icon: <ArrowElbowDownRightIcon className="size-4" />,
												label: "Hand off to a new conversation",
												run: () => handoff(conversation),
											},
										]
									: []),
							]}
						/>
					)}
				</div>
				<div className="flex shrink-0 items-center gap-1">
					<PriceJumpWarning
						used={switchCost}
						over={modelInfo?.facts?.priceJumpOver ?? null}
						factor={modelInfo?.facts?.priceJumpFactor ?? null}
						onHandoff={() => conversation && handoff(conversation)}
					/>
					<ContextMeter
						used={switchCost}
						limit={modelInfo?.inputTokenLimit ?? null}
						cached={lastCached}
					/>
					<ConversationCost conversationId={conversationId} />
					{/* While the pane is open it has its own close control, so only one shows. */}
					{!contextOpen && (
						<IconButton
							label="Show context pane"
							shortcut={SHORTCUTS.context}
							onPress={toggleContext}
						>
							<SidebarSimpleIcon mirrored />
						</IconButton>
					)}
				</div>
			</header>

			{findOpen && (
				<FindBar
					root={scroller}
					focusTick={findTick}
					onClose={() => setFindOpen(false)}
					expandEarlier={() => setEarlierFor(conversationId)}
				/>
			)}

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
							<span>
								Add your {providerName(conversation?.provider ?? "gemini")} API
								key to start chatting.
							</span>
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
					{earlier.length > 0 && (
						<>
							<button
								type="button"
								aria-expanded={showEarlier}
								onClick={() =>
									setEarlierFor((f) =>
										f === conversationId ? null : conversationId,
									)
								}
								className="mx-auto flex max-w-full items-center gap-2 rounded-md px-2 py-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-primary/60"
							>
								<span className="truncate">
									Forked from “{conversation?.forkedFromTitle}”
								</span>
								<span className="shrink-0">
									· {earlier.length} earlier message
									{earlier.length === 1 ? "" : "s"} ·{" "}
									{showEarlier ? "Hide" : "Show"}
								</span>
							</button>
							{showEarlier && earlier.map(renderMessage)}
							{showEarlier && <hr className="border-border" />}
						</>
					)}
					{messages.isSuccess && current.length === 0 && (
						<p className="py-16 text-center text-muted-foreground">
							Ask a question to get started.
						</p>
					)}
					{current.map(renderMessage)}
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
					savedDraft={conversation?.draft ?? ""}
					busy={busy}
					canStop={!!live?.messageId}
					onSend={onSend}
					onStop={() => live?.messageId && api.cancelGeneration(live.messageId)}
					extra={
						<>
							<ScopePopover
								projectId={conversation?.projectId ?? null}
								value={conversation?.sourceScope ?? null}
								documents={documents.data ?? []}
								onChange={changeScope}
								isDisabled={busy}
							/>
							<ModelPopover
								models={modelList}
								errors={models.data?.errors}
								value={modelRef}
								onChange={changeModel}
								isDisabled={!hasKey || busy}
							/>
						</>
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
