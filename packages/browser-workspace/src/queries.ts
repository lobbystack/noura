/**
 * Read models over the canonical files of a browser workspace.
 *
 * The native engine answers these queries from its SQLite index. The browser
 * has no index, so each function below reproduces the native query over the
 * parsed files: the same filters, the same order and the same limits. Shared
 * fixtures under `docs/workspace-format/fixtures/queries-v1.json` hold both
 * implementations to the same answers.
 */
import {
	isLiveWorkspaceObjectPath,
	type RebuildResult,
	type RebuiltFile,
	type RebuiltManagedFile,
} from '@noura/browser-storage';
import type {
	CalendarEntry,
	Diagnostic,
	FolderEntry,
	ObjectSummary,
	ObjectSummaryQuery,
	SearchResult,
	UnmanagedFile,
	WorkspaceEntry,
	WorkspaceObject,
} from '@noura/shared';

const textEncoder = new TextEncoder();

/** Orders strings like Rust and SQLite do: by UTF-8 bytes (code points). */
export function compareBytes(left: string, right: string): number {
	if (left === right) return 0;
	const length = Math.min(left.length, right.length);
	for (let index = 0; index < length; index += 1) {
		const a = left.codePointAt(index)!;
		const b = right.codePointAt(index)!;
		if (a !== b) return a < b ? -1 : 1;
		if (a > 0xffff) index += 1;
	}
	return left.length < right.length ? -1 : left.length > right.length ? 1 : 0;
}

function utf8Length(value: string): number {
	return textEncoder.encode(value).byteLength;
}

/** A property value as the native index stores it for filtering. */
export function propertyText(value: unknown): string {
	return typeof value === 'string' ? value : JSON.stringify(value);
}

function stringProperty(
	object: Pick<WorkspaceObject, 'properties'>,
	name: string,
): string | null {
	const value = object.properties[name];
	return typeof value === 'string' ? value : null;
}

function baseName(path: string): string {
	return path.slice(path.lastIndexOf('/') + 1);
}

/** Whether the file browser and the index show this path. */
export function isVisiblePath(path: string): boolean {
	return isLiveWorkspaceObjectPath(path);
}

function liveMarkdown(rebuilt: RebuildResult): RebuiltFile[] {
	return rebuilt.entries.filter(
		(entry) => entry.parsed !== null && isVisiblePath(entry.path),
	);
}

/** Objects whose stable ID exactly one file uses. */
export function uniqueObjects(rebuilt: RebuildResult): RebuiltManagedFile[] {
	const duplicated = new Set(rebuilt.duplicates.map((value) => value.id));
	return rebuilt.managed.filter((object) => !duplicated.has(object.id));
}

/** `files_list`: every visible file and folder with its parse status. */
export function listEntries(rebuilt: RebuildResult): WorkspaceEntry[] {
	const entries: WorkspaceEntry[] = [];
	for (const folder of rebuilt.folders) {
		if (!isVisiblePath(folder)) continue;
		entries.push({
			relativePath: folder,
			name: baseName(folder),
			kind: 'folder',
			parseStatus: null,
			objectId: null,
			objectType: null,
			revision: null,
			notDownloaded: false,
		});
	}
	for (const file of rebuilt.entries) {
		if (!isVisiblePath(file.path)) continue;
		const parsed = file.parsed;
		entries.push({
			relativePath: file.path,
			name: baseName(file.path),
			kind: 'file',
			parseStatus: parsed === null ? 'binary' : parsed.kind,
			objectId: parsed?.kind === 'managed' ? parsed.id : null,
			objectType: parsed?.kind === 'managed' ? parsed.type : null,
			revision: parsed === null ? null : file.revision,
			notDownloaded: false,
		});
	}
	entries.sort((left, right) =>
		compareBytes(left.relativePath, right.relativePath),
	);
	return entries.filter(
		(entry, index) =>
			index === 0 || entries[index - 1]!.relativePath !== entry.relativePath,
	);
}

/** `folders_list`: visible folders sorted by path. */
export function listFolders(rebuilt: RebuildResult): FolderEntry[] {
	return rebuilt.folders
		.filter(isVisiblePath)
		.sort(compareBytes)
		.map((relativePath) => ({ relativePath, name: baseName(relativePath) }));
}

/** SQLite `ORDER BY o.updated DESC, o.title`; NULL sorts last when descending. */
function byUpdatedThenTitle(
	left: Pick<WorkspaceObject, 'updated' | 'title'>,
	right: Pick<WorkspaceObject, 'updated' | 'title'>,
): number {
	if (left.updated !== right.updated) {
		if (left.updated === null) return 1;
		if (right.updated === null) return -1;
		return -compareBytes(left.updated, right.updated);
	}
	return compareBytes(left.title, right.title);
}

export type ObjectFilter = {
	type?: string | null | undefined;
	project?: string | null | undefined;
	status?: string | null | undefined;
	priority?: string | null | undefined;
	pathPrefix?: string | null | undefined;
};

/** `objects_query`: unique objects matching every given filter. */
export function queryObjects(
	rebuilt: RebuildResult,
	filter: ObjectFilter,
): WorkspaceObject[] {
	const matches = (
		object: RebuiltManagedFile,
		name: 'project' | 'status' | 'priority',
	) => {
		const expected = filter[name];
		return (
			expected === undefined ||
			expected === null ||
			stringProperty(object, name) === expected
		);
	};
	return uniqueObjects(rebuilt)
		.filter(
			(object) =>
				(filter.type == null || object.type === filter.type) &&
				matches(object, 'project') &&
				matches(object, 'status') &&
				matches(object, 'priority') &&
				(filter.pathPrefix == null ||
					object.relativePath.startsWith(filter.pathPrefix)),
		)
		.sort(byUpdatedThenTitle)
		.map(toObject);
}

export function toObject(object: RebuiltManagedFile): WorkspaceObject {
	return {
		id: object.id,
		type: object.type,
		title: object.title,
		body: object.body,
		relativePath: object.relativePath,
		revision: object.revision,
		created: object.created,
		updated: object.updated,
		properties: object.properties,
	};
}

/** `objects_summaries`: bounded, body-free overview rows. */
export function querySummaries(
	rebuilt: RebuildResult,
	query: ObjectSummaryQuery,
): ObjectSummary[] {
	const limit = Math.min(query.limit ?? 50, 500);
	const order = query.order ?? 'updated-desc';
	const rows = uniqueObjects(rebuilt).filter((object) => {
		if (query.type != null && object.type !== query.type) return false;
		const status = stringProperty(object, 'status');
		if (
			query.statusNot != null &&
			status !== null &&
			status === query.statusNot
		)
			return false;
		if (query.dueOnOrBefore != null) {
			const due = stringProperty(object, 'due');
			if (due === null) return false;
			if (compareBytes([...due].slice(0, 10).join(''), query.dueOnOrBefore) > 0)
				return false;
		}
		return true;
	});
	rows.sort((left, right) => {
		if (order === 'due-asc') {
			const a = stringProperty(left, 'due');
			const b = stringProperty(right, 'due');
			if (a !== b) {
				if (a === null) return -1;
				if (b === null) return 1;
				const compared = compareBytes(a, b);
				if (compared !== 0) return compared;
			}
		}
		return byUpdatedThenTitle(left, right);
	});
	return rows.slice(0, Math.max(0, limit)).map((object) => ({
		id: object.id,
		type: object.type,
		title: object.title,
		relativePath: object.relativePath,
		revision: object.revision,
		created: object.created,
		updated: object.updated,
		properties: object.properties,
	}));
}

/**
 * Every dated property of every object, before the range filter. The native
 * index keeps the same rows: `due`, `date` and `start`, each with the
 * object's `end`, whatever their types.
 */
export function calendarCandidates(rebuilt: RebuildResult): CalendarEntry[] {
	const entries: CalendarEntry[] = [];
	for (const object of rebuilt.managed) {
		const end =
			'end' in object.properties ? propertyText(object.properties.end) : null;
		for (const property of ['due', 'date', 'start'] as const) {
			if (!(property in object.properties)) continue;
			const start = propertyText(object.properties[property]);
			entries.push({
				sourceId: object.id,
				sourceType: object.type,
				title: object.title,
				property,
				start,
				end,
				allDay: utf8Length(start) === 10,
				revision: object.revision,
			});
		}
	}
	return entries;
}

/** `files_list_non_managed_markdown`: idless and malformed Markdown. */
export function listNonManagedMarkdown(
	rebuilt: RebuildResult,
): UnmanagedFile[] {
	return liveMarkdown(rebuilt)
		.flatMap((entry): UnmanagedFile[] => {
			const parsed = entry.parsed!;
			if (parsed.kind === 'managed') return [];
			return [
				{
					relativePath: entry.path,
					title: parsed.title,
					body: parsed.body,
					revision: entry.revision,
					parseStatus: parsed.kind,
					parseError: parsed.kind === 'malformed' ? parsed.error : null,
				},
			];
		})
		.sort((left, right) => compareBytes(left.relativePath, right.relativePath));
}

/** Workspace diagnostics, with the native codes and messages. */
export function diagnostics(rebuilt: RebuildResult): Diagnostic[] {
	const values: Diagnostic[] = [];
	for (const file of rebuilt.malformedMarkdown) {
		if (!isVisiblePath(file.path)) continue;
		values.push({
			code: 'parse_error',
			message: file.error,
			relativePath: file.path,
			objectId: null,
		});
	}
	for (const duplicate of rebuilt.duplicates)
		values.push({
			code: 'identity_conflict',
			message: `Multiple files use this stable ID: ${duplicate.paths.join(', ')}`,
			relativePath: null,
			objectId: duplicate.id,
		});
	const ids = new Set(rebuilt.managed.map((object) => object.id));
	for (const object of rebuilt.managed)
		for (const property of ['project', 'chat_id']) {
			const target = object.properties[property];
			if (typeof target !== 'string' || ids.has(target)) continue;
			values.push({
				code: 'broken_reference',
				message: `Referenced object does not exist: ${target}`,
				relativePath: object.relativePath,
				objectId: object.id,
			});
		}
	return values;
}

/** The number of Markdown files the native index would hold. */
export function indexedFileCount(rebuilt: RebuildResult): number {
	return liveMarkdown(rebuilt).length;
}

// --- Search -------------------------------------------------------------

type SearchRow = {
	objectId: string | null;
	objectType: string | null;
	relativePath: string;
	title: string;
	body: string;
	metadata: string;
	revision: string;
};

/** Column weights of the native `bm25(object_fts, …)` ranking. */
const COLUMN_WEIGHTS = { path: 2, filename: 4, title: 8, body: 1, metadata: 3 };

/** Lowercase, accent-free word tokens, like SQLite's unicode61 tokenizer. */
export function searchTokens(text: string): string[] {
	return (
		text
			.normalize('NFKD')
			.replace(/\p{M}/gu, '')
			.toLowerCase()
			.match(/[\p{L}\p{N}]+/gu) ?? []
	);
}

function searchRows(rebuilt: RebuildResult): SearchRow[] {
	return liveMarkdown(rebuilt).map((entry) => {
		const parsed = entry.parsed!;
		if (parsed.kind === 'managed')
			return {
				objectId: parsed.id,
				objectType: parsed.type,
				relativePath: entry.path,
				title: parsed.title,
				body: parsed.body,
				metadata: Object.values(parsed.properties).map(propertyText).join(' '),
				revision: entry.revision,
			};
		return {
			objectId: null,
			objectType: null,
			relativePath: entry.path,
			title: parsed.title,
			body: parsed.body,
			metadata: '',
			revision: entry.revision,
		};
	});
}

/** Positions in `tokens` where `phrase` starts; its last token is a prefix. */
function phraseHits(tokens: string[], phrase: string[]): number[] {
	const hits: number[] = [];
	for (let start = 0; start + phrase.length <= tokens.length; start += 1) {
		let matched = true;
		for (let offset = 0; offset < phrase.length; offset += 1) {
			const token = tokens[start + offset]!;
			const wanted = phrase[offset]!;
			const last = offset === phrase.length - 1;
			if (last ? !token.startsWith(wanted) : token !== wanted) {
				matched = false;
				break;
			}
		}
		if (matched) hits.push(start);
	}
	return hits;
}

function snippet(body: string, phrases: string[][]): string {
	const words = body.split(/\s+/u).filter((word) => word.length > 0);
	if (words.length === 0) return '';
	const index = words.findIndex((word) => {
		const tokens = searchTokens(word);
		return phrases.some((phrase) =>
			tokens.some((token) => token.startsWith(phrase[0]!)),
		);
	});
	const size = 18;
	const start = Math.max(
		0,
		Math.min(index === -1 ? 0 : index - 6, words.length - size),
	);
	const end = Math.min(words.length, start + size);
	return `${start > 0 ? ' … ' : ''}${words.slice(start, end).join(' ')}${end < words.length ? ' … ' : ''}`;
}

/**
 * `search_query`: every query word must match (as a word prefix) somewhere in
 * the path, file name, title, body or properties. Filtering matches the native
 * full-text index; ranking uses the same column weights with a simpler score,
 * so results tie-break by path instead of by BM25.
 */
export type SearchQuery = {
	query: string;
	type: string | null;
	pathPrefix: string | null;
	limit: number | null;
};

/** A search hit as the transport returns it; the client adds highlights. */
export type SearchHit = Omit<SearchResult, 'highlights'>;

export function search(
	rebuilt: RebuildResult,
	input: SearchQuery,
): SearchHit[] {
	const phrases = input.query
		.split(/\s+/u)
		.filter((word) => word.length > 0)
		.map(searchTokens)
		.filter((phrase) => phrase.length > 0);
	if (phrases.length === 0) return [];
	const prefix = input.pathPrefix?.toLowerCase() ?? null;
	const results: SearchHit[] = [];
	for (const row of searchRows(rebuilt)) {
		if (input.type != null && row.objectType !== input.type) continue;
		if (prefix !== null && !row.relativePath.toLowerCase().startsWith(prefix))
			continue;
		const columns = {
			path: searchTokens(row.relativePath),
			filename: searchTokens(baseName(row.relativePath)),
			title: searchTokens(row.title),
			body: searchTokens(row.body),
			metadata: searchTokens(row.metadata),
		};
		let score = 0;
		let matchedAll = true;
		for (const phrase of phrases) {
			let phraseScore = 0;
			for (const [column, tokens] of Object.entries(columns))
				phraseScore +=
					phraseHits(tokens, phrase).length *
					COLUMN_WEIGHTS[column as keyof typeof COLUMN_WEIGHTS];
			if (phraseScore === 0) {
				matchedAll = false;
				break;
			}
			score += phraseScore;
		}
		if (!matchedAll) continue;
		results.push({
			objectId: row.objectId,
			objectType: row.objectType,
			relativePath: row.relativePath,
			title: row.title,
			snippet: snippet(row.body, phrases),
			// Lower is better, like BM25 in SQLite.
			score: -score,
			revision: row.revision,
		});
	}
	results.sort(
		(left, right) =>
			left.score - right.score ||
			compareBytes(left.relativePath, right.relativePath),
	);
	return results.slice(0, Math.max(0, input.limit ?? 50));
}

// --- Change events ------------------------------------------------------

export type ObjectHead = {
	id: string;
	type: string;
	relativePath: string;
	revision: string;
};

export function objectHeads(rebuilt: RebuildResult): Map<string, ObjectHead> {
	return new Map(
		uniqueObjects(rebuilt).map((object) => [
			object.id,
			{
				id: object.id,
				type: object.type,
				relativePath: object.relativePath,
				revision: object.revision,
			},
		]),
	);
}

/** Native `OBJECT_EVENT_BATCH_LIMIT`: more changes go out as one event. */
export const OBJECT_EVENT_BATCH_LIMIT = 32;

export type ObjectChange = {
	event:
		'object:created' | 'object:moved' | 'object:updated' | 'object:deleted';
	payload: {
		id: string;
		type: string;
		path: string;
		previousPath: string | null;
		revision: string;
	};
};

/** What changed between two projections, like native reconciliation. */
export function objectChanges(
	before: Map<string, ObjectHead>,
	after: Map<string, ObjectHead>,
): ObjectChange[] {
	const changes: ObjectChange[] = [];
	const payload = (head: ObjectHead, previousPath: string | null) => ({
		id: head.id,
		type: head.type,
		path: head.relativePath,
		previousPath,
		revision: head.revision,
	});
	for (const [id, head] of after) {
		const previous = before.get(id);
		if (!previous)
			changes.push({ event: 'object:created', payload: payload(head, null) });
		else if (previous.relativePath !== head.relativePath)
			changes.push({
				event: 'object:moved',
				payload: payload(head, previous.relativePath),
			});
		else if (previous.revision !== head.revision)
			changes.push({ event: 'object:updated', payload: payload(head, null) });
	}
	for (const [id, head] of before)
		if (!after.has(id))
			changes.push({ event: 'object:deleted', payload: payload(head, null) });
	return changes.sort((left, right) =>
		compareBytes(left.payload.id, right.payload.id),
	);
}
