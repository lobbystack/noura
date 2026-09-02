import { expect, test } from 'bun:test';
import { nextKanbanOrder } from './index';

test('fractional Kanban order fits between adjacent cards', () => {
	const value = nextKanbanOrder('a0', 'a1');
	expect(value > 'a0' && value < 'a1').toBe(true);
});

test('an omitted before key appends after the above card', () => {
	const value = nextKanbanOrder('a1');
	expect(value > 'a1').toBe(true);
});

test('an omitted after key prepends before the below card', () => {
	const value = nextKanbanOrder(undefined, 'a1');
	expect(value < 'a1').toBe(true);
});
