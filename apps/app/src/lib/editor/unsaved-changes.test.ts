import { describe, expect, test } from 'bun:test';
import {
	blockedDrafts,
	describeBlockedDrafts,
	discardPendingDrafts,
	flushPendingDrafts,
	registerPendingDraft,
} from './pending-drafts.svelte';

describe('blocked drafts', () => {
	test('report which editor failed, why, and discard it on request', async () => {
		let dirty = true;
		const unregister = registerPendingDraft(
			'note',
			async () => false,
			() => dirty,
			{
				label: () => 'Plan',
				problem: () => 'The disk is full.',
				discard: () => (dirty = false),
			},
		);
		expect(await flushPendingDrafts()).toBe(false);
		expect(blockedDrafts()).toEqual([
			{ label: 'Plan', problem: 'The disk is full.', canDiscard: true },
		]);
		discardPendingDrafts();
		expect(blockedDrafts()).toEqual([]);
		expect(await flushPendingDrafts()).toBe(true);
		unregister();
	});

	test('name the file and the problem in plain words', () => {
		expect(
			describeBlockedDrafts([
				{ label: 'Plan', problem: 'The disk is full.', canDiscard: true },
			]),
		).toEqual({
			title: 'Couldn’t save “Plan”',
			description: 'The disk is full. Your changes are still in the editor.',
			canDiscard: true,
		});
		expect(describeBlockedDrafts([]).title).toBe('Couldn’t save your changes');
	});
});
