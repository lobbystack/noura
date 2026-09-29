/**
 * Every DTO that crosses the native boundary is generated from Rust by
 * ts-rs (`bun run bindings:generate`). This module re-exports those types
 * and adds only TypeScript refinements derived from them; it must not
 * redeclare a generated shape.
 */
export type * from './generated/index';
export type { SequencedOperation, SyncPage, SyncTransport } from './sync';

import type {
	CoreError,
	ProjectStatus,
	TaskPriority,
	TaskStatus,
	WorkspaceObject,
} from './generated/index';

/**
 * An object `type` value. The format allows any valid type name; the
 * literals only help editors suggest the built-in ones.
 */
export type ObjectType = 'note' | 'task' | 'project' | (string & {});

export function isCoreError(value: unknown): value is CoreError {
	if (!value || typeof value !== 'object') return false;
	const candidate = value as Record<string, unknown>;
	return (
		typeof candidate.code === 'string' &&
		typeof candidate.category === 'string' &&
		typeof candidate.message === 'string' &&
		typeof candidate.retryable === 'boolean' &&
		typeof candidate.operation === 'string'
	);
}

/** A Task as the native core normalizes it: status and priority are always set. */
export type Task = WorkspaceObject & {
	type: 'task';
	properties: WorkspaceObject['properties'] & {
		status: TaskStatus;
		priority: TaskPriority;
		due?: string;
		project?: string;
		kanban_order?: string;
	};
};
export type Note = WorkspaceObject & { type: 'note' };
export type Project = WorkspaceObject & {
	type: 'project';
	properties: WorkspaceObject['properties'] & { status: ProjectStatus };
};
