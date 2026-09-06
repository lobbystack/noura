/** Versioned opaque transport. This is not the canonical workspace format. */
export interface EncryptedOperation {
	version: 1;
	operationId: string;
	workspaceId: string;
	objectId: string;
	deviceId: string;
	epoch: number;
	/** Signed access-policy revision under which the writer was authorized. */
	policyRevision: string;
	/** Canonical base64: 12-byte nonce, AES-256-GCM ciphertext including tag. */
	nonce: string;
	ciphertext: string;
	/** Ed25519 signature over the protocol's ordered JSON tuple. */
	signature: string;
}

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
