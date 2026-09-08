import { expect, test } from 'bun:test';
import {
	CollaborationRegistry,
	type NativeCollaborationClient,
} from './collaboration-registry';
const bootstrap = {
	objectId: 'object',
	sessionId: 'session',
	generation: 'generation',
	update: 'AAA=',
	revision: 'revision',
	readOnly: false,
	role: 'writer' as const,
	status: 'Synced' as const,
};
function fixture() {
	let opened = 0;
	let closed = 0;
	let registered = 0;
	let unregistered = 0;
	let fail = false;
	const listeners = new Set<
		(event: { type: string; payload: unknown }) => void
	>();
	const client: NativeCollaborationClient = {
		collaboration: {
			async open() {
				opened++;
				return bootstrap;
			},
			async submitUpdates() {
				if (fail) throw new Error('disk full');
				return { revision: 'next' };
			},
			async flush() {},
			async close() {
				closed++;
			},
			async setPresence() {},
		},
		events: {
			async subscribe(handler) {
				listeners.add(handler);
				return () => {
					listeners.delete(handler);
				};
			},
		},
	};
	const registry = new CollaborationRegistry(client, () => {
		registered++;
		return () => {
			unregistered++;
		};
	});
	return {
		registry,
		client,
		counts: () => ({
			opened,
			closed,
			registered,
			unregistered,
			listeners: listeners.size,
		}),
		fail: (value: boolean) => {
			fail = value;
		},
	};
}
test('concurrent views share one bootstrap and close only after final release', async () => {
	const f = fixture();
	const [a, b] = await Promise.all([
		f.registry.acquire('a.md'),
		f.registry.acquire('a.md'),
	]);
	expect(a!.session).toBe(b!.session);
	expect(f.counts().opened).toBe(1);
	await a!.release();
	expect(f.counts().closed).toBe(0);
	await b!.release();
	expect(f.counts()).toEqual({
		opened: 1,
		closed: 1,
		registered: 1,
		unregistered: 1,
		listeners: 0,
	});
});
test('same object at a moved path keeps the session and pending edits', async () => {
	const f = fixture();
	const a = await f.registry.acquire('a.md');
	a!.session.transact((text) => text.insert(0, 'draft'));
	const b = await f.registry.acquire('b.md');
	expect(b!.session).toBe(a!.session);
	expect(b!.session.text.toString()).toBe('draft');
	await a!.release();
	await b!.release();
});
test('failed close preserves registered draft and reacquires the same session for retry', async () => {
	const f = fixture();
	const a = await f.registry.acquire('a.md');
	f.fail(true);
	a!.session.transact((text) => text.insert(0, 'draft'));
	await expect(a!.release()).rejects.toThrow('disk full');
	expect(f.counts().closed).toBe(0);
	expect(f.counts().unregistered).toBe(0);
	f.fail(false);
	const b = await f.registry.acquire('a.md');
	expect(b!.session).toBe(a!.session);
	await b!.release();
	expect(f.counts().closed).toBe(1);
});
test('explicit null is the only fallback and opening errors remove event subscriptions', async () => {
	const f = fixture();
	f.client.collaboration.open = async () => null;
	expect(await f.registry.acquire('local.md')).toBeNull();
	expect(f.counts().listeners).toBe(0);
	f.client.collaboration.open = async () => {
		throw new Error('unverified checkpoint');
	};
	await expect(f.registry.acquire('shared.md')).rejects.toThrow(
		'unverified checkpoint',
	);
	expect(f.counts().listeners).toBe(0);
});
