import { describe, expect, test } from 'bun:test';
import {
	installPendingDraftCloseGuard,
	type HostCloseRequest,
	type HostLifecycleAdapter,
} from './host-lifecycle';

function fakeHost() {
	let handler: ((event: HostCloseRequest) => void | Promise<void>) | undefined;
	let forced = 0;
	const host: HostLifecycleAdapter = {
		onCloseRequested: async (next) => {
			handler = next;
			return () => {
				handler = undefined;
			};
		},
		forceClose: async () => {
			forced += 1;
		},
	};
	return {
		host,
		requestClose: async () => {
			let prevented = false;
			await handler?.({ preventDefault: () => (prevented = true) });
			return prevented;
		},
		get forced() {
			return forced;
		},
	};
}

describe('pending draft close guard', () => {
	test('forces the host closed only after a successful flush', async () => {
		const fake = fakeHost();
		await installPendingDraftCloseGuard(fake.host, async () => true);

		await expect(fake.requestClose()).resolves.toBe(true);
		expect(fake.forced).toBe(1);
	});

	test('keeps the host open after a failed flush', async () => {
		const fake = fakeHost();
		await installPendingDraftCloseGuard(fake.host, async () => false);

		await expect(fake.requestClose()).resolves.toBe(true);
		expect(fake.forced).toBe(0);
	});
});
