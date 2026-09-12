export type WorkspaceManifest = {
	id: string;
	format_version: 1;
	name: string;
	created: string;
	updated: string;
	enabled_plugins: string[];
	ignore: string[];
};

export type WorkspaceObject = {
	id: string;
	type: string;
	title: string;
	body: string;
	relativePath: string;
	revision: string;
	created: string | null;
	updated: string | null;
	properties: Record<string, unknown>;
};

export type ParsedMarkdown =
	| ({ kind: 'managed' } & WorkspaceObject)
	| {
			kind: 'unmanaged';
			title: string;
			body: string;
			frontmatter: Record<string, unknown> | null;
	  }
	| { kind: 'malformed'; title: string; body: string; error: string };

export type WorkspaceFormatErrorCode =
	| 'invalid_input'
	| 'invalid_object_id'
	| 'object_serialization_failed'
	| 'invalid_manifest'
	| 'invalid_workspace_id'
	| 'workspace_name_required'
	| 'invalid_plugin_id'
	| 'unsupported_workspace_version'
	| 'manifest_serialization_failed'
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
	unsupported_workspace_version:
		'This workspace format version is not supported',
	manifest_serialization_failed:
		'.noura/workspace.yaml could not be serialized',
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
};

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
