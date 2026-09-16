/**
 * Encrypted operation sealing, opening, and HTTP push/pull for browser devices.
 *
 * A browser device reproduces the native `noura.sync.payload` and
 * `noura.sync.operation` tuples byte-for-byte and uses AES-256-GCM with the
 * object key directly, then signs the resulting envelope with its Ed25519
 * device key. `openOperation` verifies the signature before decrypting, so a
 * forged or tampered envelope never reaches AES-GCM.
 *
 * The `BrowserSyncTransport`/`pushOperations`/`pullOperations` helpers speak the
 * same versioned `SyncPage` wire contract as the native client, keep cursors and
 * sequences as decimal strings (never round a PostgreSQL bigint through a JS
 * number), and use bearer-token auth. The server is never a trust source.
 *
 * This module performs no UI work and never logs key material. JavaScript cannot
 * guarantee memory zeroization; this module clears references it owns only.
 */

import type {
	EncryptedOperation,
	SequencedOperation,
	SyncPage,
} from '@noura/shared';
import {
	NONCE_LENGTH,
	SECRET_LENGTH,
	aesGcmDecrypt,
	aesGcmEncrypt,
	canonicalBytes,
	decodeBase64,
	encodeBase64,
	fixedBytes,
	importAesKey,
	importSigningKey,
	importVerifyKey,
	isIdentifier,
	randomBytes,
	type Bytes,
} from './crypto';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';
import { ensureResponseOk, readJson, type FetchLike } from './http';
import type { DeviceIdentity } from './identity';

/** Domain string authenticated as AES-GCM additional data. */
export const OPERATION_PAYLOAD_DOMAIN = 'noura.sync.payload';

/** Domain string covered by the Ed25519 operation signature. */
export const OPERATION_SIGNING_DOMAIN = 'noura.sync.operation';

/** AES-256-GCM authentication tag length in bytes. */
export const GCM_TAG_BYTES = 16;

/** Maximum accepted ciphertext length in bytes. */
export const MAX_CIPHERTEXT_BYTES = 1024 * 1024;

/** Operation kind carried by a version 2 document operation. */
export type OperationKind = NonNullable<EncryptedOperation['kind']>;

const OPERATION_KINDS: readonly OperationKind[] = ['text', 'metadata', 'file'];

function isOperationKind(value: unknown): value is OperationKind {
	return (
		typeof value === 'string' &&
		(OPERATION_KINDS as readonly string[]).includes(value)
	);
}

/** True for a canonical nonnegative decimal cursor within signed 64-bit range. */
export function isCursor(value: unknown): value is string {
	return (
		typeof value === 'string' &&
		/^(0|[1-9][0-9]{0,18})$/.test(value) &&
		BigInt(value) <= 9223372036854775807n
	);
}

function requirePositiveEpoch(value: unknown, label = 'epoch'): number {
	if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 1) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			`${label} was not a positive safe integer`,
		);
	}
	return value;
}

function decodeOperationBase64(value: string, expectedLength?: number): Bytes {
	try {
		return decodeBase64(value, expectedLength);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation carried invalid base64',
			{ cause },
		);
	}
}

function operationAssociatedData(operation: EncryptedOperation): Bytes {
	const values: (string | number)[] = [
		OPERATION_PAYLOAD_DOMAIN,
		operation.version,
		operation.workspaceId,
		operation.objectId,
		operation.deviceId,
		operation.operationId,
		operation.epoch,
		operation.policyRevision,
	];
	if (operation.version === 2) {
		values.push(operation.generation!, operation.kind!);
	}
	return canonicalBytes(values);
}

function operationSigningBytes(operation: EncryptedOperation): Bytes {
	const values: (string | number)[] = [
		OPERATION_SIGNING_DOMAIN,
		operation.version,
		operation.workspaceId,
		operation.objectId,
		operation.deviceId,
		operation.operationId,
		operation.epoch,
		operation.policyRevision,
		operation.nonce,
		operation.ciphertext,
	];
	if (operation.version === 2) {
		values.push(operation.generation!, operation.kind!);
	}
	return canonicalBytes(values);
}

/**
 * Validate and normalize an operation envelope, rejecting unsupported versions,
 * malformed identifiers, non-canonical epochs, unparsable policy revisions, and
 * bad base64.
 *
 * Unknown fields (for example the `sequence` added by pull) are preserved by
 * callers, not returned; this function returns the canonical envelope fields
 * only.
 */
export function validateOperation(value: unknown): EncryptedOperation {
	if (!value || typeof value !== 'object' || Array.isArray(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation had an unexpected shape',
		);
	}
	const input = value as Record<string, unknown>;
	const version = input.version;
	if (version !== 1 && version !== 2) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.UnsupportedOperationVersion,
			'operation used an unsupported version',
		);
	}
	const operationId = requireIdentifier(input.operationId, 'operationId');
	const workspaceId = requireIdentifier(input.workspaceId, 'workspaceId');
	const objectId = requireIdentifier(input.objectId, 'objectId');
	const deviceId = requireIdentifier(input.deviceId, 'deviceId');
	const epoch = requirePositiveEpoch(input.epoch);
	if (!isCursor(input.policyRevision)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation policy revision was malformed',
		);
	}
	const policyRevision = input.policyRevision;
	if (
		typeof input.nonce !== 'string' ||
		typeof input.ciphertext !== 'string' ||
		typeof input.signature !== 'string'
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation was missing a required field',
		);
	}
	const nonce = input.nonce;
	const ciphertext = input.ciphertext;
	const signature = input.signature;

	const base = {
		operationId,
		workspaceId,
		objectId,
		deviceId,
		epoch,
		policyRevision,
		nonce,
		ciphertext,
		signature,
	};

	if (version === 2) {
		if (!isIdentifier(input.generation) || !isOperationKind(input.kind)) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidOperation,
				'version 2 operation was missing generation or kind',
			);
		}
		decodeOperationBase64(nonce, NONCE_LENGTH);
		checkCiphertextLength(ciphertext);
		decodeOperationBase64(signature, 64);
		return {
			...base,
			version: 2,
			generation: input.generation,
			kind: input.kind,
		};
	}
	if (input.generation !== undefined || input.kind !== undefined) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'version 1 operation carried generation or kind',
		);
	}
	decodeOperationBase64(nonce, NONCE_LENGTH);
	checkCiphertextLength(ciphertext);
	decodeOperationBase64(signature, 64);
	return { ...base, version: 1 };
}

function requireIdentifier(value: unknown, field: string): string {
	if (!isIdentifier(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			`operation ${field} was malformed`,
		);
	}
	return value;
}

function checkCiphertextLength(ciphertext: string): void {
	const bytes = decodeOperationBase64(ciphertext);
	if (bytes.length < GCM_TAG_BYTES || bytes.length > MAX_CIPHERTEXT_BYTES) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation ciphertext had the wrong length',
		);
	}
}

/** Inputs for {@link sealOperation}. */
export interface SealOperationInput {
	/** 32-byte workspace object key, used directly as the AES-256-GCM key. */
	objectKey: Uint8Array;
	/** Workspace identifier bound into the AAD and signature. */
	workspaceId: string;
	/** Stable object identifier bound into the AAD and signature. */
	objectId: string;
	/** Signing device identifier bound into the AAD and signature. */
	deviceId: string;
	/** Positive safe-integer key epoch. */
	epoch: number;
	/** Canonical nonnegative decimal access-policy revision. */
	policyRevision: string;
	/** Workspace plaintext to encrypt. Never sent to the server unencrypted. */
	plaintext: Uint8Array;
	/** Raw Ed25519 signing seed. Required unless `identity` is supplied. */
	signingSeed?: Uint8Array;
	/** Unlocked device identity supplying the signing seed. */
	identity?: Pick<DeviceIdentity, 'signingSeed'>;
	/** Document generation; requires `kind` and selects version 2. */
	generation?: string;
	/** Operation kind; requires `generation` and selects version 2. */
	kind?: OperationKind;
	/** Operation id. Defaults to a fresh random UUID. Never reuse with new bytes. */
	operationId?: string;
	/** 12-byte AES-GCM nonce. Defaults to fresh random bytes. */
	nonce?: Uint8Array;
}

/**
 * Seal a workspace operation for transport.
 *
 * The envelope is encrypted with AES-256-GCM under the object key with the
 * `noura.sync.payload` AAD tuple, then signed with the device Ed25519 key over
 * the `noura.sync.operation` tuple. Version 2 operations bind `generation` and
 * `kind` in both tuples; version 1 operations must not carry them.
 *
 * `operationId` and `nonce` are injectable only so conformance tests can
 * reproduce the shared fixture; production callers omit them and receive fresh
 * random values.
 */
export async function sealOperation(
	input: SealOperationInput,
): Promise<EncryptedOperation> {
	const seedSource = input.identity?.signingSeed ?? input.signingSeed;
	if (!seedSource) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidIdentity,
			'a signing seed or identity is required',
		);
	}
	const signingSeed = fixedBytes(seedSource, SECRET_LENGTH, 'signing seed');
	const objectKey = fixedBytes(input.objectKey, SECRET_LENGTH, 'object key');
	if (
		!isIdentifier(input.workspaceId) ||
		!isIdentifier(input.objectId) ||
		!isIdentifier(input.deviceId)
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation identifiers were malformed',
		);
	}
	const epoch = requirePositiveEpoch(input.epoch);
	if (!isCursor(input.policyRevision)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'policy revision was not a canonical decimal string',
		);
	}
	const hasGeneration = input.generation !== undefined;
	const hasKind = input.kind !== undefined;
	if (hasGeneration !== hasKind) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'generation and kind must be provided together',
		);
	}
	if (hasGeneration && !isIdentifier(input.generation)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'generation was malformed',
		);
	}
	if (hasKind && !isOperationKind(input.kind)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation kind was malformed',
		);
	}
	const version: 1 | 2 = hasGeneration ? 2 : 1;
	const operationId = input.operationId ?? globalThis.crypto.randomUUID();
	if (!isIdentifier(operationId)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation id was malformed',
		);
	}
	const nonce = input.nonce
		? fixedBytes(input.nonce, NONCE_LENGTH, 'nonce')
		: randomBytes(NONCE_LENGTH);
	const plaintext = new Uint8Array(input.plaintext);

	const draft: EncryptedOperation = {
		version,
		operationId,
		workspaceId: input.workspaceId,
		objectId: input.objectId,
		deviceId: input.deviceId,
		epoch,
		policyRevision: input.policyRevision,
		nonce: encodeBase64(nonce),
		ciphertext: '',
		signature: '',
		...(version === 2
			? { generation: input.generation!, kind: input.kind! }
			: {}),
	};

	const ciphertextKey = await importAesKey(objectKey, ['encrypt']);
	const ciphertext = await aesGcmEncrypt(
		ciphertextKey,
		nonce,
		operationAssociatedData(draft),
		plaintext,
	);
	draft.ciphertext = encodeBase64(ciphertext);

	const signingKey = await importSigningKey(signingSeed);
	const signature = await globalThis.crypto.subtle.sign(
		{ name: 'Ed25519' },
		signingKey,
		operationSigningBytes(draft),
	);
	draft.signature = encodeBase64(new Uint8Array(signature));
	return draft;
}

/** Inputs for {@link openOperation}. */
export interface OpenOperationInput {
	/** Operation envelope received over the transport. */
	operation: EncryptedOperation;
	/** 32-byte object key delivered for the operation's object. */
	objectKey: Uint8Array;
	/** Caller-pinned raw Ed25519 public key of the signing device. */
	trustedSigningPublicKey: Uint8Array;
}

/**
 * Verify and open an operation envelope.
 *
 * The signature is verified against the caller-pinned signer before any
 * decryption. A missing trust pin, a malformed envelope, a bad signature, or an
 * AES-GCM authentication failure is reported as a structured error.
 */
export async function openOperation(input: OpenOperationInput): Promise<Bytes> {
	const operation = validateOperation(input.operation);
	const objectKey = fixedBytes(input.objectKey, SECRET_LENGTH, 'object key');
	const trusted = fixedBytes(
		input.trustedSigningPublicKey,
		SECRET_LENGTH,
		'trusted signing public key',
	);

	const verifyKey = await importVerifyKey(trusted);
	const valid = await globalThis.crypto.subtle.verify(
		{ name: 'Ed25519' },
		verifyKey,
		decodeOperationBase64(operation.signature, 64),
		operationSigningBytes(operation),
	);
	if (!valid) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperationSignature,
			'operation signature did not verify',
		);
	}

	const ciphertextKey = await importAesKey(objectKey, ['decrypt']);
	try {
		return await aesGcmDecrypt(
			ciphertextKey,
			decodeOperationBase64(operation.nonce, NONCE_LENGTH),
			operationAssociatedData(operation),
			decodeOperationBase64(operation.ciphertext),
		);
	} catch (cause) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.OperationDecryptFailed,
			'operation could not be decrypted',
			{ cause },
		);
	}
}

/** Shared origin, bearer token, and injected transport for transport calls. */
export interface SyncTransportOptions {
	/** Account origin, for example `https://app.noura.example`. */
	origin: string;
	/** Device bearer token. */
	token: string;
	/** Injected transport. */
	fetch: FetchLike;
}

function asRecord(value: unknown, label = 'response'): Record<string, unknown> {
	if (!value || typeof value !== 'object' || Array.isArray(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			`${label} had an unexpected shape`,
		);
	}
	return value as Record<string, unknown>;
}

function requireString(value: unknown, field: string): string {
	if (typeof value !== 'string' || value.length === 0) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			`response was missing ${field}`,
		);
	}
	return value;
}

function requireCursor(value: unknown, field: string): string {
	if (!isCursor(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			`response carried an invalid ${field}`,
		);
	}
	return value;
}

function requireSequence(value: unknown): string {
	if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response carried an invalid sequence',
		);
	}
	return value;
}

function parseSequencedOperation(value: unknown): SequencedOperation {
	const operation = validateOperation(value);
	return {
		...operation,
		sequence: requireSequence(asRecord(value).sequence),
	};
}

/** Parse and validate a `SyncPage` response, preserving decimal-string cursors. */
export function parseSyncPage(value: unknown): SyncPage {
	const data = asRecord(value);
	if (!Array.isArray(data.operations)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response was missing operations',
		);
	}
	if (typeof data.hasMore !== 'boolean') {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response was missing hasMore',
		);
	}
	return {
		accessRevision: requireCursor(data.accessRevision, 'accessRevision'),
		operations: data.operations.map(parseSequencedOperation),
		cursor: requireCursor(data.cursor, 'cursor'),
		hasMore: data.hasMore,
	};
}

/** Inputs for {@link pushOperations}. */
export interface PushOperationsInput extends SyncTransportOptions {
	/** Workspace to push into. */
	workspaceId: string;
	/** Operations to send. The server accepts one to one hundred per request. */
	operations: EncryptedOperation[];
}

/**
 * Push signed operations.
 *
 * `POST /v1/workspaces/:workspace/operations` with a `{operations}` body returns
 * the server-assigned sequences as decimal strings. The batch bound matches the
 * server's `sync.invalid_batch` limit.
 */
export async function pushOperations(
	input: PushOperationsInput,
): Promise<string[]> {
	if (
		!Array.isArray(input.operations) ||
		input.operations.length < 1 ||
		input.operations.length > 100
	) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'operation batch size was invalid',
		);
	}
	const operations = input.operations.map(validateOperation);
	const response = await input.fetch(
		new URL(
			`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/operations`,
			input.origin,
		).toString(),
		{
			method: 'POST',
			credentials: 'include',
			headers: {
				authorization: `Bearer ${input.token}`,
				'content-type': 'application/json',
				accept: 'application/json',
			},
			body: JSON.stringify({ operations }),
		},
	);
	ensureResponseOk(response);
	const data = asRecord(await readJson(response));
	if (!Array.isArray(data.sequences)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response was missing sequences',
		);
	}
	return data.sequences.map(requireSequence);
}

/** Inputs for {@link pullOperations}. */
export interface PullOperationsInput extends SyncTransportOptions {
	/** Workspace to pull from. */
	workspaceId: string;
	/** Cursor returned by the previous pull; `"0"` starts from the beginning. */
	cursor: string;
	/** Optional access revision used by long polling to detect revocation. */
	accessRevision?: string;
	/** Optional long-poll wait in seconds; the server accepts only `25`. */
	wait?: 25;
}

/**
 * Pull operations after a cursor.
 *
 * `GET /v1/workspaces/:workspace/operations?after=<cursor>` returns an
 * `accessRevision`, `operations`, `cursor`, and `hasMore`. Optional
 * `accessRevision` and `wait=25` support long polling. Cursors and sequences
 * stay decimal strings end to end.
 */
export async function pullOperations(
	input: PullOperationsInput,
): Promise<SyncPage> {
	if (!isCursor(input.cursor)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'cursor was not a canonical decimal string',
		);
	}
	if (input.accessRevision !== undefined && !isCursor(input.accessRevision)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'access revision was not a canonical decimal string',
		);
	}
	if (input.wait !== undefined && input.wait !== 25) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidOperation,
			'wait was not the supported value',
		);
	}
	const url = new URL(
		`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/operations`,
		input.origin,
	);
	url.searchParams.set('after', input.cursor);
	if (input.accessRevision !== undefined) {
		url.searchParams.set('accessRevision', input.accessRevision);
	}
	if (input.wait !== undefined) {
		url.searchParams.set('wait', String(input.wait));
	}
	const response = await input.fetch(url.toString(), {
		method: 'GET',
		credentials: 'include',
		headers: {
			authorization: `Bearer ${input.token}`,
			accept: 'application/json',
		},
	});
	ensureResponseOk(response);
	return parseSyncPage(await readJson(response));
}

/** A workspace membership entry returned by {@link listWorkspaces}. */
export interface WorkspaceSummary {
	/** Workspace identifier. */
	id: string;
	/** Caller's role in the workspace. */
	role: string;
}

/** List the workspaces the bearer token can access. */
export async function listWorkspaces(
	input: SyncTransportOptions,
): Promise<WorkspaceSummary[]> {
	const response = await input.fetch(
		new URL('/v1/workspaces', input.origin).toString(),
		{
			method: 'GET',
			credentials: 'include',
			headers: {
				authorization: `Bearer ${input.token}`,
				accept: 'application/json',
			},
		},
	);
	ensureResponseOk(response);
	const data = asRecord(await readJson(response));
	if (!Array.isArray(data.workspaces)) {
		throw new BrowserSyncError(
			BrowserSyncErrorCode.InvalidResponse,
			'response was missing workspaces',
		);
	}
	return data.workspaces.map((value) => {
		const row = asRecord(value, 'workspace');
		return {
			id: requireString(row.id, 'id'),
			role: requireString(row.role, 'role'),
		};
	});
}

/**
 * Access state returned by {@link accessState}.
 *
 * The browser client treats the member, object, device, and envelope collections
 * as opaque records; native or key-delivery code owns their interpretation.
 */
export interface AccessState {
	/** Current access revision as a decimal string. */
	revision: string;
	/** Workspace members and roles. */
	members: unknown[];
	/** Objects with epoch, generation, and document mode. */
	objects: unknown[];
	/** Recipient-wrapped key envelopes. */
	envelopes: unknown[];
	/** Active, non-revoked devices. */
	devices: unknown[];
	/** Latest signed access policy, or `null`. */
	policy: unknown;
	[key: string]: unknown;
}

/** Inputs for {@link accessState}. */
export interface AccessStateInput extends SyncTransportOptions {
	/** Workspace to read access state for. */
	workspaceId: string;
}

/** Read the current access state for a workspace. */
export async function accessState(
	input: AccessStateInput,
): Promise<AccessState> {
	const response = await input.fetch(
		new URL(
			`/v1/workspaces/${encodeURIComponent(input.workspaceId)}/access-state`,
			input.origin,
		).toString(),
		{
			method: 'GET',
			credentials: 'include',
			headers: {
				authorization: `Bearer ${input.token}`,
				accept: 'application/json',
			},
		},
	);
	ensureResponseOk(response);
	const data = asRecord(await readJson(response));
	requireCursor(data.revision, 'revision');
	for (const field of ['members', 'objects', 'envelopes', 'devices']) {
		if (!Array.isArray(data[field])) {
			throw new BrowserSyncError(
				BrowserSyncErrorCode.InvalidResponse,
				`response was missing ${field}`,
			);
		}
	}
	return data as AccessState;
}

/**
 * Transport bound to one origin and bearer token.
 *
 * Methods delegate to {@link pushOperations}, {@link pullOperations},
 * {@link listWorkspaces}, and {@link accessState}.
 */
export class BrowserSyncTransport {
	private readonly options: SyncTransportOptions;

	constructor(options: SyncTransportOptions) {
		this.options = options;
	}

	/** Push operations; see {@link pushOperations}. */
	push(
		workspaceId: string,
		operations: EncryptedOperation[],
	): Promise<string[]> {
		return pushOperations({ ...this.options, workspaceId, operations });
	}

	/** Pull operations; see {@link pullOperations}. */
	pull(
		workspaceId: string,
		cursor: string,
		options: { accessRevision?: string; wait?: 25 } = {},
	): Promise<SyncPage> {
		return pullOperations({
			...this.options,
			workspaceId,
			cursor,
			...options,
		});
	}

	/** List accessible workspaces; see {@link listWorkspaces}. */
	listWorkspaces(): Promise<WorkspaceSummary[]> {
		return listWorkspaces(this.options);
	}

	/** Read access state; see {@link accessState}. */
	accessState(workspaceId: string): Promise<AccessState> {
		return accessState({ ...this.options, workspaceId });
	}
}
