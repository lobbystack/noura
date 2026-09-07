/** Versioned opaque transport. This is not the canonical workspace format. */
import type { EncryptedOperation } from './generated/EncryptedOperation';
export type { EncryptedOperation };
export type { AccessTransition } from './generated/AccessTransition';
export type { EncryptedCheckpoint } from './generated/EncryptedCheckpoint';

export interface SequencedOperation extends EncryptedOperation {
	sequence: string;
}

export interface SyncPage {
	accessRevision: string;
	operations: SequencedOperation[];
	/** Decimal integer; never round a PostgreSQL bigint through a JS number. */
	cursor: string;
	hasMore: boolean;
}

export interface SyncTransport {
	push(operations: EncryptedOperation[]): Promise<{ sequences: string[] }>;
	pull(workspaceId: string, cursor: string): Promise<SyncPage>;
}
