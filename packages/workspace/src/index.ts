import { readable, type Readable } from 'svelte/store';
import { nextKanbanOrder, projectKanban } from '@noura/plugin-tasks';
import {
	AiRegistry,
	type AiContributionRegistration,
	type AiContextProvider,
	type AiInstructionProvider,
	type AiToolDefinition,
} from '@noura/ai';
import type {
	PdfInfo,
	PdfRangeInput,
	CollaborationOpenInput,
	CollaborationPresenceInput,
	CollaborationSession,
	CollaborationSubmitInput,
	CollaborationReceipt,
	WorkspaceSyncStatus,
	SyncDevice,
	SyncInvitation,
	SyncInvitationLink,
	SyncInvitationRole,
	RemoteSyncWorkspace,
	SyncConflict,
	ResolveSyncConflict,
	SyncAccount,
	SyncAccountPoll,
	DeviceSignInInfo,
	AiCancelOutcome,
	AiConsentGrant,
	AiConsentGrantInput,
	AiConsentReadInput,
	AiConsentRevokeInput,
	AiConsentRevokeOutcome,
	AiProviderConfig,
	AiStreamFrame,
	AiStreamInput,
	AppendChatContextSummaryInput,
	AppendChatToolResultInput,
	AppendChatUserMessageInput,
	BeginChatAssistantInput,
	BeginChatToolCallInput,
	ChangeChatRetentionInput,
	CalendarEntry,
	Chat,
	ChatRetention,
	ChatMessage,
	ChatRead,
	CoreEvent,
	DraftReconcileInput,
	DraftReconcileResult,
	FolderEntry,
	FinishChatAssistantInput,
	FinishChatToolCallInput,
	ManifestUpdateInput,
	MutationResult,
	Note,
	ObjectPatch,
	ObjectQuery,
	ObjectType,
	Project,
	SearchInput,
	SearchResult,
	Task,
	TaskStatus,
	UnmanagedFile,
	WorkspaceEntry,
	WorkspaceManifest,
	WorkspaceObject,
	WorkspaceState,
	ResolveConflictInput,
	ManagedDraftInput,
	ManagedDraftResult,
	ManagedConflictResolveInput,
	RawMarkdownRead,
	RawReconcileInput,
	RawReconcileResult,
	RawSaveInput,
	RawSaveResult,
	RawConflictResolveInput,
	RawConflictResolveResult,
	MarkdownLinkTarget,
	RenameChatInput,
} from '@noura/shared';
export type * from '@noura/shared';
export { isCoreError } from '@noura/shared';
export { AiRegistry } from '@noura/ai';
export {
	PluginHost,
	type PluginHostServices,
	type PluginManifest,
} from '@noura/plugin-sdk';

export interface CoreTransport {
	request<T>(command: string, payload?: Record<string, unknown>): Promise<T>;
	stream?<T>(
		command: string,
		payload: Record<string, unknown>,
		handler: (frame: T) => void,
	): Promise<void>;
	subscribe(handler: (event: CoreEvent) => void): Promise<() => void>;
}

export { createTauriTransport } from './tauri-transport';
export {
	installPendingDraftCloseGuard,
	type HostCloseRequest,
	type HostLifecycleAdapter,
} from './host-lifecycle';
export { createTauriHostLifecycle } from './tauri-host-lifecycle';
export { firstPartyPlugins } from './first-party';
export {
	PluginRuntime,
	createPluginHostServices,
	type PluginSyncResult,
} from './plugin-runtime';

export interface WorkspaceService {
	pickFolder(input: { title: string }): Promise<string | null>;
	create(input: { path: string; name: string }): Promise<WorkspaceState>;
	open(input: { path: string }): Promise<WorkspaceState>;
	close(): Promise<void>;
	current(): Promise<WorkspaceState>;
	rebuildIndex(): Promise<WorkspaceState>;
	listRecent(): Promise<
		Array<{ path: string; name: string; workspaceId: string }>
	>;
}
export interface ObjectService<T extends WorkspaceObject> {
	list(query?: ObjectQuery): Promise<T[]>;
	get(id: string): Promise<T>;
	create(input: {
		title: string;
		body?: string;
		relativePath?: string;
		properties?: Record<string, unknown>;
	}): Promise<MutationResult<T>>;
	update(id: string, patch: ObjectPatch): Promise<MutationResult<T>>;
	move(input: {
		id: string;
		relativePath: string;
		expectedRevision: string;
	}): Promise<MutationResult<T>>;
	delete(input: {
		id: string;
		expectedRevision: string;
	}): Promise<MutationResult<T>>;
	/** Open this object's enclosing folder in the OS file manager. */
	showInFolder(id: string): Promise<void>;
	/** Open a terminal at this object's folder. */
	openTerminal(id: string): Promise<void>;
}
export interface NoteService extends ObjectService<Note> {
	adopt(input: {
		relativePath: string;
		expectedRevision: string;
	}): Promise<MutationResult<Note>>;
	reconcileDraft(input: DraftReconcileInput): Promise<DraftReconcileResult>;
	resolveConflict(input: ResolveConflictInput): Promise<MutationResult<Note>>;
	saveDraft(input: ManagedDraftInput): Promise<ManagedDraftResult>;
	reconcileManaged(input: ManagedDraftInput): Promise<ManagedDraftResult>;
	resolveManagedConflict(
		input: ManagedConflictResolveInput,
	): Promise<WorkspaceObject>;
}
export interface TaskService extends ObjectService<Task> {
	complete(input: {
		id: string;
		expectedRevision: string;
	}): Promise<MutationResult<Task>>;
	reopen(input: {
		id: string;
		expectedRevision: string;
	}): Promise<MutationResult<Task>>;
	saveDraft(input: ManagedDraftInput): Promise<ManagedDraftResult>;
	reconcileManaged(input: ManagedDraftInput): Promise<ManagedDraftResult>;
	resolveManagedConflict(
		input: ManagedConflictResolveInput,
	): Promise<WorkspaceObject>;
}
export interface ProjectService extends ObjectService<Project> {
	listTasks(input: { projectId: string }): Promise<Task[]>;
	listSummaries(): Promise<Array<{ project: Project; taskCount: number }>>;
	listFolderNotes(input: { projectId: string }): Promise<Note[]>;
	listFolderFiles(input: { projectId: string }): Promise<WorkspaceEntry[]>;
	queryCalendar(input: {
		projectId: string;
		start: string;
		end: string;
	}): Promise<CalendarEntry[]>;
	saveDraft(input: ManagedDraftInput): Promise<ManagedDraftResult>;
	reconcileManaged(input: ManagedDraftInput): Promise<ManagedDraftResult>;
	resolveManagedConflict(
		input: ManagedConflictResolveInput,
	): Promise<WorkspaceObject>;
}
export interface SearchService {
	query(input: SearchInput): Promise<SearchResult[]>;
}
export interface CalendarService {
	queryRange(input: {
		start: string;
		end: string;
		types?: string[];
	}): Promise<CalendarEntry[]>;
}
export interface ChatService {
	list(): Promise<Chat[]>;
	read(id: string): Promise<ChatRead>;
	create(input: {
		title: string;
		retention?: ChatRetention;
	}): Promise<MutationResult<Chat>>;
	changeRetention(
		input: ChangeChatRetentionInput,
	): Promise<MutationResult<Chat>>;
	rename(input: RenameChatInput): Promise<MutationResult<Chat>>;
	appendUserMessage(
		input: AppendChatUserMessageInput,
	): Promise<MutationResult<ChatMessage>>;
	beginAssistant(
		input: BeginChatAssistantInput,
	): Promise<MutationResult<ChatMessage>>;
	finishAssistant(
		input: FinishChatAssistantInput,
	): Promise<MutationResult<ChatMessage>>;
	beginToolCall(
		input: BeginChatToolCallInput,
	): Promise<MutationResult<ChatMessage>>;
	finishToolCall(
		input: FinishChatToolCallInput,
	): Promise<MutationResult<ChatMessage>>;
	appendToolResult(
		input: AppendChatToolResultInput,
	): Promise<MutationResult<ChatMessage>>;
	appendContextSummary(
		input: AppendChatContextSummaryInput,
	): Promise<MutationResult<ChatMessage>>;
	recoverInterrupted(id: string): Promise<ChatRead>;
	expire(now: string): Promise<string[]>;
}
export interface KanbanGroup {
	id: string;
	title: string;
	items: Task[];
}
export interface KanbanService {
	getBoard(input?: { projectId?: string }): Promise<{ groups: KanbanGroup[] }>;
	moveTask(input: {
		taskId: string;
		status: TaskStatus;
		beforeId?: string;
		afterId?: string;
		expectedRevision: string;
	}): Promise<MutationResult<Task>>;
}

export interface FileService {
	inspectPdf(input: { relativePath: string }): Promise<PdfInfo>;
	readPdfRange(input: PdfRangeInput): Promise<Uint8Array>;
	openPdfLink(url: string): Promise<void>;
	list(): Promise<WorkspaceEntry[]>;
	listNonManagedMarkdown(): Promise<UnmanagedFile[]>;
	readRawMarkdown(input: { relativePath: string }): Promise<RawMarkdownRead>;
	saveRawMarkdown(input: RawSaveInput): Promise<RawSaveResult>;
	reconcileRawMarkdown(input: RawReconcileInput): Promise<RawReconcileResult>;
	resolveRawConflict(
		input: RawConflictResolveInput,
	): Promise<RawConflictResolveResult>;
	resolveMarkdownLink(input: {
		sourceRelativePath: string;
		target: string;
	}): Promise<MarkdownLinkTarget>;
	readLocalAsset(input: {
		sourceRelativePath: string;
		target: string;
	}): Promise<{ dataUrl: string }>;
}

export interface GenericObjectService {
	list(query?: ObjectQuery): Promise<WorkspaceObject[]>;
	get(id: string): Promise<WorkspaceObject>;
	create(input: {
		type: ObjectType;
		title: string;
		body?: string;
		relativePath?: string;
		properties?: Record<string, unknown>;
	}): Promise<MutationResult<WorkspaceObject>>;
	update(
		id: string,
		patch: ObjectPatch,
	): Promise<MutationResult<WorkspaceObject>>;
}

export interface ManifestService {
	read(): Promise<WorkspaceManifest>;
	update(input: ManifestUpdateInput): Promise<WorkspaceManifest>;
}

export interface PluginStateService {
	/** Disposable cache state; index rebuilds discard it by design. */
	get<T = unknown>(pluginId: string, key: string): Promise<T | undefined>;
	set(pluginId: string, key: string, value: unknown): Promise<void>;
	delete(pluginId: string, key: string): Promise<boolean>;
}

export interface NouraClient {
	collaboration: {
		open(input: CollaborationOpenInput): Promise<CollaborationSession | null>;
		submitUpdates(
			input: CollaborationSubmitInput,
		): Promise<CollaborationReceipt>;
		flush(input: { sessionId: string }): Promise<void>;
		close(input: { sessionId: string }): Promise<void>;
		setPresence(input: CollaborationPresenceInput): Promise<void>;
	};
	sync: {
		workspaceStatus(): Promise<WorkspaceSyncStatus>;
		workspaceDevices(): Promise<SyncDevice[]>;
		workspaceInvitations(): Promise<SyncInvitation[]>;
		createWorkspaceInvitation(
			role: SyncInvitationRole,
		): Promise<SyncInvitationLink>;
		approveInvitedDevice(
			invitationId: string,
			deviceId: string,
			fingerprint: string,
		): Promise<void>;
		finalizeWorkspaceInvitation(invitationId: string): Promise<void>;
		revokeWorkspaceInvitation(invitationId: string): Promise<void>;
		workspaceConflicts(): Promise<SyncConflict[]>;
		resolveWorkspaceConflict(input: ResolveSyncConflict): Promise<void>;
		remoteWorkspaces(): Promise<RemoteSyncWorkspace[]>;
		joinWorkspace(
			workspaceId: string,
			name: string,
		): Promise<WorkspaceState | null>;
		approveWorkspaceDevice(
			deviceId: string,
			fingerprint: string,
		): Promise<void>;
		enableWorkspace(): Promise<WorkspaceSyncStatus>;
		pauseWorkspace(): Promise<WorkspaceSyncStatus>;
		resumeWorkspace(): Promise<WorkspaceSyncStatus>;
		account(): Promise<SyncAccount | null>;
		beginSignIn(origin: string): Promise<DeviceSignInInfo>;
		openSignInBrowser(): Promise<void>;
		exportRecoveryIdentity(): Promise<boolean>;
		importRecoveryKit(): Promise<boolean>;
		pollSignIn(): Promise<SyncAccountPoll>;
		cancelSignIn(): Promise<void>;
		disconnect(): Promise<void>;
	};
	workspaces: WorkspaceService;
	objects: GenericObjectService;
	manifest: ManifestService;
	pluginState: PluginStateService;
	notes: NoteService;
	tasks: TaskService;
	projects: ProjectService;
	search: SearchService;
	calendar: CalendarService;
	chats: ChatService;
	kanban: KanbanService;
	files: FileService;
	folders: {
		listTree(): Promise<FolderEntry[]>;
		create(input: { relativePath: string }): Promise<void>;
		move(input: { from: string; to: string }): Promise<void>;
		removeEmpty(input: { relativePath: string }): Promise<void>;
	};
	ai: {
		/** The client-scoped registry that activated plugins contribute to. */
		registry: AiRegistry;
		listProviders(): Promise<AiProviderConfig[]>;
		saveProvider(input: AiProviderConfig): Promise<void>;
		setCredential(input: {
			providerId: string;
			secret: string;
		}): Promise<{ credentialRef: string }>;
		deleteCredential(input: { credentialRef: string }): Promise<void>;
		readConsent(input: AiConsentReadInput): Promise<AiConsentGrant | null>;
		grantConsent(input: AiConsentGrantInput): Promise<AiConsentGrant>;
		revokeConsent(input: AiConsentRevokeInput): Promise<AiConsentRevokeOutcome>;
		stream(
			input: AiStreamInput,
			handler: (frame: AiStreamFrame) => void,
		): Promise<void>;
		cancel(operationId: string): Promise<AiCancelOutcome>;
		registerTool(
			definition: AiToolDefinition,
			registration?: AiContributionRegistration,
		): () => boolean;
		registerContextProvider(
			definition: AiContextProvider,
			registration?: AiContributionRegistration,
		): () => boolean;
		registerInstructionProvider(
			definition: AiInstructionProvider,
			registration?: AiContributionRegistration,
		): () => boolean;
	};
	commands: CommandRegistry;
	events: {
		subscribe(handler: (event: CoreEvent) => void): Promise<() => void>;
	};
}

function genericObjects(transport: CoreTransport): GenericObjectService {
	return {
		list: (query = {}) => transport.request('objects_query', { query }),
		get: (id) => transport.request('objects_get', { id }),
		create: (input) => transport.request('objects_create', { input }),
		update: (id, patch) => transport.request('objects_update', { id, patch }),
	};
}

function objects<T extends WorkspaceObject>(
	transport: CoreTransport,
	type: ObjectType,
): ObjectService<T> {
	return {
		list: (query = {}) =>
			transport.request('objects_query', { query: { ...query, type } }),
		get: (id) => transport.request<T>('objects_get', { id }),
		create: (input) =>
			transport.request('objects_create', { input: { ...input, type } }),
		update: (id, patch) => transport.request('objects_update', { id, patch }),
		move: (input) => transport.request('objects_move', { input }),
		delete: (input) => transport.request('objects_delete', { input }),
		showInFolder: (id: string) =>
			transport.request('object_show_in_folder', { id }),
		openTerminal: (id: string) =>
			transport.request('object_open_terminal', { id }),
	};
}

export interface RegisteredCommand {
	id: string;
	title: string;
	execute(input?: unknown): Promise<unknown>;
}
export class CommandRegistry {
	#commands = new Map<string, RegisteredCommand>();
	register(command: RegisteredCommand) {
		if (this.#commands.has(command.id))
			throw new Error(`Command already registered: ${command.id}`);
		this.#commands.set(command.id, command);
		return () => this.#commands.delete(command.id);
	}
	async list() {
		return [...this.#commands.values()].map(({ id, title }) => ({ id, title }));
	}
	async execute<T>(id: string, input?: unknown) {
		const command = this.#commands.get(id);
		if (!command) throw new Error(`Command not found: ${id}`);
		return (await command.execute(input)) as T;
	}
}

export function createNouraClient(
	transport: CoreTransport,
	commands = new CommandRegistry(),
	aiRegistry = new AiRegistry(),
): NouraClient {
	const noteObjects = objects<Note>(transport, 'note');
	const taskObjects = objects<Task>(transport, 'task');
	const projectObjects = objects<Project>(transport, 'project');
	const objectService = genericObjects(transport);
	const manifestService: ManifestService = {
		read: async () =>
			toManifest(await transport.request<ManifestDto>('manifest_read')),
		update: async (input) =>
			toManifest(
				await transport.request<ManifestDto>('manifest_update', {
					input: {
						name: input.name ?? null,
						enabledPlugins: input.enabledPlugins ?? null,
						ignore: input.ignore ?? null,
						expectedUpdated: input.expectedUpdated ?? null,
					},
				}),
			),
	};
	const pluginStateService: PluginStateService = {
		get: async (pluginId, key) => {
			const value = await transport.request<unknown>('plugin_state_get', {
				pluginId,
				key,
			});
			return value === null ? undefined : (value as never);
		},
		set: (pluginId, key, value) =>
			transport.request('plugin_state_set', { pluginId, key, value }),
		delete: (pluginId, key) =>
			transport.request('plugin_state_delete', { pluginId, key }),
	};
	const calendarQuery = async (input: {
		start: string;
		end: string;
		types?: string[];
	}) => {
		const values = await transport.request<CalendarEntry[]>('calendar_query', {
			input,
		});
		return input.types
			? values.filter((value) => input.types?.includes(value.sourceType))
			: values;
	};
	/**
	 * The folder a project file represents: the parent folder it sits in,
	 * matching the Obsidian folder-note convention. A project file at the
	 * workspace root has no containing folder, so it owns no contents.
	 */
	const projectFolder = (project: Project) => {
		const separator = project.relativePath.lastIndexOf('/');
		return separator === -1
			? null
			: project.relativePath.slice(0, separator + 1);
	};
	return {
		collaboration: {
			open: (input) => transport.request('collaboration_open', { input }),
			submitUpdates: (input) =>
				transport.request('collaboration_submit_updates', { input }),
			flush: (input) =>
				transport.request('collaboration_flush', {
					sessionId: input.sessionId,
				}),
			close: (input) =>
				transport.request('collaboration_close', {
					sessionId: input.sessionId,
				}),
			setPresence: (input) =>
				transport.request('collaboration_set_presence', { input }),
		},
		sync: {
			workspaceStatus: () => transport.request('sync_workspace_status'),
			workspaceDevices: () => transport.request('sync_workspace_devices'),
			workspaceInvitations: () =>
				transport.request('sync_workspace_invitations'),
			createWorkspaceInvitation: (role) =>
				transport.request('sync_workspace_create_invitation', { role }),
			approveInvitedDevice: (invitationId, deviceId, fingerprint) =>
				transport.request('sync_workspace_approve_invited_device', {
					invitationId,
					deviceId,
					fingerprint,
				}),
			finalizeWorkspaceInvitation: (invitationId) =>
				transport.request('sync_workspace_finalize_invitation', {
					invitationId,
				}),
			revokeWorkspaceInvitation: (invitationId) =>
				transport.request('sync_workspace_revoke_invitation', {
					invitationId,
				}),
			workspaceConflicts: () => transport.request('sync_workspace_conflicts'),
			resolveWorkspaceConflict: (input) =>
				transport.request('sync_workspace_resolve_conflict', { input }),
			remoteWorkspaces: () => transport.request('sync_remote_workspaces'),
			joinWorkspace: (workspaceId, name) =>
				transport.request('sync_workspace_join', { workspaceId, name }),
			approveWorkspaceDevice: (deviceId, fingerprint) =>
				transport.request('sync_workspace_approve_device', {
					deviceId,
					fingerprint,
				}),
			enableWorkspace: () => transport.request('sync_workspace_enable'),
			pauseWorkspace: () => transport.request('sync_workspace_pause'),
			resumeWorkspace: () => transport.request('sync_workspace_resume'),
			account: () => transport.request('sync_account_current'),
			beginSignIn: (origin) =>
				transport.request('sync_account_begin', { origin }),
			openSignInBrowser: () => transport.request('sync_account_open_browser'),
			exportRecoveryIdentity: () =>
				transport.request('sync_account_export_recovery'),
			importRecoveryKit: () =>
				transport.request('sync_account_import_recovery'),
			pollSignIn: () => transport.request('sync_account_poll'),
			cancelSignIn: () => transport.request('sync_account_cancel'),
			disconnect: () => transport.request('sync_account_disconnect'),
		},
		workspaces: {
			pickFolder: (input) =>
				transport.request('workspace_pick_folder', { title: input.title }),
			create: (input) => transport.request('workspace_create', { input }),
			open: (input) => transport.request('workspace_open', { input }),
			close: () => transport.request('workspace_close'),
			current: () => transport.request('workspace_state'),
			rebuildIndex: () => transport.request('workspace_rebuild_index'),
			listRecent: () => transport.request('workspace_list_recent'),
		},
		objects: objectService,
		manifest: manifestService,
		pluginState: pluginStateService,
		folders: {
			listTree: () => transport.request('folders_list'),
			create: (input) => transport.request('folders_create', { input }),
			move: (input) => transport.request('folders_move', { input }),
			removeEmpty: (input) => transport.request('folders_remove', { input }),
		},
		files: {
			inspectPdf: (input) => transport.request('files_inspect_pdf', input),
			readPdfRange: (input) =>
				transport.request('files_read_pdf_range', { input }),
			openPdfLink: (url) => transport.request('files_open_pdf_link', { url }),
			list: () => transport.request('files_list'),
			listNonManagedMarkdown: () =>
				transport.request('files_list_non_managed_markdown'),
			readRawMarkdown: (input) =>
				transport.request('raw_markdown_read', {
					relativePath: input.relativePath,
				}),
			saveRawMarkdown: (input) =>
				transport.request('raw_markdown_save', { input }),
			reconcileRawMarkdown: (input) =>
				transport.request('raw_markdown_reconcile', { input }),
			resolveRawConflict: (input) =>
				transport.request('raw_markdown_resolve', { input }),
			resolveMarkdownLink: (input) =>
				transport.request('files_resolve_markdown_link', { input }),
			readLocalAsset: (input) =>
				transport.request('files_read_local_asset', { input }),
		},
		notes: {
			...noteObjects,
			adopt: (input) =>
				transport.request('objects_adopt', {
					input: { ...input, type: 'note' },
				}),
			reconcileDraft: (input) =>
				transport.request('notes_reconcile_draft', { input }),
			resolveConflict: (input) =>
				transport.request('notes_resolve_conflict', { input }),
			saveDraft: (input) => transport.request('managed_draft_save', { input }),
			reconcileManaged: (input) =>
				transport.request('managed_draft_reconcile', { input }),
			resolveManagedConflict: (input) =>
				transport.request('managed_conflict_resolve', { input }),
		},
		tasks: {
			...taskObjects,
			complete: ({ id, expectedRevision }) =>
				taskObjects.update(id, {
					expectedRevision,
					properties: { status: 'done' },
				}),
			reopen: ({ id, expectedRevision }) =>
				taskObjects.update(id, {
					expectedRevision,
					properties: { status: 'todo' },
				}),
			saveDraft: (input) => transport.request('managed_draft_save', { input }),
			reconcileManaged: (input) =>
				transport.request('managed_draft_reconcile', { input }),
			resolveManagedConflict: (input) =>
				transport.request('managed_conflict_resolve', { input }),
		},
		projects: {
			...projectObjects,
			listTasks: ({ projectId }) => taskObjects.list({ project: projectId }),
			listSummaries: async () => {
				const [projects, tasks] = await Promise.all([
					projectObjects.list(),
					taskObjects.list(),
				]);
				return projects.map((project) => ({
					project,
					taskCount: tasks.filter(
						(task) => task.properties.project === project.id,
					).length,
				}));
			},
			listFolderNotes: async ({ projectId }) => {
				const project = await projectObjects.get(projectId);
				const folder = projectFolder(project);
				if (folder === null) return [];
				return noteObjects.list({ pathPrefix: folder });
			},
			listFolderFiles: async ({ projectId }) => {
				const project = await projectObjects.get(projectId);
				const prefix = projectFolder(project);
				if (prefix === null) return [];
				return (await transport.request<WorkspaceEntry[]>('files_list')).filter(
					(entry) => entry.relativePath.startsWith(prefix),
				);
			},
			queryCalendar: async ({ projectId, start, end }) => {
				const tasks = await taskObjects.list({ project: projectId });
				const sourceIds = new Set([projectId, ...tasks.map((task) => task.id)]);
				return (await calendarQuery({ start, end })).filter((entry) =>
					sourceIds.has(entry.sourceId),
				);
			},
			saveDraft: (input) => transport.request('managed_draft_save', { input }),
			reconcileManaged: (input) =>
				transport.request('managed_draft_reconcile', { input }),
			resolveManagedConflict: (input) =>
				transport.request('managed_conflict_resolve', { input }),
		},
		search: {
			query: async (input) => {
				const values = await transport.request<
					Array<Omit<SearchResult, 'highlights'>>
				>('search_query', { input });
				return values.map((value) => ({ ...value, highlights: [] }));
			},
		},
		calendar: { queryRange: calendarQuery },
		chats: {
			list: () => transport.request('chats_list'),
			read: (id) => transport.request('chats_read', { id }),
			create: (input) =>
				transport.request('chats_create', {
					input: {
						title: input.title,
						retention: input.retention ?? 'permanent',
						retentionDays: input.retention === 'ephemeral' ? 30 : null,
					},
				}),
			changeRetention: (input) =>
				transport.request('chats_change_retention', { input }),
			rename: (input) => transport.request('chats_rename', { input }),
			appendUserMessage: (input) =>
				transport.request('chats_append_user_message', { input }),
			beginAssistant: (input) =>
				transport.request('chats_begin_assistant', { input }),
			finishAssistant: (input) =>
				transport.request('chats_finish_assistant', { input }),
			beginToolCall: (input) =>
				transport.request('chats_begin_tool_call', { input }),
			finishToolCall: (input) =>
				transport.request('chats_finish_tool_call', { input }),
			appendToolResult: (input) =>
				transport.request('chats_append_tool_result', { input }),
			appendContextSummary: (input) =>
				transport.request('chats_append_context_summary', { input }),
			recoverInterrupted: (id) =>
				transport.request('chats_recover_interrupted', { id }),
			expire: (now) => transport.request('chats_expire', { now }),
		},
		kanban: {
			// The projection and ordering live in the tasks plugin: bundled
			// domains dogfood the same public logic ecosystem plugins use.
			getBoard: async (input = {}) => {
				const tasks = await taskObjects.list(
					input.projectId ? { project: input.projectId } : {},
				);
				return projectKanban(tasks);
			},
			moveTask: async (input) => {
				const tasks = await taskObjects.list();
				// `after` is the card visually above (lower fractional key),
				// `before` the card visually below (higher key); the moved
				// card lands between the two.
				const after = input.afterId
					? tasks.find((task) => task.id === input.afterId)?.properties
							.kanban_order
					: undefined;
				const before = input.beforeId
					? tasks.find((task) => task.id === input.beforeId)?.properties
							.kanban_order
					: undefined;
				const order = nextKanbanOrder(after, before);
				return taskObjects.update(input.taskId, {
					expectedRevision: input.expectedRevision,
					properties: { status: input.status, kanban_order: order },
				});
			},
		},
		ai: {
			registry: aiRegistry,
			listProviders: () => transport.request('ai_provider_list'),
			saveProvider: (input) => transport.request('ai_provider_save', { input }),
			setCredential: (input) =>
				transport.request('ai_credential_set', { input }),
			deleteCredential: (input) =>
				transport.request('ai_credential_delete', { input }),
			readConsent: (input) => transport.request('ai_consent_read', { input }),
			grantConsent: (input) => transport.request('ai_consent_grant', { input }),
			revokeConsent: (input) =>
				transport.request('ai_consent_revoke', { input }),
			stream: (input, handler) =>
				transport.stream
					? transport.stream('ai_stream', { input }, handler)
					: Promise.reject(
							new Error(
								'The selected native transport does not support AI streaming',
							),
						),
			cancel: (operationId) =>
				transport.request('ai_stream_cancel', { operationId }),
			registerTool: (definition, registration) =>
				aiRegistry.registerTool(definition, registration),
			registerContextProvider: (definition, registration) =>
				aiRegistry.registerContextProvider(definition, registration),
			registerInstructionProvider: (definition, registration) =>
				aiRegistry.registerInstructionProvider(definition, registration),
		},
		commands,
		events: { subscribe: (handler) => transport.subscribe(handler) },
	};
}

/**
 * The Rust manifest DTO keeps snake_case frontmatter names on the wire; the
 * public TypeScript contract is camelCase. Fields keep their identities so
 * no durability decision depends on this mapping.
 */
interface ManifestDto {
	id: string;
	format_version: number;
	name: string;
	created: string;
	updated: string;
	enabled_plugins: Array<string>;
	ignore: Array<string>;
}

function toManifest(value: ManifestDto): WorkspaceManifest {
	return {
		id: value.id,
		formatVersion: value.format_version,
		name: value.name,
		created: value.created,
		updated: value.updated,
		enabledPlugins: value.enabled_plugins,
		ignore: value.ignore,
	};
}

export function createWorkspaceStateStore(
	client: NouraClient,
): Readable<WorkspaceState> {
	return readable<WorkspaceState>(
		{ phase: 'idle', indexedFiles: 0, diagnostics: [] },
		(set) => {
			let disposed = false;
			let unsubscribe: undefined | (() => void);
			client.workspaces
				.current()
				.then((state) => {
					if (!disposed) set(state);
				})
				.catch(() => {});
			client.events
				.subscribe((event) => {
					if (event.type.startsWith('workspace:'))
						client.workspaces
							.current()
							.then((state) => {
								if (!disposed) set(state);
							})
							.catch(() => {});
				})
				.then((value) => {
					unsubscribe = value;
					if (disposed) value();
				});
			return () => {
				disposed = true;
				unsubscribe?.();
			};
		},
	);
}
