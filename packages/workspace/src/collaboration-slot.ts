import type { CollaborationProvider } from '@noura/plugin-sdk';

export interface CollaborationSlotSnapshot {
	/** The registered provider, or null when collaboration is off. */
	provider: CollaborationProvider | null;
	/** Plugin id that registered the provider. */
	owner: string | null;
	/** Increments whenever the provider changes, so views can reopen documents. */
	generation: number;
}

/**
 * Holds at most one collaboration provider for the open workspace. Editors
 * ask the slot before they open a document: with no provider they skip
 * collaboration entirely and never reach the native side. `subscribe`
 * follows the Svelte store contract.
 */
export class CollaborationProviderSlot {
	#provider: CollaborationProvider | null = null;
	#owner: string | null = null;
	#generation = 0;
	#listeners = new Set<(snapshot: CollaborationSlotSnapshot) => void>();

	get provider(): CollaborationProvider | null {
		return this.#provider;
	}

	get owner(): string | null {
		return this.#owner;
	}

	get generation(): number {
		return this.#generation;
	}

	snapshot(): CollaborationSlotSnapshot {
		return {
			provider: this.#provider,
			owner: this.#owner,
			generation: this.#generation,
		};
	}

	/**
	 * Install `provider` until the returned disposer runs. A second provider
	 * is rejected while one is registered.
	 */
	register(provider: CollaborationProvider, owner: string): () => boolean {
		if (this.#provider)
			throw new Error(
				`A collaboration provider from ${this.#owner} is already registered`,
			);
		this.#provider = provider;
		this.#owner = owner;
		this.#generation++;
		this.#publish();
		return () => {
			if (this.#provider !== provider) return false;
			this.#provider = null;
			this.#owner = null;
			this.#generation++;
			this.#publish();
			return true;
		};
	}

	subscribe(listener: (snapshot: CollaborationSlotSnapshot) => void) {
		this.#listeners.add(listener);
		listener(this.snapshot());
		return () => {
			this.#listeners.delete(listener);
		};
	}

	#publish() {
		const snapshot = this.snapshot();
		for (const listener of [...this.#listeners]) listener(snapshot);
	}
}
