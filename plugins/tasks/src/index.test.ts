import { expect, test } from 'bun:test';
import { nextKanbanOrder } from './index';

test('fractional Kanban order fits between adjacent cards', () => {
	const value = nextKanbanOrder('a0', 'a1');
	expect(value > 'a0' && value < 'a1').toBe(true);
});
