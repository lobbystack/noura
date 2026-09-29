import { z } from 'zod';
import type {
	AiContextProvider,
	AiContributionRegistration,
	AiInstructionProvider,
	AiToolDefinition,
} from '@noura/ai';
import type {
	CollaborationOpenInput,
	CollaborationPresenceInput,
	CollaborationReceipt,
	CollaborationSession,
	CollaborationSubmitInput,
	CoreEvent,
	MutationResult,
	ObjectPatch,
	ObjectFilter,
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
	'workspace.collaboration',
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
	// Matches the workspace manifest rule, so an enabled plugin ID is always
	// valid in .noura/workspace.yaml.
	id: z.string().regex(/^[a-z][a-z0-9-]{0,63}$/),
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

/**
 * Capabilities only trusted first-party plugins may hold. They reach native
 * services that handle credentials or encrypted workspace keys.
 */
export const trustedCapabilities: ReadonlySet<PluginCapability> = new Set([
	'workspace.collaboration',
]);

/**
 * Live text collaboration for one workspace. Editors reach it only through
 * the provider a plugin registers; with no provider, documents open as plain
 * local files and nothing asks the native side about collaboration.
 */
export interface CollaborationProvider {
	open(input: CollaborationOpenInput): Promise<CollaborationSession | null>;
	submitUpdates(input: CollaborationSubmitInput): Promise<CollaborationReceipt>;
	flush(input: { sessionId: string }): Promise<void>;
	close(input: { sessionId: string }): Promise<void>;
	setPresence(input: CollaborationPresenceInput): Promise<void>;
}
export interface PluginContext {
	/** The host platform for this activation. */
	platform: PluginPlatform;
	/**
	 * Capabilities this activation actually holds: manifest-declared,
	 * activated on this platform, and implemented by the host. A plugin uses
	 * this to register optional contributions conditionally instead of
	 * catching the capability guard's errors.
	 */
	capabilities: ReadonlySet<PluginCapability>;
	/** Whether `capability` is held by this activation. */
	hasCapability(capability: PluginCapability): boolean;
	files: {
		list(): Promise<WorkspaceEntry[]>;
		listNonManagedMarkdown(): Promise<UnmanagedFile[]>;
		createFolder(relativePath: string): Promise<void>;
		moveFolder(from: string, to: string): Promise<void>;
		removeEmptyFolder(relativePath: string): Promise<void>;
	};
	objects: {
		list(query?: ObjectFilter): Promise<WorkspaceObject[]>;
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
	/** Trusted: requires `workspace.collaboration`. */
	collaboration: {
		/** The host's native collaboration service. */
		service: CollaborationProvider;
		/** Make `provider` the workspace's collaboration provider until disposed. */
		registerProvider(provider: CollaborationProvider): () => boolean;
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
	collaboration: {
		service: CollaborationProvider;
		registerProvider(
			provider: CollaborationProvider,
			registration: { owner: string },
		): () => boolean;
	};
}

/**
 * Validate a manifest and return a deep-frozen snapshot. The host must never
 * trust a manifest object a plugin can mutate after validation.
 */
function freezeManifest(value: PluginManifest): PluginManifest {
	const manifest = pluginManifestSchema.parse(value);
	Object.freeze(manifest.capabilities);
	if (manifest.platforms) Object.freeze(manifest.platforms);
	if (manifest.activationCapabilities) {
		for (const key of ['desktop', 'mobile', 'web'] as const) {
			const list = manifest.activationCapabilities[key];
			if (list) Object.freeze(list);
		}
		Object.freeze(manifest.activationCapabilities);
	}
	return Object.freeze(manifest);
}

export function definePlugin(definition: PluginDefinition): PluginDefinition {
	// Return a frozen snapshot so a plugin holding its original manifest cannot
	// expand its declared capabilities after validation.
	return { ...definition, manifest: freezeManifest(definition.manifest) };
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
	/**
	 * Plugin definitions allowed to hold trusted capabilities, matched by
	 * object identity. Omit to trust none.
	 */
	trustedPlugins?: Iterable<PluginDefinition>;
}

export class PluginRuntimeError extends Error {
	readonly category = 'validation';
	readonly retryable = false;
	constructor(
		readonly code:
			| 'plugin_already_active'
			| 'plugin_platform_unsupported'
			| 'plugin_capability_unsupported'
			| 'plugin_capability_not_declared'
			| 'plugin_capability_untrusted',
		message: string,
		readonly operation: 'plugin_activate' | 'plugin_capability',
		readonly details: Record<string, unknown>,
	) {
		super(message);
		this.name = 'PluginRuntimeError';
	}
}

interface ActivePlugin {
	definition: PluginDefinition;
	manifest: PluginManifest;
}

export class PluginHost {
	#active = new Map<string, ActivePlugin>();
	#contexts = new Map<string, PluginContext>();
	/**
	 * Registrations made through a context belong to that activation, even when
	 * a plugin forgets to repeat the disposal in its optional deactivate hook.
	 * This also rolls registrations back when activation throws midway through.
	 */
	#disposers = new Map<string, Set<() => boolean>>();
	private readonly services: PluginHostServices;
	private readonly supportedCapabilities: ReadonlySet<PluginCapability> | null;
	private readonly trustedPlugins: ReadonlySet<PluginDefinition>;
	readonly platform: PluginPlatform;
	constructor(services: PluginHostServices, options: PluginHostOptions = {}) {
		this.services = services;
		this.platform = options.platform ?? 'desktop';
		this.supportedCapabilities = options.supportedCapabilities
			? new Set(options.supportedCapabilities)
			: null;
		this.trustedPlugins = new Set(options.trustedPlugins ?? []);
	}
	activationError(definition: PluginDefinition): PluginRuntimeError | null {
		return this.#activationError(definition, definition.manifest);
	}
	#activationError(
		definition: PluginDefinition,
		manifest: PluginManifest,
	): PluginRuntimeError | null {
		pluginManifestSchema.parse(manifest);
		if (!this.trustedPlugins.has(definition)) {
			const capability = manifest.capabilities.find((value) =>
				trustedCapabilities.has(value),
			);
			if (capability) {
				return new PluginRuntimeError(
					'plugin_capability_untrusted',
					`Plugin ${manifest.id} cannot hold trusted ${capability}`,
					'plugin_activate',
					{ pluginId: manifest.id, capability },
				);
			}
		}
		if (!supportsPlatform(manifest, this.platform)) {
			return new PluginRuntimeError(
				'plugin_platform_unsupported',
				`Plugin ${manifest.id} does not support ${this.platform}`,
				'plugin_activate',
				{ pluginId: manifest.id, platform: this.platform },
			);
		}
		for (const capability of activationCapabilities(manifest, this.platform)) {
			if (!manifest.capabilities.includes(capability)) {
				return new PluginRuntimeError(
					'plugin_capability_not_declared',
					`Plugin ${manifest.id} activates with undeclared ${capability}`,
					'plugin_activate',
					{ pluginId: manifest.id, capability },
				);
			}
			if (
				this.supportedCapabilities &&
				!this.supportedCapabilities.has(capability)
			) {
				return new PluginRuntimeError(
					'plugin_capability_unsupported',
					`Plugin ${manifest.id} requires unavailable ${capability}`,
					'plugin_activate',
					{
						pluginId: manifest.id,
						capability,
						platform: this.platform,
					},
				);
			}
		}
		return null;
	}
	async activate(definition: PluginDefinition) {
		const manifest = freezeManifest(definition.manifest);
		if (this.#active.has(manifest.id))
			throw new PluginRuntimeError(
				'plugin_already_active',
				`Plugin already active: ${manifest.id}`,
				'plugin_activate',
				{ pluginId: manifest.id },
			);
		const activationError = this.#activationError(definition, manifest);
		if (activationError) throw activationError;
		this.#disposers.set(manifest.id, new Set());
		const context = this.contextFor(
			manifest,
			this.trustedPlugins.has(definition),
		);
		try {
			await definition.activate(context);
			this.#active.set(manifest.id, { definition, manifest });
			this.#contexts.set(manifest.id, context);
		} catch (error) {
			this.#dispose(manifest.id);
			throw error;
		}
	}
	/** Deactivate a plugin and run its cleanup. Returns whether it was active. */
	async deactivate(id: string): Promise<boolean> {
		const active = this.#active.get(id);
		if (!active) return false;
		this.#active.delete(id);
		const context = this.#contexts.get(id);
		this.#contexts.delete(id);
		// Contributions must be gone before user-defined asynchronous cleanup
		// yields. A disabled plugin cannot remain callable during this window.
		this.#dispose(id);
		if (active.definition.deactivate)
			await active.definition.deactivate(context!);
		return true;
	}
	isActive(id: string): boolean {
		return this.#active.has(id);
	}
	activeManifests(): PluginManifest[] {
		return [...this.#active.values()].map((active) => active.manifest);
	}
	private contextFor(
		manifest: PluginManifest,
		trusted: boolean,
	): PluginContext {
		const granted = new Set(
			activationCapabilities(manifest, this.platform).filter(
				(capability) =>
					manifest.capabilities.includes(capability) &&
					(this.supportedCapabilities === null ||
						this.supportedCapabilities.has(capability)),
			),
		);
		const guard = (capability: PluginCapability) => {
			if (trustedCapabilities.has(capability) && !trusted) {
				throw new PluginRuntimeError(
					'plugin_capability_untrusted',
					`Plugin ${manifest.id} cannot use trusted ${capability}`,
					'plugin_capability',
					{ pluginId: manifest.id, capability },
				);
			}
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
			capabilities: granted,
			hasCapability: (capability) => granted.has(capability),
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
			collaboration: {
				service: {
					open: (input) => {
						guard('workspace.collaboration');
						return this.services.collaboration.service.open(input);
					},
					submitUpdates: (input) => {
						guard('workspace.collaboration');
						return this.services.collaboration.service.submitUpdates(input);
					},
					flush: (input) => {
						guard('workspace.collaboration');
						return this.services.collaboration.service.flush(input);
					},
					close: (input) => {
						guard('workspace.collaboration');
						return this.services.collaboration.service.close(input);
					},
					setPresence: (input) => {
						guard('workspace.collaboration');
						return this.services.collaboration.service.setPresence(input);
					},
				},
				registerProvider: (provider) => {
					guard('workspace.collaboration');
					return this.#track(
						manifest.id,
						this.services.collaboration.registerProvider(provider, {
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
