export type ObjectType = 'note' | 'task' | 'project' | (string & {});
export type {
	EncryptedOperation,
	SequencedOperation,
	SyncPage,
	SyncTransport,
} from './sync';
export type WorkspacePhase =
	| 'idle'
	| 'opening'
	| 'scanning'
	| 'indexing'
	| 'ready'
	| 'rebuilding'
	| 'failed';
export type EventSource = 'application' | 'external' | 'reconciliation' | 'mcp';
export type { AiCancelOutcome } from './generated/AiCancelOutcome';
export type { AiConsentDataCategory } from './generated/AiConsentDataCategory';
export type { AiConsentGrant } from './generated/AiConsentGrant';
export type { AiConsentGrantInput } from './generated/AiConsentGrantInput';
export type { AiConsentReadInput } from './generated/AiConsentReadInput';
export type { AiConsentRevokeInput } from './generated/AiConsentRevokeInput';
export type { AiConsentRevokeOutcome } from './generated/AiConsentRevokeOutcome';
export type { AiContentPart } from './generated/AiContentPart';
export type { AiModelRef } from './generated/AiModelRef';
export type { AiProviderConfig } from './generated/AiProviderConfig';
export type { AiStopReason } from './generated/AiStopReason';
export type { AiStreamEvent } from './generated/AiStreamEvent';
export type { AiStreamFrame } from './generated/AiStreamFrame';
export type { AiStreamInput } from './generated/AiStreamInput';
export type { AiStreamSummary } from './generated/AiStreamSummary';
export type { AiToolCall } from './generated/AiToolCall';
export type { AiToolDefinition } from './generated/AiToolDefinition';
export type { AiTransportMessage } from './generated/AiTransportMessage';
export type { AiUsage } from './generated/AiUsage';
export type { AppendChatContextSummaryInput } from './generated/AppendChatContextSummaryInput';
export type { AppendChatToolResultInput } from './generated/AppendChatToolResultInput';
export type { AppendChatUserMessageInput } from './generated/AppendChatUserMessageInput';
export type { BeginChatAssistantInput } from './generated/BeginChatAssistantInput';
export type { BeginChatToolCallInput } from './generated/BeginChatToolCallInput';
export type { ChangeChatRetentionInput } from './generated/ChangeChatRetentionInput';
export type { Chat } from './generated/Chat';
export type { ChatMessage } from './generated/ChatMessage';
export type { ChatMessageKind } from './generated/ChatMessageKind';
export type { ChatMessageStatus } from './generated/ChatMessageStatus';
export type { ChatRetention } from './generated/ChatRetention';
export type { ChatRead } from './generated/ChatRead';
export type { CreateChatInput } from './generated/CreateChatInput';
export type { RenameChatInput } from './generated/RenameChatInput';
export type { FinishChatAssistantInput } from './generated/FinishChatAssistantInput';
export type { FinishChatToolCallInput } from './generated/FinishChatToolCallInput';
export type { FolderEntry } from './generated/FolderEntry';
export type { DraftReconcileInput } from './generated/DraftReconcileInput';
export type { ResolveConflictInput } from './generated/ResolveConflictInput';
export type { ManagedDraftInput } from './generated/ManagedDraftInput';
export type { ManagedDraftResult } from './generated/ManagedDraftResult';
export type { ObjectActivation } from './generated/ObjectActivation';
export type { ManagedConflictResolution } from './generated/ManagedConflictResolution';
export type { ManagedConflictResolveInput } from './generated/ManagedConflictResolveInput';
export type { RawMarkdownRead } from './generated/RawMarkdownRead';
export type { RawReconcileInput } from './generated/RawReconcileInput';
export type { RawReconcileResult } from './generated/RawReconcileResult';
export type { RawSaveInput } from './generated/RawSaveInput';
export type { RawSaveResult } from './generated/RawSaveResult';
export type { RawConflictResolveInput } from './generated/RawConflictResolveInput';
export type { RawConflictResolveResult } from './generated/RawConflictResolveResult';
export type { MarkdownLinkTarget } from './generated/MarkdownLinkTarget';
export type { UnmanagedFile } from './generated/UnmanagedFile';
export type { WorkspaceEntry } from './generated/WorkspaceEntry';
export type { WorkspaceEntryKind } from './generated/WorkspaceEntryKind';

export interface WorkspaceManifest {
	id: string;
	formatVersion: number;
	name: string;
	created: string;
	updated: string;
	enabledPlugins: string[];
	ignore: string[];
}
export interface ManifestUpdateInput {
	name?: string | null;
	enabledPlugins?: string[] | null;
	ignore?: string[] | null;
	/** Reject the update unless the on-disk manifest still has this `updated`. */
	expectedUpdated?: string | null;
}

export interface WorkspaceObject {
	id: string;
	type: ObjectType;
	title: string;
	body: string;
	relativePath: string;
	revision: string;
	created: string | null;
	updated: string | null;
	properties: Record<string, unknown>;
}

export interface WorkspaceMetadata {
	id: string;
	name: string;
	rootPath: string;
	formatVersion: number;
}
export interface WorkspaceState {
	phase: WorkspacePhase;
	workspaceId?: string | null;
	rootPath?: string | null;
	indexedFiles: number;
	diagnostics: Diagnostic[];
}
export interface Diagnostic {
	code: string;
	message: string;
	relativePath?: string | null;
	objectId?: string | null;
}
export interface CoreWarning {
	code: string;
	message: string;
}
export interface MutationResult<T> {
	value: T;
	revision: string;
	durability: 'committed';
	indexStatus: 'updated' | 'repair-pending';
	warnings: CoreWarning[];
}
export interface CoreEvent<T = unknown> {
	eventId: string;
	type: string;
	workspaceId: string;
	occurredAt: string;
	source: EventSource;
	payload: T;
}
export interface CoreError {
	code: string;
	category:
		| 'validation'
		| 'filesystem'
		| 'permission'
		| 'parse'
		| 'identity'
		| 'index'
		| 'conflict'
		| 'credential'
		| 'provider'
		| 'transient';
	message: string;
	retryable: boolean;
	operation: string;
	workspaceId?: string | null;
	objectId?: string | null;
	path?: string | null;
	details?: Record<string, unknown> | null;
}
export function isCoreError(value: unknown): value is CoreError {
	if (!value || typeof value !== 'object') return false;
	const candidate = value as Record<string, unknown>;
	return (
		typeof candidate.code === 'string' &&
		typeof candidate.category === 'string' &&
		typeof candidate.message === 'string' &&
		typeof candidate.retryable === 'boolean' &&
		typeof candidate.operation === 'string'
	);
}

export interface ObjectQuery {
	type?: ObjectType;
	project?: string;
	status?: string;
	priority?: string;
	pathPrefix?: string;
}
export interface ObjectPatch {
	title?: string;
	body?: string;
	properties?: Record<string, unknown>;
	removeProperties?: string[];
	expectedRevision: string;
}
export type DraftReconcileResult =
	| { status: 'unchanged'; current: Note; body: string }
	| { status: 'merged'; current: Note; body: string }
	| { status: 'conflict'; current: Note };
export interface SearchInput {
	query: string;
	type?: ObjectType;
	pathPrefix?: string;
	limit?: number;
}
export interface SearchResult {
	objectId: string | null;
	objectType: ObjectType | null;
	relativePath: string;
	title: string;
	snippet: string;
	highlights: Array<{ start: number; end: number }>;
	score: number;
	revision: string;
}
export interface CalendarEntry {
	sourceId: string;
	sourceType: ObjectType;
	title: string;
	property: 'due' | 'date' | 'start';
	start: string;
	end: string | null;
	allDay: boolean;
	revision: string;
}

export type TaskStatus = 'todo' | 'in-progress' | 'done' | 'cancelled';
export type TaskPriority = 'low' | 'medium' | 'high' | 'urgent';
export type ProjectStatus =
	'planned' | 'active' | 'on-hold' | 'completed' | 'cancelled';
export type Task = WorkspaceObject & {
	type: 'task';
	properties: WorkspaceObject['properties'] & {
		status: TaskStatus;
		priority: TaskPriority;
		due?: string;
		project?: string;
		kanban_order?: string;
	};
};
export type Note = WorkspaceObject & { type: 'note' };
export type Project = WorkspaceObject & {
	type: 'project';
	properties: WorkspaceObject['properties'] & { status: ProjectStatus };
};
export type { SyncAccount } from './generated/SyncAccount';
export type { SyncAccountPoll } from './generated/SyncAccountPoll';
export type { DeviceSignInInfo } from './generated/DeviceSignInInfo';
export type { WorkspaceSyncStatus } from './generated/WorkspaceSyncStatus';
export type { WorkspaceSyncActivationStatus } from './generated/WorkspaceSyncActivationStatus';
export type { WorkspaceSyncPhase } from './generated/WorkspaceSyncPhase';
export type { WorkspaceSyncTransitionObject } from './generated/WorkspaceSyncTransitionObject';
export type { WorkspaceSyncTransitionPhase } from './generated/WorkspaceSyncTransitionPhase';
export type { WorkspaceSyncTransitionStatus } from './generated/WorkspaceSyncTransitionStatus';
export type { WorkspaceCapability } from './generated/WorkspaceCapability';
export type { CollaborationBootstrapRole } from './generated/CollaborationBootstrapRole';
export type { CollaborationConflictReview } from './generated/CollaborationConflictReview';
export type { CollaborationStatus } from './generated/CollaborationStatus';
export type { CollaborationStatusEvent } from './generated/CollaborationStatusEvent';
export type { CollaborationPresenceInput } from './generated/CollaborationPresenceInput';
export type { CollaborationPresenceMember } from './generated/CollaborationPresenceMember';
export type { CollaborationPresenceEvent } from './generated/CollaborationPresenceEvent';
export type { EncryptedPresence } from './generated/EncryptedPresence';
export type { SyncDevice } from './generated/SyncDevice';
export type { SyncInvitation } from './generated/SyncInvitation';
export type { SyncInvitationLink } from './generated/SyncInvitationLink';
export type { SyncInvitationRole } from './generated/SyncInvitationRole';
export type { SyncInvitationStatus } from './generated/SyncInvitationStatus';
export type { RemoteSyncWorkspace } from './generated/RemoteSyncWorkspace';
export type { SyncConflict } from './generated/SyncConflict';
export type { ResolveSyncConflict } from './generated/ResolveSyncConflict';
export type { SyncResolutionChoice } from './generated/SyncResolutionChoice';

export type { CollaborationOpenInput } from './generated/CollaborationOpenInput';
export type { CollaborationSession } from './generated/CollaborationSession';
export type { CollaborationSubmitInput } from './generated/CollaborationSubmitInput';
export type { CollaborationReceipt } from './generated/CollaborationReceipt';
