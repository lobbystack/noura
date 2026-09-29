import type {
	ProjectStatus,
	TaskPriority,
	TaskStatus,
} from './generated/index';

/**
 * Accepts a list only when it names every member of `Union`, so a variant
 * added to the Rust enum fails type checking here until the list includes it.
 */
function allOf<Union extends string>() {
	return <const List extends readonly Union[]>(
		list: List &
			([Exclude<Union, List[number]>] extends [never]
				? unknown
				: { missing: Exclude<Union, List[number]> }),
	): List => list;
}

/** Task statuses in board order. */
export const TASK_STATUSES = allOf<TaskStatus>()([
	'todo',
	'in-progress',
	'done',
	'cancelled',
]);
/** Task priorities from lowest to highest. */
export const TASK_PRIORITIES = allOf<TaskPriority>()([
	'low',
	'medium',
	'high',
	'urgent',
]);
/** Project statuses in lifecycle order. */
export const PROJECT_STATUSES = allOf<ProjectStatus>()([
	'planned',
	'active',
	'on-hold',
	'completed',
	'cancelled',
]);

export function isTaskStatus(value: unknown): value is TaskStatus {
	return (TASK_STATUSES as readonly unknown[]).includes(value);
}
export function isTaskPriority(value: unknown): value is TaskPriority {
	return (TASK_PRIORITIES as readonly unknown[]).includes(value);
}
export function isProjectStatus(value: unknown): value is ProjectStatus {
	return (PROJECT_STATUSES as readonly unknown[]).includes(value);
}
