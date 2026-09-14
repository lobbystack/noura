import { describe, expect, test } from 'bun:test';
import type { EncryptedOperation } from '@noura/shared';
import fixture from '../../../docs/workspace-format/fixtures/operation-v1.json';
import {
	BrowserSyncError,
	BrowserSyncErrorCode,
	accessState,
	decodeBase64,
	encodeBase64,
	listWorkspaces,
	openOperation,
	pullOperations,
	pushOperations,
	sealOperation,
	validateOperation,
	type FetchLike,
	type OperationKind,
} from './index';

const ORIGIN = 'https://app.noura.example';
const TOKEN = 'device-token';

type FixtureKind = 'text' | 'metadata' | 'file';

interface OperationInputs {
	workspace_id: string;
	object_id: string;
	device_id: string;
	epoch: number;
	policy_revision: string;
	operation_id: string;
	nonce: string;
	plaintext_base64: string;
	generation?: string;
	kind?: FixtureKind;
}

interface ValidVector {
	name: string;
	inputs: OperationInputs;
	expected_operation: EncryptedOperation;
	expected_plaintext_base64: string;
}

interface InvalidVector {
	name: string;
	operation: 'verify' | 'open';
	operation_value: EncryptedOperation;
	signing_public: string;
	object_key: string;
	expected_error: string;
}

interface OperationFixture {
	payload_domain: string;
	operation_domain: string;
	version: number;
	object_key: string;
	signing_secret: string;
	signing_public: string;
	valid: ValidVector[];
	invalid: InvalidVector[];
}

const operationFixture = fixture as unknown as OperationFixture;

interface Call {
	input: RequestInfo | URL;
	init: RequestInit | undefined;
}

function recordingFetch(respond: (call: Call) => Response): {
	fetch: FetchLike;
	calls: Call[];
} {
	const calls: Call[] = [];
	const fetchImpl: FetchLike = async (input, init) => {
		const call: Call = { input, init };
		calls.push(call);
		return respond(call);
	};
	return { fetch: fetchImpl, calls };
}

async function makeOperation(): Promise<EncryptedOperation> {
	return sealOperation({
		objectKey: decodeBase64(operationFixture.object_key),
		identity: { signingSeed: decodeBase64(operationFixture.signing_secret) },
		workspaceId: 'workspace',
		objectId: 'object',
		deviceId: 'device_browser',
		epoch: 1,
		policyRevision: '1',
		plaintext: new TextEncoder().encode('operation body'),
	});
}

function expectedCode(vector: InvalidVector): BrowserSyncErrorCode {
	switch (vector.expected_error) {
		case 'sync_invalid_signature':
			return BrowserSyncErrorCode.InvalidOperationSignature;
		case 'sync_decrypt_failed':
			return BrowserSyncErrorCode.OperationDecryptFailed;
		case 'sync_invalid_base64':
			return BrowserSyncErrorCode.InvalidOperation;
		case 'sync_invalid_envelope':
			return vector.name === 'unsupported_version'
				? BrowserSyncErrorCode.UnsupportedOperationVersion
				: BrowserSyncErrorCode.InvalidOperation;
		default:
			throw new Error(`unmapped fixture error ${vector.expected_error}`);
	}
}

describe('operation-v1 fixture byte compatibility', () => {
	test('fixture declares the shared domains', () => {
		expect(operationFixture.payload_domain).toBe('noura.sync.payload');
		expect(operationFixture.operation_domain).toBe('noura.sync.operation');
		expect(operationFixture.version).toBe(1);
		expect(operationFixture.valid.length).toBeGreaterThan(0);
		expect(operationFixture.invalid.length).toBeGreaterThan(0);
	});

	for (const vector of operationFixture.valid) {
		test(`seals and opens exact bytes: ${vector.name}`, async () => {
			const sealed = await sealOperation({
				objectKey: decodeBase64(operationFixture.object_key),
				signingSeed: decodeBase64(operationFixture.signing_secret),
				workspaceId: vector.inputs.workspace_id,
				objectId: vector.inputs.object_id,
				deviceId: vector.inputs.device_id,
				epoch: vector.inputs.epoch,
				policyRevision: vector.inputs.policy_revision,
				plaintext: decodeBase64(vector.inputs.plaintext_base64),
				operationId: vector.inputs.operation_id,
				nonce: decodeBase64(vector.inputs.nonce),
				...(vector.inputs.generation === undefined
					? {}
					: {
							generation: vector.inputs.generation,
							kind: vector.inputs.kind as OperationKind,
						}),
			});
			expect(sealed).toEqual(vector.expected_operation);

			const opened = await openOperation({
				operation: sealed,
				objectKey: decodeBase64(operationFixture.object_key),
				trustedSigningPublicKey: decodeBase64(operationFixture.signing_public),
			});
			expect(encodeBase64(opened)).toBe(vector.expected_plaintext_base64);
		});
	}

	for (const vector of operationFixture.invalid) {
		test(`rejects tampering: ${vector.name}`, async () => {
			await expect(
				openOperation({
					operation: vector.operation_value,
					objectKey: decodeBase64(vector.object_key),
					trustedSigningPublicKey: decodeBase64(vector.signing_public),
				}),
			).rejects.toMatchObject({ code: expectedCode(vector) });
		});
	}
});

describe('validateOperation', () => {
	test('rejects an unsupported version', async () => {
		const operation = await makeOperation();
		let error: unknown;
		try {
			validateOperation({ ...operation, version: 3 });
		} catch (caught) {
			error = caught;
		}
		expect(error).toBeInstanceOf(BrowserSyncError);
		expect((error as BrowserSyncError).code).toBe(
			BrowserSyncErrorCode.UnsupportedOperationVersion,
		);
	});

	test('rejects a malformed identifier', async () => {
		const operation = await makeOperation();
		await expect(
			openOperation({
				operation: { ...operation, objectId: 'not a valid id!' },
				objectKey: decodeBase64(operationFixture.object_key),
				trustedSigningPublicKey: decodeBase64(operationFixture.signing_public),
			}),
		).rejects.toMatchObject({
			code: BrowserSyncErrorCode.InvalidOperation,
		});
	});
});

describe('pushOperations', () => {
	test('posts the operations and returns sequences', async () => {
		const operation = await makeOperation();
		const { fetch, calls } = recordingFetch(() =>
			Response.json({ sequences: ['7'] }),
		);

		const sequences = await pushOperations({
			origin: ORIGIN,
			token: TOKEN,
			workspaceId: 'workspace',
			operations: [operation],
			fetch,
		});

		expect(sequences).toEqual(['7']);
		const call = calls[0]!;
		expect(String(call.input)).toBe(
			`${ORIGIN}/v1/workspaces/workspace/operations`,
		);
		expect(call.init?.method).toBe('POST');
		expect(new Headers(call.init?.headers).get('authorization')).toBe(
			`Bearer ${TOKEN}`,
		);
		const body = JSON.parse(String(call.init?.body)) as {
			operations: EncryptedOperation[];
		};
		expect(body.operations).toEqual([operation]);
	});

	test('rejects an empty batch without calling the transport', async () => {
		let called = false;
		const fetchImpl: FetchLike = async () => {
			called = true;
			return Response.json({ sequences: [] });
		};
		await expect(
			pushOperations({
				origin: ORIGIN,
				token: TOKEN,
				workspaceId: 'workspace',
				operations: [],
				fetch: fetchImpl,
			}),
		).rejects.toMatchObject({ code: BrowserSyncErrorCode.InvalidOperation });
		expect(called).toBe(false);
	});

	test('rejects a response missing sequences', async () => {
		const operation = await makeOperation();
		const { fetch } = recordingFetch(() => Response.json({}));
		await expect(
			pushOperations({
				origin: ORIGIN,
				token: TOKEN,
				workspaceId: 'workspace',
				operations: [operation],
				fetch,
			}),
		).rejects.toMatchObject({
			code: BrowserSyncErrorCode.InvalidResponse,
		});
	});
});

describe('pullOperations', () => {
	function page(overrides: Record<string, unknown> = {}) {
		return {
			accessRevision: '9',
			operations: [],
			cursor: '12',
			hasMore: false,
			...overrides,
		};
	}

	test('preserves a bigint cursor as a decimal string', async () => {
		const operation = await makeOperation();
		const sequenced = { ...operation, sequence: '9223372036854775806' };
		const { fetch, calls } = recordingFetch(() =>
			Response.json(
				page({
					operations: [sequenced],
					cursor: '9223372036854775806',
					hasMore: true,
				}),
			),
		);

		const result = await pullOperations({
			origin: ORIGIN,
			token: TOKEN,
			workspaceId: 'workspace',
			cursor: '0',
			accessRevision: '9',
			wait: 25,
			fetch,
		});

		expect(typeof result.cursor).toBe('string');
		expect(result.cursor).toBe('9223372036854775806');
		expect(result.operations[0]!.sequence).toBe('9223372036854775806');
		expect(result.hasMore).toBe(true);

		const call = calls[0]!;
		const url = new URL(String(call.input));
		expect(call.init?.method).toBe('GET');
		expect(url.pathname).toBe('/v1/workspaces/workspace/operations');
		expect(url.searchParams.get('after')).toBe('0');
		expect(url.searchParams.get('accessRevision')).toBe('9');
		expect(url.searchParams.get('wait')).toBe('25');
		expect(new Headers(call.init?.headers).get('authorization')).toBe(
			`Bearer ${TOKEN}`,
		);
	});

	test('rejects a page missing hasMore', async () => {
		const { fetch } = recordingFetch(() =>
			Response.json(page({ hasMore: undefined })),
		);
		await expect(
			pullOperations({
				origin: ORIGIN,
				token: TOKEN,
				workspaceId: 'workspace',
				cursor: '0',
				fetch,
			}),
		).rejects.toMatchObject({
			code: BrowserSyncErrorCode.InvalidResponse,
		});
	});

	test('surfaces an unauthorized response as a structured error', async () => {
		const { fetch } = recordingFetch(() => new Response(null, { status: 401 }));
		await expect(
			pullOperations({
				origin: ORIGIN,
				token: TOKEN,
				workspaceId: 'workspace',
				cursor: '0',
				fetch,
			}),
		).rejects.toMatchObject({ code: BrowserSyncErrorCode.Unauthorized });
	});
});

describe('listWorkspaces and accessState', () => {
	test('lists workspaces with bearer auth', async () => {
		const { fetch, calls } = recordingFetch(() =>
			Response.json({
				workspaces: [
					{ id: 'workspace-a', role: 'owner' },
					{ id: 'workspace-b', role: 'editor' },
				],
			}),
		);
		const workspaces = await listWorkspaces({
			origin: ORIGIN,
			token: TOKEN,
			fetch,
		});
		expect(workspaces).toEqual([
			{ id: 'workspace-a', role: 'owner' },
			{ id: 'workspace-b', role: 'editor' },
		]);
		expect(new Headers(calls[0]!.init?.headers).get('authorization')).toBe(
			`Bearer ${TOKEN}`,
		);
		expect(String(calls[0]!.input)).toBe(`${ORIGIN}/v1/workspaces`);
	});

	test('reads access state for a workspace', async () => {
		const { fetch, calls } = recordingFetch(() =>
			Response.json({
				revision: '4',
				members: [{ accountId: 'account_owner', role: 'owner' }],
				objects: [],
				envelopes: [],
				devices: [],
				policy: null,
			}),
		);
		const result = await accessState({
			origin: ORIGIN,
			token: TOKEN,
			workspaceId: 'workspace',
			fetch,
		});
		expect(result.revision).toBe('4');
		expect(String(calls[0]!.input)).toBe(
			`${ORIGIN}/v1/workspaces/workspace/access-state`,
		);
	});

	test('rejects an access state missing collections', async () => {
		const { fetch } = recordingFetch(() =>
			Response.json({ revision: '4', members: [] }),
		);
		await expect(
			accessState({
				origin: ORIGIN,
				token: TOKEN,
				workspaceId: 'workspace',
				fetch,
			}),
		).rejects.toMatchObject({
			code: BrowserSyncErrorCode.InvalidResponse,
		});
	});
});
