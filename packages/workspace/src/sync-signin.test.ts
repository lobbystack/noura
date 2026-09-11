import { describe, expect, test, setSystemTime } from 'bun:test';
import { createSyncSignIn, type SignInProgress } from './sync-signin';
import type { CoreTransport } from './index';

function harness() {
	const calls: string[] = [];
	let returned = () => {};
	let response: unknown = {
		status: 'connected',
		account: { deviceId: 'device_test' },
	};
	let failure = '';
	const transport: CoreTransport = {
		async request<T>(command: string) {
			calls.push(command);
			if (command === failure) throw { message: 'Unavailable' };
			const results: Record<string, unknown> = {
				sync_service_configuration: { origin: 'http://localhost:1900' },
				sync_account_current: null,
				sync_account_begin: {
					userCode: 'ABCD1234',
					verificationUri:
						'http://localhost:1900/account/device?user_code=ABCD1234',
					expiresIn: 300,
				},
				sync_account_poll: response,
			};
			return results[command] as T;
		},
		async subscribe() {
			return () => {};
		},
		async subscribeSyncReturn(handler) {
			returned = handler;
			return () => {};
		},
	};
	const signIn = createSyncSignIn(transport);
	let value: SignInProgress;
	signIn.subscribe((next) => {
		value = next;
	});
	return {
		signIn,
		calls,
		get value() {
			return value!;
		},
		returned: () => returned(),
		fail: (command: string) => {
			failure = command;
		},
		respond: (next: unknown) => {
			response = next;
		},
	};
}
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('app-lifetime sign-in', () => {
	test('opens browser immediately and keeps the request when a settings subscriber leaves', async () => {
		const h = harness();
		await h.signIn.initialize();
		const unsubscribe = h.signIn.subscribe(() => {});
		await h.signIn.begin();
		unsubscribe();
		expect(h.calls.slice(-2)).toEqual([
			'sync_account_begin',
			'sync_account_open_browser',
		]);
		expect(h.value.request?.userCode).toBe('ABCD1234');
		h.returned();
		await settle();
		expect(h.value.account?.deviceId).toBe('device_test');
		expect(h.calls).not.toContain('sync_workspace_enable');
	});
	test('browser failure retains the request for retry', async () => {
		const h = harness();
		await h.signIn.initialize();
		h.fail('sync_account_open_browser');
		await h.signIn.begin();
		expect(h.value.request).not.toBeNull();
		expect(h.value.error).toBe('Unavailable');
		await h.signIn.cancel();
	});
	test('an unsolicited return cannot authenticate or start a request', async () => {
		const h = harness();
		await h.signIn.initialize();
		h.returned();
		await settle();
		expect(h.value.account).toBeNull();
		expect(h.calls).not.toContain('sync_account_begin');
		expect(h.calls).not.toContain('sync_account_poll');
	});
	test('denied or failed polling ends the pending attempt', async () => {
		const h = harness();
		await h.signIn.initialize();
		await h.signIn.begin();
		h.fail('sync_account_poll');
		h.returned();
		await settle();
		expect(h.value.request).toBeNull();
		expect(h.value.account).toBeNull();
		expect(h.calls).toContain('sync_account_cancel');
	});
	test('explicit cancellation prevents late return from polling', async () => {
		const h = harness();
		await h.signIn.initialize();
		await h.signIn.begin();
		await h.signIn.cancel();
		h.returned();
		await settle();
		expect(h.value.request).toBeNull();
		expect(h.calls).not.toContain('sync_account_poll');
	});
	test('expired requests are cancelled instead of exchanged', async () => {
		const h = harness();
		await h.signIn.initialize();
		await h.signIn.begin();
		try {
			setSystemTime(Date.now() + 301_000);
			h.returned();
			await settle();
			expect(h.value.request).toBeNull();
			expect(h.value.notice).toContain('expired');
			expect(h.calls).not.toContain('sync_account_poll');
		} finally {
			setSystemTime();
			await h.signIn.cancel();
		}
	});
});
