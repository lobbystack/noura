import type { Task, TaskPriority } from '@noura/workspace';

export type TaskViewId = 'today' | 'upcoming' | 'all' | 'completed' | 'folder';

export interface TaskView {
	mode: TaskViewId;
	folderPath?: string;
}

function dueDate(task: Task): string | null {
	const value = task.properties?.due;
	return typeof value === 'string' && value.length > 0 ? value : null;
}

function endOfDay(date: Date): number {
	return new Date(
		date.getFullYear(),
		date.getMonth(),
		date.getDate(),
		23,
		59,
		59,
		999,
	).getTime();
}

function parseDue(due: string): number | null {
	const parsed = new Date(due.length === 10 ? due + 'T23:59:59' : due);
	return Number.isNaN(parsed.getTime()) ? null : parsed.getTime();
}

export function isDone(task: Task): boolean {
	return task.properties?.status === 'done';
}

export function taskIsToday(task: Task, now: Date): boolean {
	if (isDone(task)) return false;
	const due = dueDate(task);
	if (due === null) return false;
	const time = parseDue(due);
	return time !== null && time <= endOfDay(now);
}

export function taskIsUpcoming(task: Task, now: Date): boolean {
	if (isDone(task)) return false;
	const due = dueDate(task);
	if (due === null) return false;
	const time = parseDue(due);
	return time !== null && time > endOfDay(now);
}

export function taskInFolder(task: Task, folderPath: string): boolean {
	const normalized = folderPath.replace(/\/+$/, '');
	return task.relativePath.startsWith(normalized + '/');
}

export function filterTasks(tasks: Task[], view: TaskView, now: Date): Task[] {
	let selected = tasks;
	if (view.mode === 'today') {
		selected = tasks.filter((entry) => taskIsToday(entry, now));
	} else if (view.mode === 'upcoming') {
		selected = tasks.filter((entry) => taskIsUpcoming(entry, now));
	} else if (view.mode === 'completed') {
		selected = tasks.filter(isDone);
	} else if (view.mode === 'folder') {
		selected = tasks.filter((entry) =>
			taskInFolder(entry, view.folderPath ?? ''),
		);
	}
	return orderTasks(selected);
}

const PRIORITY_WEIGHT: Record<TaskPriority, number> = {
	urgent: 0,
	high: 1,
	medium: 2,
	low: 3,
};

export function orderTasks(tasks: Task[]): Task[] {
	return [...tasks].sort((left, right) => {
		const doneLeft = isDone(left) ? 1 : 0;
		const doneRight = isDone(right) ? 1 : 0;
		if (doneLeft !== doneRight) return doneLeft - doneRight;
		const dueLeft = dueDate(left);
		const dueRight = dueDate(right);
		if (dueLeft && dueRight && dueLeft !== dueRight) {
			return dueLeft < dueRight ? -1 : 1;
		}
		if (dueLeft && !dueRight) return -1;
		if (!dueLeft && dueRight) return 1;
		const leftPriority =
			PRIORITY_WEIGHT[left.properties?.priority ?? 'medium'] ?? 2;
		const rightPriority =
			PRIORITY_WEIGHT[right.properties?.priority ?? 'medium'] ?? 2;
		if (leftPriority !== rightPriority) return leftPriority - rightPriority;
		return (left.updated ?? '').localeCompare(right.updated ?? '');
	});
}
