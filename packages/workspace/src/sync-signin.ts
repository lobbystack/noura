import { writable } from 'svelte/store';
import type {
	DeviceSignInInfo,
	SyncAccount,
	SyncAccountPoll,
} from '@noura/shared';
import type { CoreTransport } from './index';

export type SyncServiceConfiguration = { origin: string | null };
export type SignInProgress = {
	account: SyncAccount | null;
	request: DeviceSignInInfo | null;
	loading: boolean;
	busy: boolean;
	error: string;
	notice: string;
	origin: string | null;
	returnCount: number;
};
function message(cause: unknown): string {
	if (cause && typeof cause === 'object' && 'code' in cause) {
		const messages: Record<string, string> = {
			sync_credentials_unavailable:
				'Noura could not access secure credential storage. Check your operating system’s credential-store access and restart Noura, then try again.',
			sync_signin_denied:
				'This sign-in request was denied. Log in again when you are ready.',
			sync_signin_expired: 'Sign-in expired. Please log in again.',
			sync_signin_unavailable:
				'Unable to reach the Noura account service. Check your connection and try logging in again.',
		};
		const text = messages[String(cause.code)];
		if (text) return text;
	}
	return cause && typeof cause === 'object' && 'message' in cause
		? String(cause.message)
		: 'Unable to connect. Please try again.';
}

/** App-lifetime orchestration; all credentials and authorization remain native. */
export function createSyncSignIn(transport: CoreTransport) {
	let value: SignInProgress = {
		account: null,
		request: null,
		loading: true,
		busy: false,
		error: '',
		notice: '',
		origin: null,
		returnCount: 0,
	};
	const state = writable(value);
	const update = (patch: Partial<SignInProgress>) => {
		value = { ...value, ...patch };
		state.set(value);
	};
	let initialization: Promise<void> | undefined;
	let generation = 0;
	let timer: ReturnType<typeof setTimeout> | undefined;
	let expiresAt = 0;
	let polling = false;
	const clear = () => {
		clearTimeout(timer);
		timer = undefined;
	};
	async function refresh() {
		const account = await transport.request<SyncAccount | null>(
			'sync_account_current',
		);
		update({ account });
	}
	async function cancel(notice = 'Sign-in cancelled.') {
		++generation;
		clear();
		update({ busy: true });
		try {
			await transport.request('sync_account_cancel');
			await refresh();
			update({ request: null, error: '', notice });
		} catch (cause) {
			update({ error: message(cause) });
		} finally {
			update({ busy: false });
		}
	}
	async function poll(attempt: number) {
		if (attempt !== generation || !value.request || polling) return;
		if (Date.now() >= expiresAt) {
			await cancel('Sign-in expired. Please log in again.');
			return;
		}
		polling = true;
		try {
			const result =
				await transport.request<SyncAccountPoll>('sync_account_poll');
			if (attempt !== generation) return;
			if (result.status === 'connected') {
				clear();
				update({
					account: result.account,
					request: null,
					error: '',
					notice: 'Logged in. You can now enable sync for this workspace.',
				});
			} else {
				timer = setTimeout(
					() => void poll(attempt),
					Math.min(
						Math.max(1, result.retryAfter) * 1000,
						Math.max(0, expiresAt - Date.now()),
					),
				);
			}
		} catch (cause) {
			if (attempt !== generation) return;
			await transport.request('sync_account_cancel').catch(() => {});
			update({ request: null, error: message(cause) });
		} finally {
			polling = false;
		}
	}
	async function returned() {
		update({ returnCount: value.returnCount + 1 });
		if (value.request) {
			clear();
			void poll(generation);
		} else {
			try {
				await refresh();
				if (!value.account)
					update({ notice: 'Please log in again to connect this device.' });
			} catch (cause) {
				update({ error: message(cause) });
			}
		}
	}
	function initialize() {
		return (initialization ??= (async () => {
			try {
				const configuration = await transport.request<SyncServiceConfiguration>(
					'sync_service_configuration',
				);
				update({ origin: configuration.origin });
				await transport.subscribeSyncReturn?.(() => void returned());
				await refresh();
			} catch (cause) {
				update({ error: message(cause) });
			} finally {
				update({ loading: false });
			}
		})());
	}
	async function openBrowser(signUp = false) {
		update({ error: '' });
		try {
			await transport.request('sync_account_open_browser', { signUp });
		} catch (cause) {
			update({ error: message(cause) });
		}
	}
	async function begin(origin?: string, signUp = false) {
		if (value.busy || value.request) return;
		const attempt = ++generation;
		update({ busy: true, error: '', notice: '' });
		try {
			const request = await transport.request<DeviceSignInInfo>(
				'sync_account_begin',
				{ origin: origin || null },
			);
			if (attempt !== generation) return;
			expiresAt =
				Date.now() + Math.min(900, Math.max(0, request.expiresIn)) * 1000;
			update({ request });
			timer = setTimeout(
				() => void poll(attempt),
				Math.min(5000, Math.max(0, expiresAt - Date.now())),
			);
			await openBrowser(signUp);
		} catch (cause) {
			update({ error: message(cause) });
		} finally {
			update({ busy: false });
		}
	}
	return {
		subscribe: state.subscribe,
		initialize,
		begin,
		cancel,
		openBrowser,
		refresh,
	};
}
