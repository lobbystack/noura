import { generateKeyBetween } from 'fractional-indexing';
import { z } from 'zod';
import { definePlugin } from '@noura/plugin-sdk';
import type { Task, TaskStatus } from '@noura/shared';
export const taskPropertiesSchema = z
	.object({
		status: z
			.enum(['todo', 'in-progress', 'done', 'cancelled'])
			.default('todo'),
		priority: z.enum(['low', 'medium', 'high', 'urgent']).default('medium'),
		due: z.string().optional(),
		project: z.string().optional(),
		kanban_order: z.string().optional(),
	})
	.passthrough();
export const taskStatuses: TaskStatus[] = [
	'todo',
	'in-progress',
	'done',
	'cancelled',
];
export function projectKanban(tasks: Task[]) {
	return {
		groups: taskStatuses.map((status) => ({
			id: status,
			title: status,
			items: tasks
				.filter((task) => task.properties.status === status)
				.sort((left, right) =>
					(left.properties.kanban_order ?? left.id).localeCompare(
						right.properties.kanban_order ?? right.id,
					),
				),
		})),
	};
}
export function nextKanbanOrder(before?: string, after?: string) {
	return generateKeyBetween(before ?? null, after ?? null);
}
export default definePlugin({
	manifest: {
		id: 'tasks',
		name: 'Tasks',
		version: '0.1.0',
		capabilities: [
			'workspace.objects',
			'workspace.search',
			'workspace.commands',
			'workspace.events',
		],
	},
	activate() {},
});
