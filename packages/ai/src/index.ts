export interface AiProviderConfig {
	id: string;
	kind: string;
	displayName: string;
	model: string;
	endpoint?: string;
	credentialRef?: string;
	enabled: boolean;
}

/** A JSON value accepted by a JSON Schema document. */
export type JsonSchemaValue =
	string | number | boolean | null | JsonSchemaObject | Array<JsonSchemaValue>;
export interface JsonSchemaObject {
	[key: string]: JsonSchemaValue;
}
/**
 * Plugin tools use ordinary JSON Schema documents. Pi and TypeBox are kept
 * behind the AI package boundary so plugin authors do not depend on either.
 */
export type JsonSchema = boolean | JsonSchemaObject;

export type AiContributionCategory = 'tool' | 'context' | 'instructions';
export type AiRiskCategory = 'low' | 'medium' | 'high';
export interface AiContributionRegistration {
	/** The registry owner, normally the plugin manifest id. */
	owner?: string;
	/** The host may increase a contribution's risk classification. */
	risk?: AiRiskCategory;
}
export interface AiRegistryEntry<T> {
	category: AiContributionCategory;
	owner: string;
	/** Monotonic per-registry revision used to reject stale registrations. */
	revision: number;
	risk: AiRiskCategory;
	definition: T;
}

export interface AiToolDefinition {
	name: string;
	description: string;
	inputSchema: JsonSchema;
	execute(input: unknown): Promise<unknown>;
	risk?: AiRiskCategory;
}
export interface AiSystemContextInput {
	workspaceId: string;
	query?: string;
}
export interface AiContextProvider {
	id: string;
	provide(
		input: AiSystemContextInput,
	): Promise<Array<{ title: string; content: string; sourceId?: string }>>;
	risk?: AiRiskCategory;
}
export interface AiInstructionProvider {
	id: string;
	provide(input: AiSystemContextInput): Promise<string | Array<string>>;
	risk?: AiRiskCategory;
}

export { createPiRuntimeSpike } from './pi-runtime-spike';
export {
	createNativePiRuntimeSpike,
	type PiRuntimeSpikeFrame,
	type PiRuntimeSpikeTransport,
} from './pi-native-runtime-spike';
export {
	buildSystemPrompt,
	PiChatController,
	isAiRevisionConflict,
	type AiChatCompaction,
	type AiChatCompactionInput,
	type AiChatContextMessage,
	type AiChatContextSummary,
	type AiChatMessageKind,
	type AiChatMessageRef,
	type AiChatMessageStatus,
	type AiChatPersistence,
	type AiChatPersistenceWrite,
	type AiChatRehydration,
	type AiChatRun,
	type AiChatRunResult,
	type AiProviderStreamFrame,
	type AiProviderStreamRequest,
	type AiProviderStreamTransport,
	type AiRevisionConflict,
	type AiToolConsent,
	type AiToolConsentPolicy,
	type AiToolConsentRequest,
	type PiChatControllerOptions,
	type PiChatStartInput,
} from './pi-chat-controller';

export class AiRegistry {
	#tools = new Map<string, AiRegistryEntry<AiToolDefinition>>();
	#contexts = new Map<string, AiRegistryEntry<AiContextProvider>>();
	#instructions = new Map<string, AiRegistryEntry<AiInstructionProvider>>();
	#listeners = new Set<() => void>();
	#nextRevision = 1;

	registerTool(
		value: AiToolDefinition,
		registration: AiContributionRegistration = {},
	) {
		return this.#register(this.#tools, value.name, 'tool', value, registration);
	}
	registerContextProvider(
		value: AiContextProvider,
		registration: AiContributionRegistration = {},
	) {
		return this.#register(
			this.#contexts,
			value.id,
			'context',
			value,
			registration,
		);
	}
	registerInstructionProvider(
		value: AiInstructionProvider,
		registration: AiContributionRegistration = {},
	) {
		return this.#register(
			this.#instructions,
			value.id,
			'instructions',
			value,
			registration,
		);
	}
	tools() {
		return this.toolEntries().map((entry) => entry.definition);
	}
	contextProviders() {
		return this.contextEntries().map((entry) => entry.definition);
	}
	instructionProviders() {
		return this.instructionEntries().map((entry) => entry.definition);
	}
	toolEntries() {
		return this.#entries(this.#tools);
	}
	contextEntries() {
		return this.#entries(this.#contexts);
	}
	instructionEntries() {
		return this.#entries(this.#instructions);
	}
	isCurrent(entry: AiRegistryEntry<unknown>): boolean {
		const key = this.#keyFor(entry);
		if (!key) return false;
		const entries =
			entry.category === 'tool'
				? this.#tools
				: entry.category === 'context'
					? this.#contexts
					: this.#instructions;
		return entries.get(key)?.revision === entry.revision;
	}
	/** Notifies active runs when a plugin contribution is removed or replaced. */
	subscribe(listener: () => void): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}
	#register<T extends { risk?: AiRiskCategory }>(
		entries: Map<string, AiRegistryEntry<T>>,
		key: string,
		category: AiContributionCategory,
		definition: T,
		registration: AiContributionRegistration,
	): () => boolean {
		if (entries.has(key))
			throw new Error(`AI ${category} already registered: ${key}`);
		const entry: AiRegistryEntry<T> = {
			category,
			owner: registration.owner ?? 'noura',
			revision: this.#nextRevision++,
			risk: registration.risk ?? definition.risk ?? 'low',
			definition,
		};
		if (!entry.owner) throw new Error(`AI ${category} owner cannot be empty`);
		entries.set(key, entry);
		this.#notify();
		return () => {
			if (entries.get(key)?.revision !== entry.revision) return false;
			entries.delete(key);
			this.#notify();
			return true;
		};
	}
	#entries<T>(entries: Map<string, AiRegistryEntry<T>>) {
		return [...entries.values()].sort(
			(left, right) =>
				left.owner.localeCompare(right.owner) ||
				this.#keyFor(left)!.localeCompare(this.#keyFor(right)!) ||
				left.revision - right.revision,
		);
	}
	#keyFor(entry: AiRegistryEntry<unknown>): string | undefined {
		if (entry.category === 'tool') {
			const definition = entry.definition as AiToolDefinition;
			return definition.name;
		}
		const definition = entry.definition as
			AiContextProvider | AiInstructionProvider;
		return definition.id;
	}
	#notify() {
		for (const listener of this.#listeners) listener();
	}
}
