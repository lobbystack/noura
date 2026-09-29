import type {
	CreateNoteInput,
	ManifestUpdateInput as CoreManifestUpdateInput,
	ParsedMarkdown,
	UpdateNoteInput,
	WorkspaceManifest,
	WorkspaceObject,
} from '@noura/shared';

export type {
	CreateNoteInput,
	ParsedMarkdown,
	UpdateNoteInput,
	WorkspaceManifest,
	WorkspaceObject,
};

export type WorkspaceFormatErrorCode =
	| 'invalid_input'
	| 'invalid_object_id'
	| 'object_serialization_failed'
	| 'invalid_manifest'
	| 'invalid_workspace_id'
	| 'workspace_name_required'
	| 'invalid_plugin_id'
	| 'invalid_ignore_pattern'
	| 'unsupported_workspace_version'
	| 'manifest_serialization_failed'
	| 'title_required'
	| 'invalid_timestamp'
	| 'invalid_field'
	| 'invalid_date'
	| 'invalid_project_id'
	| 'invalid_utf8'
	| 'invalid_calendar_date'
	| 'invalid_calendar_range'
	| 'internal_error';

const errorMessages: Record<WorkspaceFormatErrorCode, string> = {
	invalid_input: 'The workspace format input has an invalid shape',
	invalid_object_id: 'The stable ID does not match the object type',
	object_serialization_failed: 'Frontmatter could not be serialized',
	invalid_manifest: '.noura/workspace.yaml is invalid',
	invalid_workspace_id:
		'The workspace ID must be a lowercase stable workspace ID',
	workspace_name_required: 'A workspace name is required',
	invalid_plugin_id:
		'Plugin identifiers use lowercase letters, digits, and hyphens',
	invalid_ignore_pattern: 'Workspace ignore patterns must not be empty',
	unsupported_workspace_version:
		'This workspace format version is not supported',
	manifest_serialization_failed:
		'.noura/workspace.yaml could not be serialized',
	title_required: 'A title is required',
	invalid_timestamp: 'A timestamp must be an RFC 3339 instant',
	invalid_field: 'Task metadata is invalid',
	invalid_date: 'due must use YYYY-MM-DD or RFC 3339 with an explicit offset',
	invalid_project_id: 'Task project references use a stable project ID',
	invalid_utf8: 'The raw Markdown file is not UTF-8',
	invalid_calendar_date:
		'Calendar values must use YYYY-MM-DD or RFC 3339 with an explicit offset',
	invalid_calendar_range: 'The calendar range end must be after its start',
	internal_error: 'The workspace format operation could not be completed',
};

export class WorkspaceFormatError extends Error {
	constructor(public readonly code: WorkspaceFormatErrorCode) {
		super(errorMessages[code]);
		this.name = 'WorkspaceFormatError';
	}
}

type RawWorkspaceFormatBindings = {
	parse_markdown(relativePath: string, bytes: Uint8Array): unknown;
	serialize_object(object: WorkspaceObject): Uint8Array;
	parse_workspace_manifest(bytes: Uint8Array): unknown;
	serialize_workspace_manifest(manifest: WorkspaceManifest): string;
	content_revision(bytes: Uint8Array): string;
	is_valid_object_id(id: string, objectType: string): boolean;
	is_valid_managed_object_path(path: string): boolean;
	create_workspace_manifest(name: string, now: string): unknown;
	update_workspace_manifest(
		manifest: WorkspaceManifest,
		input: ManifestUpdateInput,
		now: string,
	): unknown;
	create_note(input: CreateNoteInput): unknown;
	update_note(note: WorkspaceObject, input: UpdateNoteInput): unknown;
	create_task(input: CreateTaskInput): unknown;
	update_task(task: WorkspaceObject, input: UpdateTaskInput): unknown;
	create_project(input: CreateProjectInput): unknown;
	update_project(project: WorkspaceObject, input: UpdateProjectInput): unknown;
	merge_text(base: string, local: string, external: string): string | undefined;
	merge_markdown_body(
		base: string,
		local: string,
		external: string,
	): string | undefined;
	merge_managed_draft(
		input: ManagedDraftMergeInput,
		canonical: WorkspaceObject,
	): unknown;
	read_raw_text(bytes: Uint8Array): unknown;
	compose_raw_text(original: Uint8Array, body: string): Uint8Array;
	raw_history_segment(relativePath: string): string;
	select_calendar_entries(
		entries: CalendarEntry[],
		start: string,
		end: string,
	): unknown;
};

type RawWorkspaceFormatModule = RawWorkspaceFormatBindings & {
	default(input?: {
		module_or_path: RequestInfo | URL | Response | BufferSource;
	}): Promise<unknown>;
};

export type WorkspaceFormat = {
	parseMarkdown(relativePath: string, bytes: Uint8Array): ParsedMarkdown;
	serializeObject(object: WorkspaceObject): Uint8Array;
	parseWorkspaceManifest(bytes: Uint8Array): WorkspaceManifest;
	serializeWorkspaceManifest(manifest: WorkspaceManifest): string;
	contentRevision(bytes: Uint8Array): string;
	isValidObjectId(id: string, objectType: string): boolean;
	isValidManagedObjectPath(path: string): boolean;
	createWorkspaceManifest(name: string, now: string): WorkspaceManifest;
	updateWorkspaceManifest(
		manifest: WorkspaceManifest,
		input: ManifestUpdateInput,
		now: string,
	): WorkspaceManifest;
	createNote(input: CreateNoteInput): WorkspaceObject;
	updateNote(note: WorkspaceObject, input: UpdateNoteInput): WorkspaceObject;
	createTask(input: CreateTaskInput): WorkspaceObject;
	updateTask(task: WorkspaceObject, input: UpdateTaskInput): WorkspaceObject;
	createProject(input: CreateProjectInput): WorkspaceObject;
	updateProject(
		project: WorkspaceObject,
		input: UpdateProjectInput,
	): WorkspaceObject;
	/** Line merge. Returns null when both sides changed the same lines. */
	mergeText(base: string, local: string, external: string): string | null;
	/** Line merge that also refuses Markdown needing manual review. */
	mergeMarkdownBody(
		base: string,
		local: string,
		external: string,
	): string | null;
	/** Field-by-field managed draft merge. Returns null on conflict. */
	mergeManagedDraft(
		input: ManagedDraftMergeInput,
		canonical: WorkspaceObject,
	): WorkspaceObject | null;
	readRawText(bytes: Uint8Array): RawText;
	/** Encodes LF text with the BOM and line endings of `original`. */
	composeRawText(original: Uint8Array, body: string): Uint8Array;
	rawHistorySegment(relativePath: string): string;
	/** Keeps entries overlapping [start, end), in native calendar order. */
	selectCalendarEntries(
		entries: CalendarEntry[],
		start: string,
		end: string,
	): CalendarEntry[];
};

export type ManagedDraftMergeInput = {
	baseTitle: string;
	localTitle: string;
	baseBody: string;
	localBody: string;
	baseProperties: Record<string, unknown>;
	localProperties: Record<string, unknown>;
};

export type RawText = {
	/** The file's text with every CRLF turned into LF. */
	body: string;
	/** Whether most lines end in CRLF. */
	usesCrlf: boolean;
	hasBom: boolean;
};

export type CalendarEntry = {
	sourceId: string;
	sourceType: string;
	title: string;
	property: string;
	start: string;
	end: string | null;
	allDay: boolean;
	revision: string;
};

export type CreateTaskInput = CreateNoteInput;
export type UpdateTaskInput = UpdateNoteInput;
export type CreateProjectInput = CreateNoteInput;
export type UpdateProjectInput = UpdateNoteInput;

/** The optimistic `expectedUpdated` check belongs to the caller, not the format. */
export type ManifestUpdateInput = Omit<
	CoreManifestUpdateInput,
	'expectedUpdated'
>;

function asFormatError(error: unknown): WorkspaceFormatError {
	if (
		typeof error === 'object' &&
		error !== null &&
		'code' in error &&
		typeof error.code === 'string' &&
		error.code in errorMessages
	) {
		return new WorkspaceFormatError(error.code as WorkspaceFormatErrorCode);
	}
	return new WorkspaceFormatError('internal_error');
}

function call<T>(operation: () => T): T {
	try {
		return operation();
	} catch (error) {
		throw asFormatError(error);
	}
}

function createWorkspaceFormat(
	bindings: RawWorkspaceFormatBindings,
): WorkspaceFormat {
	return {
		parseMarkdown: (relativePath, bytes) =>
			call(
				() => bindings.parse_markdown(relativePath, bytes) as ParsedMarkdown,
			),
		serializeObject: (object) =>
			call(() => new Uint8Array(bindings.serialize_object(object))),
		parseWorkspaceManifest: (bytes) =>
			call(() => bindings.parse_workspace_manifest(bytes) as WorkspaceManifest),
		serializeWorkspaceManifest: (manifest) =>
			call(() => bindings.serialize_workspace_manifest(manifest)),
		contentRevision: (bytes) => call(() => bindings.content_revision(bytes)),
		isValidObjectId: (id, objectType) =>
			call(() => bindings.is_valid_object_id(id, objectType)),
		isValidManagedObjectPath: (path) =>
			call(() => bindings.is_valid_managed_object_path(path)),
		createWorkspaceManifest: (name, now) =>
			call(
				() =>
					bindings.create_workspace_manifest(name, now) as WorkspaceManifest,
			),
		updateWorkspaceManifest: (manifest, input, now) =>
			call(
				() =>
					bindings.update_workspace_manifest(
						manifest,
						input,
						now,
					) as WorkspaceManifest,
			),
		createNote: (input) =>
			call(() => bindings.create_note(input) as WorkspaceObject),
		updateNote: (note, input) =>
			call(() => bindings.update_note(note, input) as WorkspaceObject),
		createTask: (input) =>
			call(() => bindings.create_task(input) as WorkspaceObject),
		updateTask: (task, input) =>
			call(() => bindings.update_task(task, input) as WorkspaceObject),
		createProject: (input) =>
			call(() => bindings.create_project(input) as WorkspaceObject),
		updateProject: (project, input) =>
			call(() => bindings.update_project(project, input) as WorkspaceObject),
		mergeText: (base, local, external) =>
			call(() => bindings.merge_text(base, local, external) ?? null),
		mergeMarkdownBody: (base, local, external) =>
			call(() => bindings.merge_markdown_body(base, local, external) ?? null),
		mergeManagedDraft: (input, canonical) =>
			call(
				() =>
					bindings.merge_managed_draft(
						input,
						canonical,
					) as WorkspaceObject | null,
			),
		readRawText: (bytes) =>
			call(() => bindings.read_raw_text(bytes) as RawText),
		composeRawText: (original, body) =>
			call(() => new Uint8Array(bindings.compose_raw_text(original, body))),
		rawHistorySegment: (relativePath) =>
			call(() => bindings.raw_history_segment(relativePath)),
		selectCalendarEntries: (entries, start, end) =>
			call(
				() =>
					bindings.select_calendar_entries(
						entries,
						start,
						end,
					) as CalendarEntry[],
			),
	};
}

/** Loads the generated wasm-bindgen module for use in a browser worker. */
export async function loadWorkspaceFormat(
	moduleUrl = new URL('../wasm/workspace_format_wasm.js', import.meta.url).href,
	wasmInput?: RequestInfo | URL | Response | BufferSource,
): Promise<WorkspaceFormat> {
	const module = (await import(
		/* @vite-ignore */ moduleUrl
	)) as RawWorkspaceFormatModule;
	await module.default(
		wasmInput === undefined ? undefined : { module_or_path: wasmInput },
	);
	return createWorkspaceFormat(module);
}
