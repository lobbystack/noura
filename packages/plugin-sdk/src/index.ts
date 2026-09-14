import { z } from 'zod';
import type {
	AiContextProvider,
	AiContributionRegistration,
	AiInstructionProvider,
	AiToolDefinition,
} from '@noura/ai';
import type {
	CoreEvent,
	MutationResult,
	ObjectPatch,
	ObjectQuery,
	SearchInput,
	SearchResult,
	UnmanagedFile,
	WorkspaceEntry,
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
	'ai.instructions',
]);
export const pluginPlatformSchema = z.enum(['desktop', 'mobile', 'web']);
const platformActivationCapabilitiesSchema = z
	.object({
		desktop: z.array(capabilitySchema).optional(),
		mobile: z.array(capabilitySchema).optional(),
		web: z.array(capabilitySchema).optional(),
	})
	.strict();
export const pluginManifestSchema = z.object({
	id: z.string().regex(/^[a-z][a-z0-9-]*$/),
	name: z.string().min(1),
	version: z.string(),
	capabilities: z.array(capabilitySchema),
	/** Omitted by pre-platform manifests, which remain compatible everywhere. */
	platforms: z.array(pluginPlatformSchema).min(1).optional(),
	/** Capabilities needed for activation on each platform. */
	activationCapabilities: platformActivationCapabilitiesSchema.optional(),
});
export type PluginCapability = z.infer<typeof capabilitySchema>;
export type PluginPlatform = z.infer<typeof pluginPlatformSchema>;
export type PluginManifest = z.infer<typeof pluginManifestSchema>;
export type { AiContextProvider, AiInstructionProvider, AiToolDefinition };
export interface PluginContext {
	/** The host platform for this activation. */
	platform: PluginPlatform;
	files: {
		list(): Promise<WorkspaceEntry[]>;
		listNonManagedMarkdown(): Promise<UnmanagedFile[]>;
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
		delete(key: string): Promise<boolean>;
	};
	ai: {
		registerTool(definition: AiToolDefinition): () => boolean;
		registerContextProvider(definition: AiContextProvider): () => boolean;
		registerInstructionProvider(
			definition: AiInstructionProvider,
		): () => boolean;
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
	/**
	 * Runs on host.deactivate. Receives the same context instance activate
	 * saw, so handlers and disposers captured during activation stay usable.
	 */
	deactivate?(context: PluginContext): void | Promise<void>;
}
/**
 * Services the host grants to plugins. Storage is addressed per plugin so
 * each plugin's cache values stay namespaced behind its manifest id.
 * Durable plugin data always belongs in workspace files; storage here is
 * disposable cache state (see the plugin runtime docs).
 */
export interface PluginHostServices {
	files: PluginContext['files'];
	objects: PluginContext['objects'];
	search: PluginContext['search'];
	events: PluginContext['events'];
	commands: PluginContext['commands'];
	storage: {
		get<T>(pluginId: string, key: string): Promise<T | undefined>;
		set<T>(pluginId: string, key: string, value: T): Promise<void>;
		delete(pluginId: string, key: string): Promise<boolean>;
	};
	ai: {
		registerTool(
			definition: AiToolDefinition,
			registration: AiContributionRegistration,
		): () => boolean;
		registerContextProvider(
			definition: AiContextProvider,
			registration: AiContributionRegistration,
		): () => boolean;
		registerInstructionProvider(
			definition: AiInstructionProvider,
			registration: AiContributionRegistration,
		): () => boolean;
	};
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

/** A manifest predating platform declarations remains portable by default. */
export function supportsPlatform(
	manifest: PluginManifest,
	platform: PluginPlatform,
): boolean {
	return manifest.platforms?.includes(platform) ?? true;
}

/**
 * Older manifests activate against every declared capability. New manifests
 * may omit services that are intentionally unavailable on a platform.
 */
export function activationCapabilities(
	manifest: PluginManifest,
	platform: PluginPlatform,
): PluginCapability[] {
	return manifest.activationCapabilities?.[platform] ?? manifest.capabilities;
}

export interface PluginHostOptions {
	/** The adapter platform that is activating plugins. Defaults to desktop. */
	platform?: PluginPlatform;
	/**
	 * Capability services implemented by this host. Omit for a full host, such
	 * as the desktop client. A partial host rejects activation before plugin
	 * code receives an unavailable service.
	 */
	supportedCapabilities?: Iterable<PluginCapability>;
}

export class PluginRuntimeError extends Error {
	readonly category = 'validation';
	readonly retryable = false;
	constructor(
		readonly code:
			| 'plugin_already_active'
			| 'plugin_platform_unsupported'
			| 'plugin_capability_unsupported'
			| 'plugin_capability_not_declared',
		message: string,
		readonly operation: 'plugin_activate' | 'plugin_capability',
		readonly details: Record<string, unknown>,
	) {
		super(message);
		this.name = 'PluginRuntimeError';
	}
}

export class PluginHost {
	#active = new Map<string, PluginDefinition>();
	#contexts = new Map<string, PluginContext>();
	/**
	 * Registrations made through a context belong to that activation, even when
	 * a plugin forgets to repeat the disposal in its optional deactivate hook.
	 * This also rolls registrations back when activation throws midway through.
	 */
	#disposers = new Map<string, Set<() => boolean>>();
	private readonly services: PluginHostServices;
	private readonly supportedCapabilities: ReadonlySet<PluginCapability> | null;
	readonly platform: PluginPlatform;
	constructor(services: PluginHostServices, options: PluginHostOptions = {}) {
		this.services = services;
		this.platform = options.platform ?? 'desktop';
		this.supportedCapabilities = options.supportedCapabilities
			? new Set(options.supportedCapabilities)
			: null;
	}
	activationError(definition: PluginDefinition): PluginRuntimeError | null {
		pluginManifestSchema.parse(definition.manifest);
		if (!supportsPlatform(definition.manifest, this.platform)) {
			return new PluginRuntimeError(
				'plugin_platform_unsupported',
				`Plugin ${definition.manifest.id} does not support ${this.platform}`,
				'plugin_activate',
				{ pluginId: definition.manifest.id, platform: this.platform },
			);
		}
		for (const capability of activationCapabilities(
			definition.manifest,
			this.platform,
		)) {
			if (!definition.manifest.capabilities.includes(capability)) {
				return new PluginRuntimeError(
					'plugin_capability_not_declared',
					`Plugin ${definition.manifest.id} activates with undeclared ${capability}`,
					'plugin_activate',
					{ pluginId: definition.manifest.id, capability },
				);
			}
			if (
				this.supportedCapabilities &&
				!this.supportedCapabilities.has(capability)
			) {
				return new PluginRuntimeError(
					'plugin_capability_unsupported',
					`Plugin ${definition.manifest.id} requires unavailable ${capability}`,
					'plugin_activate',
					{
						pluginId: definition.manifest.id,
						capability,
						platform: this.platform,
					},
				);
			}
		}
		return null;
	}
	async activate(definition: PluginDefinition) {
		if (this.#active.has(definition.manifest.id))
			throw new PluginRuntimeError(
				'plugin_already_active',
				`Plugin already active: ${definition.manifest.id}`,
				'plugin_activate',
				{ pluginId: definition.manifest.id },
			);
		const activationError = this.activationError(definition);
		if (activationError) throw activationError;
		this.#disposers.set(definition.manifest.id, new Set());
		const context = this.contextFor(definition.manifest);
		try {
			await definition.activate(context);
			this.#active.set(definition.manifest.id, definition);
			this.#contexts.set(definition.manifest.id, context);
		} catch (error) {
			this.#dispose(definition.manifest.id);
			throw error;
		}
	}
	/** Deactivate a plugin and run its cleanup. Returns whether it was active. */
	async deactivate(id: string): Promise<boolean> {
		const definition = this.#active.get(id);
		if (!definition) return false;
		this.#active.delete(id);
		const context = this.#contexts.get(id);
		this.#contexts.delete(id);
		// Contributions must be gone before user-defined asynchronous cleanup
		// yields. A disabled plugin cannot remain callable during this window.
		this.#dispose(id);
		if (definition.deactivate) await definition.deactivate(context!);
		return true;
	}
	isActive(id: string): boolean {
		return this.#active.has(id);
	}
	activeManifests(): PluginManifest[] {
		return [...this.#active.values()].map((plugin) => plugin.manifest);
	}
	private contextFor(manifest: PluginManifest): PluginContext {
		const guard = (capability: PluginCapability) => {
			if (!manifest.capabilities.includes(capability)) {
				throw new PluginRuntimeError(
					'plugin_capability_not_declared',
					`Plugin ${manifest.id} does not declare ${capability}`,
					'plugin_capability',
					{ pluginId: manifest.id, capability },
				);
			}
			if (
				this.supportedCapabilities &&
				!this.supportedCapabilities.has(capability)
			) {
				throw new PluginRuntimeError(
					'plugin_capability_unsupported',
					`Plugin ${manifest.id} cannot use unavailable ${capability}`,
					'plugin_capability',
					{ pluginId: manifest.id, capability, platform: this.platform },
				);
			}
		};
		return {
			platform: this.platform,
			files: {
				list: () => {
					guard('workspace.files');
					return this.services.files.list();
				},
				listNonManagedMarkdown: () => {
					guard('workspace.files');
					return this.services.files.listNonManagedMarkdown();
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
				subscribe: async (handler) => {
					guard('workspace.events');
					return this.#track(
						manifest.id,
						await this.services.events.subscribe(handler),
					);
				},
			},
			commands: {
				register: (command) => {
					guard('workspace.commands');
					return this.#track(
						manifest.id,
						this.services.commands.register(command),
					);
				},
			},
			storage: {
				get: <T>(key: string) => {
					guard('workspace.storage');
					return this.services.storage.get<T>(manifest.id, key);
				},
				set: <T>(key: string, value: T) => {
					guard('workspace.storage');
					return this.services.storage.set(manifest.id, key, value);
				},
				delete: (key: string) => {
					guard('workspace.storage');
					return this.services.storage.delete(manifest.id, key);
				},
			},
			ai: {
				registerTool: (definition) => {
					guard('ai.tools');
					return this.#track(
						manifest.id,
						this.services.ai.registerTool(definition, { owner: manifest.id }),
					);
				},
				registerContextProvider: (definition) => {
					guard('ai.context');
					return this.#track(
						manifest.id,
						this.services.ai.registerContextProvider(definition, {
							owner: manifest.id,
						}),
					);
				},
				registerInstructionProvider: (definition) => {
					guard('ai.instructions');
					return this.#track(
						manifest.id,
						this.services.ai.registerInstructionProvider(definition, {
							owner: manifest.id,
						}),
					);
				},
			},
		};
	}
	#track(pluginId: string, dispose: () => void): () => boolean {
		const disposers = this.#disposers.get(pluginId);
		// An event subscription can finish after an activation rollback or a
		// deactivation. Do not leave that late registration alive.
		if (!disposers) {
			dispose();
			return () => false;
		}
		let disposed = false;
		const tracked = () => {
			if (disposed) return false;
			disposed = true;
			disposers.delete(tracked);
			dispose();
			return true;
		};
		disposers.add(tracked);
		return tracked;
	}
	#dispose(pluginId: string) {
		const disposers = this.#disposers.get(pluginId);
		this.#disposers.delete(pluginId);
		for (const dispose of disposers ?? []) dispose();
	}
}
