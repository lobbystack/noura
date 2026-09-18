import { describe, expect, test } from 'bun:test';
import fixture from '../../../docs/workspace-format/fixtures/policy-web-v1.json';
import { accessSigningBytes, type AccessPolicy } from './access-policy';
import { BrowserSyncError, BrowserSyncErrorCode } from './errors';

type UnsignedPolicy = Omit<AccessPolicy, 'signature'>;

const signingText = (policy: UnsignedPolicy): string =>
	new TextDecoder().decode(accessSigningBytes(policy));

describe('accessSigningBytes', () => {
	test('matches the pinned version-1 browser web policy fixture byte for byte', () => {
		const policy = fixture.policy as unknown as UnsignedPolicy;
		expect(signingText(policy)).toBe(fixture.signing_bytes);
	});

	test('matches the pinned mixed age and web policy fixture byte for byte', () => {
		const policy = fixture.mixed.policy as unknown as UnsignedPolicy;
		expect(signingText(policy)).toBe(fixture.mixed.signing_bytes);
		// The web tuple carries four extra fields, so a mixed policy cannot
		// share canonical bytes with the web-only policy.
		expect(fixture.mixed.signing_bytes).not.toBe(fixture.signing_bytes);
	});

	test('rejects a version-2 object without a document descriptor', () => {
		const policy = {
			version: 2,
			workspaceId: 'workspace',
			revision: '1',
			previousPolicyDigest: null,
			deviceId: 'device',
			members: [{ accountId: 'account', role: 'owner' }],
			objects: [{ objectId: 'object', epoch: 1, grants: [], envelopes: [] }],
		} satisfies UnsignedPolicy;

		expect(() => accessSigningBytes(policy)).toThrow(BrowserSyncError);
		try {
			accessSigningBytes(policy);
			throw new Error('expected accessSigningBytes to reject');
		} catch (error) {
			expect((error as BrowserSyncError).code).toBe(
				BrowserSyncErrorCode.InvalidPolicy,
			);
		}
	});
});
