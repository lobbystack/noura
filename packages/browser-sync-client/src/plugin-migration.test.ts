import { describe, expect, test } from 'bun:test';
import {
	migrateBrowserSyncPlugin,
	type BrowserSyncPluginMigration,
} from './plugin-migration';

function harness(options: {
	binding: boolean;
	enabled: string[];
	marker?: boolean;
}) {
	const state = {
		enabled: options.enabled,
		updated: '1',
		marker: options.marker ?? false,
		writes: 0,
	};
	const migration: BrowserSyncPluginMigration = {
		hasBinding: async () => options.binding,
		marker: {
			exists: async () => state.marker,
			write: async () => {
				state.marker = true;
			},
		},
		registry: {
			read: async () => ({
				enabledPluginIds: state.enabled,
				updated: state.updated,
			}),
			setEnabled: async (id, enabled, expected) => {
				if (expected !== state.updated) throw new Error('manifest_conflict');
				state.enabled = enabled
					? [...state.enabled, id]
					: state.enabled.filter((value) => value !== id);
				state.updated = String(Number(state.updated) + 1);
				state.writes++;
			},
		},
	};
	return { migration, state };
}

describe('browser sync plugin migration', () => {
	test('a workspace without a binding stays off and unmarked', async () => {
		const { migration, state } = harness({
			binding: false,
			enabled: ['notes'],
		});
		expect(await migrateBrowserSyncPlugin(migration)).toBe(false);
		expect(state.enabled).toEqual(['notes']);
		expect(state.marker).toBe(false);
	});

	test('a bound workspace turns sync on once', async () => {
		const { migration, state } = harness({
			binding: true,
			enabled: ['future-plugin', 'notes'],
		});
		expect(await migrateBrowserSyncPlugin(migration)).toBe(true);
		expect(state.enabled).toEqual(['future-plugin', 'notes', 'sync']);
		expect(state.marker).toBe(true);
		// A later "off" survives the next open.
		state.enabled = ['future-plugin', 'notes'];
		expect(await migrateBrowserSyncPlugin(migration)).toBe(false);
		expect(state.enabled).toEqual(['future-plugin', 'notes']);
		expect(state.writes).toBe(1);
	});

	test('an interrupted migration only writes the marker', async () => {
		const { migration, state } = harness({
			binding: true,
			enabled: ['notes', 'sync'],
		});
		expect(await migrateBrowserSyncPlugin(migration)).toBe(false);
		expect(state.writes).toBe(0);
		expect(state.marker).toBe(true);
	});
});
