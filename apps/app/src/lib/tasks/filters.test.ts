import { describe, expect, test } from 'bun:test';
import {
	filterTasks,
	isDone,
	taskInFolder,
	taskIsToday,
	taskIsUpcoming,
	type TaskView,
} from './filters';
import type { Task } from '@noura/workspace';

function task(overrides: Partial<Task> = {}): Task {
	return {
		id: 'task_default',
		type: 'task',
		title: 'Task',
		body: '',
		relativePath: 'tasks/task.md',
		revision: 'rev',
		created: null,
		updated: null,
		properties: {} as Task['properties'],
		...overrides,
	} as unknown as Task;
}

const now = new Date('2026-09-01T12:00:00');

describe('task placement', () => {
	test('overdue and due-today tasks count as today', () => {
		const overdue = task({
			id: 'task_overdue',
			properties: { status: 'todo', due: '2026-08-31' } as Task['properties'],
		});
		const today = task({
			id: 'task_today',
			properties: { status: 'todo', due: '2026-09-01' } as Task['properties'],
		});
		expect(taskIsToday(overdue, now)).toBe(true);
		expect(taskIsToday(today, now)).toBe(true);
		expect(taskIsUpcoming(today, now)).toBe(false);
	});

	test('done tasks never appear in Today', () => {
		const done = task({
			id: 'task_done',
			properties: { status: 'done', due: '2026-09-01' } as Task['properties'],
		});
		expect(taskIsToday(done, now)).toBe(false);
		expect(isDone(done)).toBe(true);
	});

	test('tomorrow belongs to Upcoming', () => {
		const tomorrow = task({
			id: 'task_tomorrow',
			properties: { status: 'todo', due: '2026-09-02' } as Task['properties'],
		});
		expect(taskIsUpcoming(tomorrow, now)).toBe(true);
		expect(taskIsToday(tomorrow, now)).toBe(false);
	});

	test('folder views scope by path prefix', () => {
		const deep = task({
			id: 'task_deep',
			relativePath: 'projects/alpha/task.md',
		});
		const sibling = task({
			id: 'task_sibling',
			relativePath: 'tasks/elsewhere.md',
		});
		expect(taskInFolder(deep, 'projects/alpha')).toBe(true);
		expect(taskInFolder(sibling, 'projects/alpha')).toBe(false);
	});
});

describe('filtered views', () => {
	const tasks = [
		task({
			id: 'task_overdue',
			title: 'Overdue',
			properties: { status: 'todo', due: '2026-08-31' } as Task['properties'],
		}),
		task({
			id: 'task_future',
			title: 'Future',
			properties: { status: 'todo', due: '2026-09-10' } as Task['properties'],
		}),
		task({
			id: 'task_done',
			title: 'Done',
			properties: { status: 'done' } as Task['properties'],
		}),
		task({
			id: 'task_open',
			title: 'Open',
			properties: { status: 'todo' } as Task['properties'],
		}),
	];

	test('today view hides done and later items', () => {
		const result = filterTasks(tasks, { mode: 'today' }, now);
		expect(result.map((entry) => entry.id)).toEqual(['task_overdue']);
	});

	test('upcoming only includes later dates', () => {
		const result = filterTasks(tasks, { mode: 'upcoming' }, now);
		expect(result.map((t) => t.id)).toEqual(['task_future']);
	});

	test('completed only includes done', () => {
		const result = filterTasks(tasks, { mode: 'completed' }, now);
		expect(result.map((t) => t.id)).toEqual(['task_done']);
	});
});
