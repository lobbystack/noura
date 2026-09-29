import { describe, expect, test } from 'bun:test';
import { visibleSettingsSections } from './settings-sections';

const ids = (
	enabled: string[],
	signedIn = false,
	desktop = true,
	web = false,
): string[] =>
	visibleSettingsSections({
		desktop,
		web,
		enabledPluginIds: new Set(enabled),
		signedIn,
	}).map((section) => section.id);

describe('settings sections', () => {
	test('sync sections appear only with the sync plugin', () => {
		const off = ids(['notes']);
		expect(off).not.toContain('account');
		expect(off).not.toContain('sync');
		expect(off).not.toContain('people');
		const on = ids(['notes', 'sync']);
		expect(on).toContain('account');
		expect(on).toContain('sync');
		expect(on).toContain('people');
	});

	test('a signed-in device keeps the account section to sign out', () => {
		const signedIn = ids(['notes'], true);
		expect(signedIn).toContain('account');
		expect(signedIn).not.toContain('sync');
		expect(signedIn).not.toContain('people');
	});

	test('desktop-only sections stay off other platforms', () => {
		expect(ids([], false, true)).toContain('external-tools');
		expect(ids([], false, false)).not.toContain('external-tools');
	});

	test('keeps registry order', () => {
		expect(ids(['sync'])).toEqual([
			'general',
			'editor',
			'shortcuts',
			'account',
			'workspace',
			'plugins',
			'ai',
			'external-tools',
			'sync',
			'people',
			'files',
			'about',
		]);
	});

	test('the browser shows its storage section and hides native ones', () => {
		expect(ids(['sync'], true, false, true)).toEqual([
			'general',
			'editor',
			'shortcuts',
			'storage',
			'workspace',
			'plugins',
			'sync',
			'about',
		]);
	});
});
