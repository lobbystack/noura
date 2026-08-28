export interface AiProviderConfig {
	id: string;
	kind: string;
	displayName: string;
	model: string;
	endpoint?: string;
	credentialRef?: string;
	enabled: boolean;
}
export interface AiToolDefinition {
	name: string;
	description: string;
	inputSchema: Record<string, unknown>;
	execute(input: unknown): Promise<unknown>;
}
export interface AiContextProvider {
	id: string;
	provide(input: {
		workspaceId: string;
		query?: string;
	}): Promise<Array<{ title: string; content: string; sourceId?: string }>>;
}
export class AiRegistry {
	#tools = new Map<string, AiToolDefinition>();
	#contexts = new Map<string, AiContextProvider>();
	registerTool(value: AiToolDefinition) {
		if (this.#tools.has(value.name))
			throw new Error(`AI tool already registered: ${value.name}`);
		this.#tools.set(value.name, value);
		return () => this.#tools.delete(value.name);
	}
	registerContextProvider(value: AiContextProvider) {
		if (this.#contexts.has(value.id))
			throw new Error(`AI context provider already registered: ${value.id}`);
		this.#contexts.set(value.id, value);
		return () => this.#contexts.delete(value.id);
	}
	tools() {
		return [...this.#tools.values()];
	}
	contextProviders() {
		return [...this.#contexts.values()];
	}
}
