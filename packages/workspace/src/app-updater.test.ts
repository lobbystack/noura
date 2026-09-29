import { describe, expect, test } from 'bun:test';
import {
	createAppUpdater,
	type AppUpdaterAdapter,
	type PendingAppUpdate,
} from './app-updater';

function fakeAdapter(update: Partial<PendingAppUpdate> | null) {
	const calls: string[] = [];
	const adapter: AppUpdaterAdapter = {
		check: async () => {
			calls.push('check');
			if (!update) return null;
			return {
				version: '0.2.0',
				download: async () => {
					calls.push('download');
				},
				install: async () => {
					calls.push('install');
				},
				...update,
			};
		},
		restart: async () => {
			calls.push('restart');
		},
	};
	return { adapter, calls };
}

describe('app updater', () => {
	test('stays idle when no newer release exists', async () => {
		const { adapter, calls } = fakeAdapter(null);
		const updater = createAppUpdater(adapter, { flush: async () => true });

		const state = await updater.check();
		expect(state).toMatchObject({
			status: 'idle',
			lastCheck: { outcome: 'current' },
		});
		expect(calls).toEqual(['check']);
	});

	test('downloads in the background before reporting ready', async () => {
		const { adapter, calls } = fakeAdapter({ notes: 'Fixes' });
		const states: string[] = [];
		const updater = createAppUpdater(adapter, {
			flush: async () => true,
			onChange: (state) => states.push(state.status),
		});

		await expect(updater.check()).resolves.toEqual({
			status: 'ready',
			version: '0.2.0',
			notes: 'Fixes',
		});
		expect(states).toEqual(['checking', 'downloading', 'ready']);
		expect(calls).toEqual(['check', 'download']);
	});

	test('returns to idle when the release host is unreachable', async () => {
		const updater = createAppUpdater(
			{
				check: async () => {
					throw new Error('offline');
				},
				restart: async () => {},
			},
			{ flush: async () => true },
		);

		await expect(updater.check()).resolves.toMatchObject({
			status: 'idle',
			lastCheck: { outcome: 'failed' },
		});
	});

	test('returns to idle when a download fails signature verification', async () => {
		const { adapter } = fakeAdapter({
			download: async () => {
				throw new Error('signature mismatch');
			},
		});
		const updater = createAppUpdater(adapter, { flush: async () => true });

		await expect(updater.check()).resolves.toMatchObject({
			status: 'idle',
			lastCheck: { outcome: 'failed' },
		});
		await expect(updater.installAndRestart()).resolves.toMatchObject({
			status: 'idle',
		});
	});

	test('flushes drafts before installing and restarting', async () => {
		const { adapter, calls } = fakeAdapter({});
		const updater = createAppUpdater(adapter, {
			flush: async () => {
				calls.push('flush');
				return true;
			},
		});

		await updater.check();
		await updater.installAndRestart();
		expect(calls).toEqual(['check', 'download', 'flush', 'install', 'restart']);
	});

	test('keeps the running version when drafts fail to flush', async () => {
		const { adapter, calls } = fakeAdapter({});
		const updater = createAppUpdater(adapter, { flush: async () => false });

		await updater.check();
		const state = await updater.installAndRestart();
		expect(state.status).toBe('ready');
		expect(state).toHaveProperty('error');
		expect(calls).not.toContain('install');
		expect(calls).not.toContain('restart');
	});

	test('reports an install failure without restarting', async () => {
		const { adapter, calls } = fakeAdapter({
			install: async () => {
				throw new Error('disk full');
			},
		});
		const updater = createAppUpdater(adapter, { flush: async () => true });

		await updater.check();
		const state = await updater.installAndRestart();
		expect(state.status).toBe('ready');
		expect(state).toHaveProperty('error');
		expect(calls).not.toContain('restart');
	});
});
