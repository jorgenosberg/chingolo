import { Channel } from "@tauri-apps/api/core";
import {
	type ChatEvent,
	type ConversationFilter,
	commands,
	type MetadataPatch,
	type Scope,
	type SettingsPatch,
	type UsagePeriod,
} from "./bindings";

type Result<T> = { status: "ok"; data: T } | { status: "error"; error: string };

/** Commands report errors as plain strings. Turn them into thrown Errors for react-query. */
async function unwrap<T>(p: Promise<Result<T>>): Promise<T> {
	const r = await p;
	if (r.status === "error") throw new Error(r.error);
	return r.data;
}

export const api = {
	listProjects: () => unwrap(commands.listProjects()),
	createProject: (name: string) => unwrap(commands.createProject(name)),
	renameProject: (id: string, name: string) =>
		unwrap(commands.renameProject(id, name)),
	deleteProject: (id: string) => unwrap(commands.deleteProject(id)),
	listChapters: (projectId: string) => unwrap(commands.listChapters(projectId)),
	createChapter: (projectId: string, title: string) =>
		unwrap(commands.createChapter(projectId, title)),
	renameChapter: (id: string, title: string) =>
		unwrap(commands.renameChapter(id, title)),
	deleteChapter: (id: string) => unwrap(commands.deleteChapter(id)),

	listConversations: (filter: Partial<ConversationFilter>) =>
		unwrap(
			commands.listConversations({
				projectId: null,
				query: null,
				...filter,
			}),
		),
	searchMessages: (query: string) => unwrap(commands.searchMessages(query)),
	createConversation: (projectId: string | null, chapterId: string | null) =>
		unwrap(commands.createConversation(projectId, chapterId)),
	renameConversation: (id: string, title: string) =>
		unwrap(commands.renameConversation(id, title)),
	archiveConversation: (id: string, archived: boolean) =>
		unwrap(commands.archiveConversation(id, archived)),
	setConversationModel: (id: string, provider: string, modelId: string) =>
		unwrap(commands.setConversationModel(id, provider, modelId)),
	setConversationScope: (id: string, scope: Scope | null) =>
		unwrap(commands.setConversationScope(id, scope)),
	deleteConversation: (id: string) => unwrap(commands.deleteConversation(id)),
	forkConversation: (id: string, fromMessageId: string | null) =>
		unwrap(commands.forkConversation(id, fromMessageId)),
	summarizeForHandoff: (id: string) => unwrap(commands.summarizeForHandoff(id)),
	listMessages: (conversationId: string) =>
		unwrap(commands.listMessages(conversationId)),
	cancelGeneration: (messageId: string) => commands.cancelGeneration(messageId),

	getSettings: () => unwrap(commands.getSettings()),
	updateSettings: (patch: Partial<SettingsPatch>) =>
		unwrap(
			commands.updateSettings({
				onboarded: null,
				privacyAcknowledged: null,
				defaultModel: null,
				defaultProvider: null,
				embeddingNoticeDismissed: null,
				rerankEnabled: null,
				rerankModel: null,
				rerankProvider: null,
				crossrefEnabled: null,
				titleSearchEnabled: null,
				metadataAiEnabled: null,
				metadataModel: null,
				metadataProvider: null,
				favoriteModels: null,
				...patch,
			}),
		),
	saveApiKey: (provider: string, key: string) =>
		unwrap(commands.saveApiKey(provider, key)),
	deleteApiKey: (provider: string) => unwrap(commands.deleteApiKey(provider)),
	listAllModels: () => unwrap(commands.listAllModels()),

	pickAndImport: (folder: boolean, projectId: string | null) =>
		unwrap(commands.pickAndImport(folder, projectId)),
	setImportTarget: (projectId: string | null) =>
		unwrap(commands.setImportTarget(projectId)),
	listDocuments: (projectId: string | null) =>
		unwrap(commands.listDocuments(projectId)),
	updateDocumentMetadata: (id: string, patch: Partial<MetadataPatch>) =>
		unwrap(
			commands.updateDocumentMetadata(id, {
				title: null,
				authors: null,
				year: null,
				doi: null,
				journal: null,
				publisher: null,
				volume: null,
				issue: null,
				pages: null,
				series: null,
				isbn: null,
				issn: null,
				...patch,
			}),
		),
	deleteDocument: (id: string) => unwrap(commands.deleteDocument(id)),
	cancelImport: (id: string) => commands.cancelImport(id),
	retryImport: (id: string) => unwrap(commands.retryImport(id)),
	refreshDetails: (id: string) => unwrap(commands.refreshDetails(id)),
	refreshAllDetails: () => unwrap(commands.refreshAllDetails()),
	attachDocument: (projectId: string, documentId: string) =>
		unwrap(commands.attachDocument(projectId, documentId)),
	detachDocument: (projectId: string, documentId: string) =>
		unwrap(commands.detachDocument(projectId, documentId)),
	getPassage: (passageId: string) => unwrap(commands.getPassage(passageId)),
	getLayout: (documentId: string) => unwrap(commands.getLayout(documentId)),
	getSource: (conversationId: string, handle: number) =>
		unwrap(commands.getSource(conversationId, handle)),
	searchLibrary: (query: string, projectId: string | null, meaning: boolean) =>
		unwrap(commands.searchLibrary(query, projectId, 30, meaning)),

	checkForUpdate: () => unwrap(commands.checkForUpdate()),
	installUpdate: () => unwrap(commands.installUpdate()),

	ocrStatus: () => commands.ocrStatus(),
	downloadOcrModel: () => commands.downloadOcrModel(),
	layoutStatus: () => commands.layoutStatus(),
	layoutProgress: () => unwrap(commands.layoutProgress()),
	downloadLayoutModel: () => commands.downloadLayoutModel(),
	tablesStatus: () => commands.tablesStatus(),
	downloadTablesModel: () => commands.downloadTablesModel(),
	embeddingStatus: () => commands.embeddingStatus(),
	downloadEmbeddingModel: () => unwrap(commands.downloadEmbeddingModel()),
	allowCloudEmbedding: () => unwrap(commands.allowCloudEmbedding()),
	setEmbeddingBackend: (backend: "gemini" | "local") =>
		unwrap(commands.setEmbeddingBackend(backend)),
	setEmbeddingPaused: (paused: boolean) => commands.setEmbeddingPaused(paused),

	getUsage: (period: UsagePeriod) => unwrap(commands.getUsage(period)),
	getConversationCost: (id: string) => unwrap(commands.getConversationCost(id)),
	showLogs: () => unwrap(commands.showLogs()),
	setPricesAutoUpdate: (enabled: boolean) =>
		unwrap(commands.setPricesAutoUpdate(enabled)),
	updatePricesNow: () => unwrap(commands.updatePricesNow()),
};

export function sendMessageRaw(
	conversationId: string,
	content: string,
	channel: Channel<ChatEvent>,
) {
	return unwrap(commands.sendMessage(conversationId, content, channel));
}

export { Channel };
