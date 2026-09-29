import { describe, expect, test } from 'bun:test';
import {
	CollaborationProviderSlot,
	createNouraClient,
	type CoreTransport,
} from '@noura/workspace';
import { createCollaborationAccess } from './collaboration-access';

function harness(open: unknown = null) {
	const requests: string[] = [];
	let subscriptions = 0;
	const transport: CoreTransport = {
		async request<T>(command: string) {
			requests.push(command);
			if (command === 'collaboration_open') return open as T;
			return null as T;
		},
		async subscribe() {
			subscriptions++;
			return () => {};
		},
	};
	const client = createNouraClient(transport);
	const slot = new CollaborationProviderSlot();
	const access = createCollaborationAccess({
		slot,
		events: client.events,
		registerDraft: () => () => {},
		scope: () => 'workspace',
	});
	return {
		access,
		client,
		slot,
		requests,
		subscriptions: () => subscriptions,
	};
}

describe('collaboration access', () => {
	test('an empty slot opens documents without any native request', async () => {
		const { access, requests, subscriptions } = harness();
		expect(access.available).toBe(false);
		expect(await access.acquire('note.md')).toBeNull();
		expect(requests).toEqual([]);
		expect(subscriptions()).toBe(0);
	});

	test('a registered provider asks the native side exactly once', async () => {
		const { access, client, slot, requests } = harness();
		const dispose = slot.register(client.collaboration, 'sync');
		expect(access.available).toBe(true);
		expect(await access.acquire('note.md')).toBeNull();
		expect(
			requests.filter((command) => command === 'collaboration_open'),
		).toHaveLength(1);
		dispose();
		expect(access.available).toBe(false);
		expect(await access.acquire('note.md')).toBeNull();
		expect(
			requests.filter((command) => command === 'collaboration_open'),
		).toHaveLength(1);
	});

	test('sessions do not survive a provider change', async () => {
		const bootstrap = {
			objectId: 'object',
			sessionId: 'session',
			generation: 'generation',
			update: 'AAA=',
			revision: 'revision',
			readOnly: false,
			role: 'writer',
			status: 'Synced',
		};
		const { access, client, slot, requests } = harness(bootstrap);
		// Each plugin activation registers a fresh provider object.
		const first = slot.register({ ...client.collaboration }, 'sync');
		const lease = await access.acquire('note.md');
		expect(lease?.session.bootstrap.objectId).toBe('object');
		first();
		slot.register({ ...client.collaboration }, 'sync');
		const next = await access.acquire('note.md');
		expect(next?.session).not.toBe(lease?.session);
		expect(
			requests.filter((command) => command === 'collaboration_open'),
		).toHaveLength(2);
	});
});
