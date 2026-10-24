// Runs the built plugin against the mock host, which answers as Noura
// does and refuses what plugin.toml doesn't declare.
import { createMockHost } from '@noura/mock-host';
import { expect, test } from 'vitest';

test('the page counts the files in the workspace and the presses', async () => {
	const host = await createMockHost({ files: { 'Plan.md': 'plan', 'Notes/Idea.md': 'idea' } });

	expect(await host.pages()).toEqual([{ id: 'main', title: '{{name}}' }]);
	expect(await host.texts('main')).toEqual([
		'Your workspace holds 2 files.',
		'You pressed the button 0 times.',
	]);

	await host.press('main', 'press');

	expect(await host.texts('main')).toEqual([
		'Your workspace holds 2 files.',
		'You pressed the button 1 time.',
	]);
});
