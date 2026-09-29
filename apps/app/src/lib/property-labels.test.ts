import { expect, test } from 'bun:test';
import {
	PROJECT_STATUSES,
	TASK_PRIORITIES,
	TASK_STATUSES,
} from '@noura/shared';
import { choiceLabel } from './property-labels';

test('statuses and priorities read as words', () => {
	expect(TASK_STATUSES.map(choiceLabel)).toEqual([
		'To do',
		'In progress',
		'Done',
		'Cancelled',
	]);
	expect(TASK_PRIORITIES.map(choiceLabel)).toEqual([
		'Low',
		'Medium',
		'High',
		'Urgent',
	]);
	expect(PROJECT_STATUSES.map(choiceLabel)).toEqual([
		'Planned',
		'Active',
		'On hold',
		'Completed',
		'Cancelled',
	]);
	expect(choiceLabel('some-new-value')).toBe('Some new value');
});
