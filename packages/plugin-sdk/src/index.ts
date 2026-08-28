import { z } from 'zod';
import type { AiContextProvider, AiToolDefinition } from '@noura/ai';
import type {
	CoreEvent,
	FolderEntry,
	MutationResult,
	ObjectPatch,
	ObjectQuery,
	SearchInput,
	SearchResult,
	WorkspaceObject,
} from '@noura/shared';

export const capabilitySchema = z.enum([
	'workspace.files',
	'workspace.objects',
	'workspace.search',
	'workspace.commands',
	'workspace.events',
	'workspace.storage',
	'ai.tools',
	'ai.context',
]);
export const pluginManifestSchema = z.object({
	id: z.string().regex(/^[a-z][a-z0-9-]*$/),
	name: z.string().min(1),
	version: z.string(),
	capabilities: z.array(capabilitySchema),
});
export type PluginCapability = z.infer<typeof capabilitySchema>;
export type PluginManifest = z.infer<typeof pluginManifestSchema>;
export interface PluginContext {
	files: {
		list(): Promise<FolderEntry[]>;
		createFolder(relativePath: string): Promise<void>;
		moveFolder(from: string, to: string): Promise<void>;
		removeEmptyFolder(relativePath: string): Promise<void>;
	};
	objects: {
		list(query?: ObjectQuery): Promise<WorkspaceObject[]>;
		get(id: string): Promise<WorkspaceObject>;
		create(input: unknown): Promise<MutationResult<WorkspaceObject>>;
		update(
			id: string,
			patch: ObjectPatch,
		): Promise<MutationResult<WorkspaceObject>>;
	};
	search: { query(input: SearchInput): Promise<SearchResult[]> };
	events: {
		subscribe(handler: (event: CoreEvent) => void): Promise<() => void>;
	};
	commands: { register(command: PluginCommand): () => void };
	storage: {
		get<T>(key: string): Promise<T | undefined>;
		set<T>(key: string, value: T): Promise<void>;
	};
	ai: {
		registerTool(definition: AiToolDefinition): () => boolean;
		registerContextProvider(definition: AiContextProvider): () => boolean;
	};
}
export interface PluginCommand {
	id: string;
	title: string;
	execute(input?: unknown): Promise<unknown>;
}
export interface PluginDefinition {
	manifest: PluginManifest;
	activate(context: PluginContext): void | Promise<void>;
}

export function definePlugin(definition: PluginDefinition): PluginDefinition {
	pluginManifestSchema.parse(definition.manifest);
	return definition;
}
export function requireCapability(
	manifest: PluginManifest,
	capability: PluginCapability,
): void {
	if (!manifest.capabilities.includes(capability))
		throw new Error(`Plugin ${manifest.id} does not declare ${capability}`);
}

export class PluginHost {
	#active = new Map<string, PluginDefinition>();
	constructor(private readonly services: PluginContext) {}
	async activate(definition: PluginDefinition) {
		if (this.#active.has(definition.manifest.id))
			throw new Error(`Plugin already active: ${definition.manifest.id}`);
		pluginManifestSchema.parse(definition.manifest);
		await definition.activate(this.contextFor(definition.manifest));
		this.#active.set(definition.manifest.id, definition);
	}
	activeManifests() {
		return [...this.#active.values()].map((plugin) => plugin.manifest);
	}
	private contextFor(manifest: PluginManifest): PluginContext {
		const guard = (capability: PluginCapability) =>
			requireCapability(manifest, capability);
		return {
			files: {
				list: () => {
					guard('workspace.files');
					return this.services.files.list();
				},
				createFolder: (relativePath) => {
					guard('workspace.files');
					return this.services.files.createFolder(relativePath);
				},
				moveFolder: (from, to) => {
					guard('workspace.files');
					return this.services.files.moveFolder(from, to);
				},
				removeEmptyFolder: (relativePath) => {
					guard('workspace.files');
					return this.services.files.removeEmptyFolder(relativePath);
				},
			},
			objects: {
				list: (query) => {
					guard('workspace.objects');
					return this.services.objects.list(query);
				},
				get: (id) => {
					guard('workspace.objects');
					return this.services.objects.get(id);
				},
				create: (input) => {
					guard('workspace.objects');
					return this.services.objects.create(input);
				},
				update: (id, patch) => {
					guard('workspace.objects');
					return this.services.objects.update(id, patch);
				},
			},
			search: {
				query: (input) => {
					guard('workspace.search');
					return this.services.search.query(input);
				},
			},
			events: {
				subscribe: (handler) => {
					guard('workspace.events');
					return this.services.events.subscribe(handler);
				},
			},
			commands: {
				register: (command) => {
					guard('workspace.commands');
					return this.services.commands.register(command);
				},
			},
			storage: {
				get: <T>(key: string) => {
					guard('workspace.storage');
					return this.services.storage.get<T>(`${manifest.id}:${key}`);
				},
				set: <T>(key: string, value: T) => {
					guard('workspace.storage');
					return this.services.storage.set(`${manifest.id}:${key}`, value);
				},
			},
			ai: {
				registerTool: (definition) => {
					guard('ai.tools');
					return this.services.ai.registerTool(definition);
				},
				registerContextProvider: (definition) => {
					guard('ai.context');
					return this.services.ai.registerContextProvider(definition);
				},
			},
		};
	}
}
