import {
	CollaborationSession,
	type CollaborationBootstrap,
	type CollaborationBatch,
	type CollaborationEvent,
} from '@noura/editor';

export interface NativeCollaborationClient {
	collaboration: {
		open(input: {
			relativePath: string;
		}): Promise<CollaborationBootstrap | null>;
		submitUpdates(input: CollaborationBatch): Promise<{ revision: string }>;
		flush(input: { sessionId: string }): Promise<void>;
		close(input: { sessionId: string }): Promise<void>;
		setPresence?(input: {
			sessionId: string;
			anchor: string | null;
			head: string | null;
		}): Promise<void>;
	};
	events: {
		subscribe(
			handler: (event: { type: string; payload: unknown }) => void,
		): Promise<() => void>;
	};
}
export interface CollaborationLease {
	session: CollaborationSession;
	release(): Promise<void>;
}
type RecordEntry = {
	session: CollaborationSession;
	refs: number;
	unregister: () => void;
	closing?: Promise<void>;
};
type RegisterDraft = (
	id: string,
	flush: () => Promise<boolean>,
	pending: () => boolean,
) => () => void;

/** Scoped to one workspace client; paths only deduplicate opening, object IDs own sessions. */
export class CollaborationRegistry {
	private records = new Map<string, RecordEntry>();
	private openings = new Map<string, Promise<RecordEntry | null>>();
	constructor(
		private client: NativeCollaborationClient,
		private registerDraft: RegisterDraft,
	) {}
	async acquire(relativePath: string): Promise<CollaborationLease | null> {
		let opening = this.openings.get(relativePath);
		if (!opening) {
			opening = this.open(relativePath);
			this.openings.set(relativePath, opening);
		}
		let record: RecordEntry | null;
		try {
			record = await opening;
		} finally {
			if (this.openings.get(relativePath) === opening)
				this.openings.delete(relativePath);
		}
		if (!record) return null;
		if (record.closing) {
			await record.closing;
			return this.acquire(relativePath);
		}
		record.refs++;
		let released = false;
		return {
			session: record.session,
			release: async () => {
				if (released) return;
				released = true;
				record.refs--;
				if (record.refs > 0) return;
				const active = record;
				active.closing = active.session.close().then(() => {
					active.unregister();
					this.records.delete(active.session.bootstrap.objectId);
				});
				try {
					await active.closing;
				} finally {
					active.closing = undefined;
				}
			},
		};
	}
	private async open(relativePath: string): Promise<RecordEntry | null> {
		// Subscribe before bootstrap so an update racing open cannot be lost.
		let handler: ((event: CollaborationEvent) => void) | undefined;
		const buffered: { objectId: string; event: CollaborationEvent }[] = [];
		let overflow = false;
		let objectId: string | undefined;
		const unlisten = await this.client.events.subscribe((event) => {
			if (
				![
					'collaboration:update',
					'collaboration:status',
					'collaboration:presence',
				].includes(event.type)
			)
				return;
			const payload = event.payload as CollaborationEvent & {
				objectId: string;
			};
			const converted = {
				...payload,
				type: event.type.slice('collaboration:'.length),
			} as CollaborationEvent;
			if (handler && objectId === payload.objectId) handler(converted);
			else if (!handler) {
				if (buffered.length >= 256) overflow = true;
				else buffered.push({ objectId: payload.objectId, event: converted });
			}
		});
		try {
			const bootstrap = await this.client.collaboration.open({ relativePath });
			if (!bootstrap) {
				unlisten();
				return null;
			}
			objectId = bootstrap.objectId;
			const existing = this.records.get(objectId);
			if (existing) {
				unlisten();
				if (bootstrap.sessionId !== existing.session.bootstrap.sessionId)
					await this.client.collaboration.close({
						sessionId: bootstrap.sessionId,
					});
				return existing;
			}
			if (overflow) {
				await this.client.collaboration.close({
					sessionId: bootstrap.sessionId,
				});
				throw new Error(
					'Collaboration changed too quickly while opening. Reopen this document.',
				);
			}
			const sessionId = bootstrap.sessionId;
			const session = new CollaborationSession(bootstrap, {
				submit: (batch) => this.client.collaboration.submitUpdates(batch),
				flush: () => this.client.collaboration.flush({ sessionId }),
				close: () => this.client.collaboration.close({ sessionId }),
				setPresence: this.client.collaboration.setPresence
					? async (value) => {
							await this.client.collaboration.setPresence?.({
								sessionId,
								anchor: value?.anchor ?? null,
								head: value?.head ?? null,
							});
						}
					: undefined,
				subscribe: (listener) => {
					handler = listener;
					return unlisten;
				},
			});
			for (const item of buffered)
				if (item.objectId === objectId) handler?.(item.event);
			const unregister = this.registerDraft(
				`collaboration:${sessionId}`,
				async () => {
					await session.flush();
					return !session.hasPending;
				},
				() => session.hasPending || session.error !== null,
			);
			const entry = { session, refs: 0, unregister };
			this.records.set(objectId, entry);
			return entry;
		} catch (error) {
			unlisten();
			throw error;
		}
	}
}
