import { expect, test } from 'bun:test';
import fixture from '../../../docs/workspace-format/fixtures/checkpoint-v1.json';
import liveFixture from '../../../docs/workspace-format/fixtures/collaboration-v2.json';
import { operation, verifyOperation } from './protocol';
import {
	accessTransition,
	transitionDigest,
	verifyTransition,
} from './checkpoints';
import { verifyAccess } from './access';

test('Rust version-two policy, generation and operation signatures match the relay', () => {
	const value = accessTransition(liveFixture.transition);
	verifyAccess(value.policy, liveFixture.publicKey);
	verifyTransition(value, liveFixture.publicKey);
	verifyOperation(operation(liveFixture.liveOperation), liveFixture.publicKey);
	expect(transitionDigest(value)).toBe(liveFixture.digest);
	expect(value.policy.version).toBe(2);
	const changed = structuredClone(liveFixture.transition);
	changed.policy.objects[0]!.document.generation = 'replayed';
	expect(() =>
		verifyAccess(accessTransition(changed).policy, liveFixture.publicKey),
	).toThrow();
});

test('Rust checkpoint signatures and digest match the relay protocol', () => {
	const value = accessTransition(fixture.transition);
	verifyAccess(value.policy, fixture.publicKey);
	verifyTransition(value, fixture.publicKey);
	expect(transitionDigest(value)).toBe(fixture.digest);
	expect(JSON.stringify(value)).not.toContain('private/note.txt');
	expect(JSON.stringify(value)).not.toContain('current content');
});

test('checkpoint generation, boundary, signature and transaction identity cannot be replaced', () => {
	for (const field of ['generation', 'coveredSequence', 'signature']) {
		const value = structuredClone(fixture.transition);
		value.checkpoints[0]![field as 'generation'] =
			field === 'signature' ? Buffer.alloc(64).toString('base64') : '5';
		expect(() =>
			verifyTransition(accessTransition(value), fixture.publicKey),
		).toThrow();
	}
	const changed = { ...fixture.transition, transitionId: 'replayed' };
	expect(() =>
		verifyTransition(accessTransition(changed), fixture.publicKey),
	).toThrow('sync.invalid_signature');
});

test('checkpoint wire format rejects unknown fields, duplicate objects and unsupported versions', () => {
	expect(() =>
		accessTransition({ ...fixture.transition, version: 2 }),
	).toThrow();
	expect(() =>
		accessTransition({ ...fixture.transition, plaintext: 'secret' }),
	).toThrow();
	expect(() =>
		accessTransition({
			...fixture.transition,
			checkpoints: [
				...fixture.transition.checkpoints,
				...fixture.transition.checkpoints,
			],
		}),
	).toThrow();
	const changed = structuredClone(fixture.transition);
	changed.checkpoints[0]!.coveredSequence = '9223372036854775808';
	expect(() => accessTransition(changed)).toThrow();
});
