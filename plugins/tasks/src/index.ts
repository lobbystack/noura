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
/**
 * Fractional order key for a card positioned between two neighbors.
 * `after` is the order key of the task visually above (the lower key)
 * and `before` the order key of the task visually below (the higher
 * key); either may be omitted to prepend or append at a column edge.
 */
export function nextKanbanOrder(after?: string, before?: string) {
	return generateKeyBetween(after ?? null, before ?? null);
}
interface CreateTaskInput {
	title?: unknown;
	properties?: Record<string, unknown>;
}
interface CompleteTaskInput {
	id?: unknown;
	expectedRevision?: unknown;
}
const commandDisposers = new WeakMap<object, Array<() => void>>();
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
	activate(context) {
		const disposers: Array<() => void> = [];
		disposers.push(
			context.commands.register({
				id: 'tasks.create',
				title: 'Create task',
				async execute(input) {
					const { title, properties } = (input ?? {}) as CreateTaskInput;
					if (typeof title !== 'string' || title.trim().length === 0) {
						throw new Error('A task title is required');
					}
					const result = await context.objects.create({
						type: 'task',
						title,
						properties,
					});
					return result.value;
				},
			}),
		);
		disposers.push(
			context.commands.register({
				id: 'tasks.complete',
				title: 'Complete task',
				async execute(input) {
					const { id, expectedRevision } = (input ?? {}) as CompleteTaskInput;
					if (typeof id !== 'string' || typeof expectedRevision !== 'string') {
						throw new Error(
							'Completing a task needs its stable ID and expected revision',
						);
					}
					const result = await context.objects.update(id, {
						expectedRevision,
						properties: { status: 'done' },
					});
					return result.value;
				},
			}),
		);
		commandDisposers.set(context, disposers);
	},
	deactivate(context) {
		for (const dispose of commandDisposers.get(context)?.splice(0) ?? []) {
			dispose();
		}
	},
});
