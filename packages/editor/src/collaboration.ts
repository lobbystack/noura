import * as Y from 'yjs';
import type {
	CollaborationPresenceMember,
	CollaborationSession as NativeCollaborationBootstrap,
	CollaborationStatus as NativeCollaborationStatus,
} from '@noura/shared';

export type CollaborationStatus = NativeCollaborationStatus;
export type CollaborationBootstrap = NativeCollaborationBootstrap;
export interface CollaborationBatch {
	sessionId: string;
	generation: string;
	batchId: string;
	updates: string[];
}
export type CollaborationPresence = CollaborationPresenceMember;
export type CollaborationEvent =
	| { type: 'update'; generation: string; update: string }
	| { type: 'status'; generation: string; status: CollaborationStatus }
	| { type: 'presence'; generation: string; presence: CollaborationPresence[] };
/** Native adapter authenticates, validates, decrypts and expires presence before delivery. */
export interface CollaborationProvider {
	submit(batch: CollaborationBatch): Promise<{ revision: string }>;
	subscribe(handler: (event: CollaborationEvent) => void): () => void;
	flush?(): Promise<void>;
	close?(): Promise<void>;
	setPresence?(
		selection: { anchor: string; head: string } | null,
	): Promise<void>;
}
export const encodeCollaborationUpdate = (bytes: Uint8Array): string => {
	let value = '';
	for (const byte of bytes) value += String.fromCharCode(byte);
	return btoa(value);
};
export const decodeCollaborationUpdate = (value: string): Uint8Array =>
	Uint8Array.from(atob(value), (c) => c.charCodeAt(0));
const remoteOrigin = Symbol('native collaboration');

/** One session is shared by all views of an object. Owner must await close before discarding it. */
export class CollaborationSession {
	readonly doc = new Y.Doc();
	readonly text = this.doc.getText('content');
	readonly localOrigin = Symbol('local editor');
	readonly undoManager = new Y.UndoManager(this.text, {
		trackedOrigins: new Set([this.localOrigin]),
	});
	status: CollaborationStatus;
	revision: string;
	error: unknown = null;
	presence: CollaborationPresence[] = [];
	private pending: Uint8Array[] = [];
	private batch: CollaborationBatch | null = null;
	private inFlight: Promise<void> | null = null;
	private timer: ReturnType<typeof setTimeout> | undefined;
	private listeners = new Set<() => void>();
	private unsubscribe: () => void;
	private closed = false;
	private stale = false;
	get hasPending(): boolean {
		return this.pending.length > 0 || this.batch !== null;
	}
	constructor(
		readonly bootstrap: CollaborationBootstrap,
		private provider: CollaborationProvider,
	) {
		this.revision = bootstrap.revision;
		this.status = bootstrap.status;
		Y.applyUpdate(
			this.doc,
			decodeCollaborationUpdate(bootstrap.update),
			remoteOrigin,
		);
		this.doc.on('update', this.onUpdate);
		this.unsubscribe = provider.subscribe((event) => {
			if (event.generation !== bootstrap.generation) {
				this.stale = true;
				this.undoManager.clear();
				this.status = 'Needs review';
				this.notify();
				return;
			}
			if (event.type === 'update')
				Y.applyUpdate(
					this.doc,
					decodeCollaborationUpdate(event.update),
					remoteOrigin,
				);
			if (event.type === 'status')
				this.status =
					this.hasPending && event.status === 'Synced'
						? 'Saving'
						: event.status;
			if (event.type === 'presence') this.presence = event.presence;
			this.notify();
		});
	}
	private notify() {
		for (const listener of this.listeners) listener();
	}
	subscribe(listener: () => void): () => void {
		this.listeners.add(listener);
		listener();
		return () => {
			this.listeners.delete(listener);
		};
	}
	private onUpdate = (update: Uint8Array, origin: unknown) => {
		if (origin === remoteOrigin) return;
		this.pending = [Y.mergeUpdates([...this.pending, update])];
		this.status =
			this.bootstrap.readOnly || this.stale ? 'Needs review' : 'Saving';
		this.notify();
		if (!this.timer && !this.error)
			this.timer = setTimeout(() => {
				this.timer = undefined;
				void this.flush().catch(() => {});
			}, 100);
	};
	transact(edit: (text: Y.Text) => void) {
		if (this.closed || this.bootstrap.readOnly || this.stale)
			throw new Error('Document is not editable');
		this.doc.transact(() => edit(this.text), this.localOrigin);
	}
	async flush(): Promise<void> {
		if (this.timer) clearTimeout(this.timer);
		this.timer = undefined;
		if (this.inFlight) return this.inFlight;
		if (this.closed) return;
		this.inFlight = this.drain();
		try {
			await this.inFlight;
		} finally {
			this.inFlight = null;
		}
	}
	private async drain(): Promise<void> {
		try {
			while (this.hasPending) {
				if (this.stale || this.bootstrap.readOnly)
					throw new Error('Document requires review');
				if (!this.batch) {
					this.batch = {
						sessionId: this.bootstrap.sessionId,
						generation: this.bootstrap.generation,
						batchId: crypto.randomUUID(),
						updates: [encodeCollaborationUpdate(Y.mergeUpdates(this.pending))],
					};
					this.pending = [];
				}
				const result = await this.provider.submit(this.batch);
				this.revision = result.revision;
				this.batch = null;
				this.error = null;
			}
			await this.provider.flush?.();
			if (this.hasPending) return await this.drain();
			this.status = this.stale ? 'Needs review' : 'Saved locally';
			this.notify();
		} catch (error) {
			this.error = error;
			this.status = 'Needs review';
			this.notify();
			throw error;
		}
	}
	async close() {
		await this.flush();
		try {
			await this.provider.close?.();
		} catch (error) {
			this.error = error;
			this.status = 'Needs review';
			this.notify();
			throw error;
		}
		this.closed = true;
		this.unsubscribe();
		this.doc.off('update', this.onUpdate);
		this.undoManager.destroy();
		(this.doc as Y.Doc & { destroy(): void }).destroy();
		this.listeners.clear();
	}
	setPresence(anchor: number, head: number) {
		return this.provider.setPresence?.({
			anchor: encodeCollaborationUpdate(
				Y.encodeRelativePosition(
					Y.createRelativePositionFromTypeIndex(this.text, anchor),
				),
			),
			head: encodeCollaborationUpdate(
				Y.encodeRelativePosition(
					Y.createRelativePositionFromTypeIndex(this.text, head),
				),
			),
		});
	}
}
