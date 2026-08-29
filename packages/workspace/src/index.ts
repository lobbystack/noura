import { readable, type Readable } from 'svelte/store';
import { generateKeyBetween } from 'fractional-indexing';
import {
	AiRegistry,
	type AiContextProvider,
	type AiToolDefinition,
} from '@noura/ai';
import type {
	AiInvokeInput,
	AiProviderConfig,
	AiResponse,
	CalendarEntry,
	CoreEvent,
	FolderEntry,
	MutationResult,
	Note,
	ObjectPatch,
	ObjectQuery,
	Project,
	SearchInput,
	SearchResult,
	Task,
	TaskStatus,
	UnmanagedFile,
	WorkspaceEntry,
	WorkspaceObject,
	WorkspaceState,
} from '@noura/shared';
export type * from '@noura/shared';

export interface CoreTransport {
	request<T>(command: string, payload?: Record<string, unknown>): Promise<T>;
	subscribe(handler: (event: CoreEvent) => void): Promise<() => void>;
}

export { createTauriTransport } from './tauri-transport';
export { activateFirstPartyPlugins, firstPartyPlugins } from './first-party';

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
}
export interface NoteService extends ObjectService<Note> {
	adopt(input: {
		relativePath: string;
		expectedRevision: string;
	}): Promise<MutationResult<Note>>;
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
}
export interface ProjectService extends ObjectService<Project> {
	listTasks(input: { projectId: string }): Promise<Task[]>;
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
	list(): Promise<WorkspaceEntry[]>;
	listNonManagedMarkdown(): Promise<UnmanagedFile[]>;
}

export interface NouraClient {
	workspaces: WorkspaceService;
	notes: NoteService;
	tasks: TaskService;
	projects: ProjectService;
	search: SearchService;
	calendar: CalendarService;
	kanban: KanbanService;
	files: FileService;
	folders: {
		listTree(): Promise<FolderEntry[]>;
		create(input: { relativePath: string }): Promise<void>;
		move(input: { from: string; to: string }): Promise<void>;
		removeEmpty(input: { relativePath: string }): Promise<void>;
	};
	ai: {
		listProviders(): Promise<AiProviderConfig[]>;
		saveProvider(input: AiProviderConfig): Promise<void>;
		setCredential(input: {
			providerId: string;
			secret: string;
		}): Promise<{ credentialRef: string }>;
		deleteCredential(input: { credentialRef: string }): Promise<void>;
		invoke(input: AiInvokeInput): Promise<AiResponse>;
		registerTool(definition: AiToolDefinition): () => boolean;
		registerContextProvider(definition: AiContextProvider): () => boolean;
	};
	commands: CommandRegistry;
	events: {
		subscribe(handler: (event: CoreEvent) => void): Promise<() => void>;
	};
}

function objects<T extends WorkspaceObject>(
	transport: CoreTransport,
	type: string,
): ObjectService<T> {
	return {
		list: (query = {}) =>
			transport.request('objects_query', { query: { ...query, type } }),
		get: (id) => transport.request('objects_get', { id }),
		create: (input) =>
			transport.request('objects_create', { input: { ...input, type } }),
		update: (id, patch) => transport.request('objects_update', { id, patch }),
		move: (input) => transport.request('objects_move', { input }),
		delete: (input) => transport.request('objects_delete', { input }),
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
	return {
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
		folders: {
			listTree: () => transport.request('folders_list'),
			create: (input) => transport.request('folders_create', { input }),
			move: (input) => transport.request('folders_move', { input }),
			removeEmpty: (input) => transport.request('folders_remove', { input }),
		},
		files: {
			list: () => transport.request('files_list'),
			listNonManagedMarkdown: () =>
				transport.request('files_list_non_managed_markdown'),
		},
		notes: {
			...noteObjects,
			adopt: (input) =>
				transport.request('objects_adopt', {
					input: { ...input, type: 'note' },
				}),
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
		},
		projects: {
			...projectObjects,
			listTasks: ({ projectId }) => taskObjects.list({ project: projectId }),
		},
		search: {
			query: async (input) => {
				const values = await transport.request<
					Array<Omit<SearchResult, 'highlights'>>
				>('search_query', { input });
				return values.map((value) => ({ ...value, highlights: [] }));
			},
		},
		calendar: {
			queryRange: async (input) => {
				const values = await transport.request<CalendarEntry[]>(
					'calendar_query',
					{ input },
				);
				return input.types
					? values.filter((value) => input.types?.includes(value.sourceType))
					: values;
			},
		},
		kanban: {
			getBoard: async (input = {}) => {
				const tasks = await taskObjects.list(
					input.projectId ? { project: input.projectId } : {},
				);
				const statuses: TaskStatus[] = [
					'todo',
					'in-progress',
					'done',
					'cancelled',
				];
				return {
					groups: statuses.map((status) => ({
						id: status,
						title: status,
						items: tasks
							.filter((task) => task.properties.status === status)
							.sort((left, right) =>
								(left.properties.kanban_order ?? left.id).localeCompare(
									right.properties.kanban_order ?? right.id,
								),
							),
					})),
				};
			},
			moveTask: async (input) => {
				const tasks = await taskObjects.list();
				const after = input.afterId
					? tasks.find((task) => task.id === input.afterId)?.properties
							.kanban_order
					: undefined;
				const before = input.beforeId
					? tasks.find((task) => task.id === input.beforeId)?.properties
							.kanban_order
					: undefined;
				const order = generateKeyBetween(after ?? null, before ?? null);
				return taskObjects.update(input.taskId, {
					expectedRevision: input.expectedRevision,
					properties: { status: input.status, kanban_order: order },
				});
			},
		},
		ai: {
			listProviders: () => transport.request('ai_provider_list'),
			saveProvider: (input) => transport.request('ai_provider_save', { input }),
			setCredential: (input) =>
				transport.request('ai_credential_set', { input }),
			deleteCredential: (input) =>
				transport.request('ai_credential_delete', { input }),
			invoke: (input) => transport.request('ai_invoke', { input }),
			registerTool: (definition) => aiRegistry.registerTool(definition),
			registerContextProvider: (definition) =>
				aiRegistry.registerContextProvider(definition),
		},
		commands,
		events: { subscribe: (handler) => transport.subscribe(handler) },
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
