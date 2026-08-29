export type ObjectType = 'note' | 'task' | 'project' | (string & {});
export type WorkspacePhase =
	| 'idle'
	| 'opening'
	| 'scanning'
	| 'indexing'
	| 'ready'
	| 'rebuilding'
	| 'failed';
export type EventSource = 'application' | 'external' | 'reconciliation' | 'mcp';
export type { AiInvokeInput } from './generated/AiInvokeInput';
export type { AiMessage } from './generated/AiMessage';
export type { AiProviderConfig } from './generated/AiProviderConfig';
export type { AiResponse } from './generated/AiResponse';
export type { FolderEntry } from './generated/FolderEntry';
export type { UnmanagedFile } from './generated/UnmanagedFile';
export type { WorkspaceEntry } from './generated/WorkspaceEntry';
export type { WorkspaceEntryKind } from './generated/WorkspaceEntryKind';

export interface WorkspaceObject {
	id: string;
	type: ObjectType;
	title: string;
	body: string;
	relativePath: string;
	revision: string;
	created: string | null;
	updated: string | null;
	properties: Record<string, unknown>;
}

export interface WorkspaceMetadata {
	id: string;
	name: string;
	rootPath: string;
	formatVersion: number;
}
export interface WorkspaceState {
	phase: WorkspacePhase;
	workspaceId?: string | null;
	rootPath?: string | null;
	indexedFiles: number;
	diagnostics: Diagnostic[];
}
export interface Diagnostic {
	code: string;
	message: string;
	relativePath?: string | null;
	objectId?: string | null;
}
export interface CoreWarning {
	code: string;
	message: string;
}
export interface MutationResult<T> {
	value: T;
	revision: string;
	durability: 'committed';
	indexStatus: 'updated' | 'repair-pending';
	warnings: CoreWarning[];
}
export interface CoreEvent<T = unknown> {
	eventId: string;
	type: string;
	workspaceId: string;
	occurredAt: string;
	source: EventSource;
	payload: T;
}
export interface CoreError {
	code: string;
	category:
		| 'validation'
		| 'filesystem'
		| 'permission'
		| 'parse'
		| 'identity'
		| 'index'
		| 'conflict'
		| 'credential'
		| 'provider'
		| 'transient';
	message: string;
	retryable: boolean;
	operation: string;
	workspaceId?: string | null;
	objectId?: string | null;
	path?: string | null;
	details?: Record<string, unknown> | null;
}

export interface ObjectQuery {
	type?: ObjectType;
	project?: string;
	status?: string;
	priority?: string;
	pathPrefix?: string;
}
export interface ObjectPatch {
	title?: string;
	body?: string;
	properties?: Record<string, unknown>;
	removeProperties?: string[];
	expectedRevision: string;
}
export interface SearchInput {
	query: string;
	type?: ObjectType;
	pathPrefix?: string;
	limit?: number;
}
export interface SearchResult {
	objectId: string | null;
	objectType: ObjectType | null;
	relativePath: string;
	title: string;
	snippet: string;
	highlights: Array<{ start: number; end: number }>;
	score: number;
	revision: string;
}
export interface CalendarEntry {
	sourceId: string;
	sourceType: ObjectType;
	title: string;
	property: 'due' | 'date' | 'start';
	start: string;
	end: string | null;
	allDay: boolean;
	revision: string;
}

export type TaskStatus = 'todo' | 'in-progress' | 'done' | 'cancelled';
export type TaskPriority = 'low' | 'medium' | 'high' | 'urgent';
export type ProjectStatus =
	'planned' | 'active' | 'on-hold' | 'completed' | 'cancelled';
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
