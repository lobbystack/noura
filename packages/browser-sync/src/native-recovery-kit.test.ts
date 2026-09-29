import { describe, expect, test } from 'bun:test';
import nativeFixture from '../../../docs/workspace-format/fixtures/native-recovery-v1.json';
import { extractEmbeddedRecoveryIdentity } from './native-recovery-kit';

interface NativeRecoveryFixture {
	recovery_identity: string;
}

const NATIVE_KIT = nativeFixture as unknown as NativeRecoveryFixture;

describe('native recovery kit parsing', () => {
	test('extracts the embedded recovery identity, or null when absent', () => {
		expect(extractEmbeddedRecoveryIdentity(NATIVE_KIT)).toBe(
			NATIVE_KIT.recovery_identity,
		);
		const { recovery_identity: _omitted, ...withoutIdentity } = NATIVE_KIT;
		expect(extractEmbeddedRecoveryIdentity(withoutIdentity)).toBeNull();
		expect(() =>
			extractEmbeddedRecoveryIdentity({ format: 'other' }),
		).toThrow();
	});
});
