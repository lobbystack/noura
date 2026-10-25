// The mock host's workspace: files and folders by path, and the objects of
// the plugin's own types, all in memory. It answers the `workspace` and
// `objects` interfaces with the errors Noura gives, though it doesn't check
// properties against the types' schemas or fill in their defaults. A
// Markdown file you seed whose frontmatter holds an `id` and a `type` is an
// object that `workspace` reads, as in Noura; the `objects` interface works
// on the objects the plugin writes, whose files read as Noura writes them.
import { createHash, randomBytes } from 'node:crypto';
import { parse as parseYaml, stringify as stringifyYaml } from 'yaml';

/** A WIT `error` variant from noura:plugin/types, which a call throws. */
export type HostError = { tag: string; val?: unknown };

function refuse(tag: string, val?: unknown): never {
	throw val === undefined ? { tag } : { tag, val };
}

/** An object as the `objects` interface returns it. */
export interface MockObject {
	id: string;
	objectType: string;
	title: string;
	path: string;
	body: string;
	/** The frontmatter's other properties, as a JSON object. */
	properties: string;
	revision: string;
	created?: string;
	updated?: string;
}

/** An object as the `workspace` interface reads it. */
export interface ReadObject {
	id: string;
	objectType: string;
	title: string;
	path: string;
	body: string;
	properties: string;
	revision: string;
}

export interface NewObject {
	objectType: string;
	title: string;
	path?: string;
	body: string;
	properties: string;
}

export interface Changes {
	body?: string;
	properties: string;
	remove: string[];
}

/** Whether `path` is a workspace-relative path a plugin may name. */
export function validPath(path: string): boolean {
	return (
		path !== '' &&
		path.length <= 4096 &&
		!path.startsWith('/') &&
		!path.includes('\\') &&
		path
			.split('/')
			.every(
				(part) =>
					part !== '' && part !== '.' && part !== '..' && !/\p{Cc}/u.test(part),
			) &&
		path.split('/')[0] !== '.noura'
	);
}

function checkPath(path: string): void {
	if (!validPath(path)) {
		refuse('invalid-path', path);
	}
}

function revisionOf(bytes: Uint8Array): string {
	return createHash('sha256').update(bytes).digest('hex').slice(0, 32);
}

function properties(text: string): Record<string, unknown> {
	if (text.trim() === '') {
		return {};
	}
	try {
		const value: unknown = JSON.parse(text);
		if (value && typeof value === 'object' && !Array.isArray(value)) {
			return value as Record<string, unknown>;
		}
	} catch {
		// Refused below.
	}
	return refuse('failed', 'Properties must be a JSON object.');
}

/** A ULID in lowercase, as Noura writes stable IDs: the time in milliseconds, then randomness, in Crockford's base 32. */
export function ulid(): string {
	const alphabet = '0123456789abcdefghjkmnpqrstvwxyz';
	let time = Date.now();
	let head = '';
	for (let i = 0; i < 10; i += 1) {
		head = alphabet[time % 32] + head;
		time = Math.floor(time / 32);
	}
	const random = [...randomBytes(16)]
		.map((byte) => alphabet[byte % 32])
		.join('');
	return head + random;
}

/** The time now as Noura writes it: in UTC, without the fraction of a second's trailing zeros. */
export function instant(): string {
	return new Date().toISOString().replace(/\.?0+Z$/, 'Z');
}

/** A file's title: its name without the extension. */
function titleOf(path: string): string {
	const name = path.split('/').pop() ?? path;
	const dot = name.lastIndexOf('.');
	return dot > 0 ? name.slice(0, dot) : name;
}

/** Whether Noura indexes the file at `path` for the objects it holds. */
function isMarkdown(path: string): boolean {
	return path.endsWith('.md');
}

/** Whether `id` is a stable ID of the type `objectType`: the type, `_` and a lowercase ULID. */
function validId(id: string, objectType: string): boolean {
	return (
		/^[a-z][a-z-]*$/.test(objectType) &&
		id.startsWith(`${objectType}_`) &&
		/^[0-7][0-9a-hjkmnp-tv-z]{25}$/.test(id.slice(objectType.length + 1))
	);
}

/**
 * The object the file `bytes` at `path` holds, read as Noura reads
 * Markdown: frontmatter between `---` lines whose `id` and `type` name a
 * stable ID. A file without one holds no object.
 */
function parseObject(path: string, bytes: Uint8Array): ReadObject | undefined {
	let text: string;
	try {
		text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(
			bytes,
		);
	} catch {
		return undefined;
	}
	text = text.replace(/^\uFEFF/, '').replaceAll('\r\n', '\n');
	if (!text.startsWith('---\n')) {
		return undefined;
	}
	const close = text.indexOf('\n---\n', 4);
	if (close === -1) {
		return undefined;
	}
	let frontmatter: unknown;
	try {
		frontmatter = parseYaml(text.slice(4, close + 1));
	} catch {
		return undefined;
	}
	if (
		!frontmatter ||
		typeof frontmatter !== 'object' ||
		Array.isArray(frontmatter)
	) {
		return undefined;
	}
	const {
		id,
		type: objectType,
		created: _created,
		updated: _updated,
		...rest
	} = frontmatter as Record<string, unknown>;
	if (
		typeof id !== 'string' ||
		typeof objectType !== 'string' ||
		!validId(id, objectType)
	) {
		return undefined;
	}
	const properties = Object.fromEntries(
		Object.keys(rest)
			.sort()
			.map((name) => [name, rest[name]]),
	);
	return {
		id,
		objectType,
		title: titleOf(path),
		path,
		body: text.slice(close + '\n---\n'.length).replace(/\r+\n/g, '\n'),
		properties: JSON.stringify(properties),
		revision: revisionOf(bytes),
	};
}

/** The frontmatter fields Noura writes for every object, which no property can name. */
const MANAGED = new Set(['id', 'type', 'created', 'updated']);

/** Options that make the YAML library write frontmatter as Noura's writer does. */
const NOURA_YAML = {
	indentSeq: false,
	lineWidth: 0,
	minContentWidth: 0,
	singleQuote: true,
} as const;

/**
 * The file Noura writes for `object`: frontmatter with `id` and `type`, the
 * properties its type declares in `declared` order, the others sorted by
 * name, then `created` and `updated`, and the body after it.
 */
function serialize(object: MockObject, declared: readonly string[]): string {
	const values = JSON.parse(object.properties) as Record<string, unknown>;
	// Noura keeps a property's own fields sorted by name, as it keeps JSON.
	const sorted = (value: unknown): unknown =>
		Array.isArray(value)
			? value.map(sorted)
			: value && typeof value === 'object'
				? new Map(
						Object.keys(value)
							.sort()
							.map((name) => [
								name,
								sorted((value as Record<string, unknown>)[name]),
							]),
					)
				: value;
	const names = Object.keys(values).filter((name) => !MANAGED.has(name));
	const entries: [string, unknown][] = [
		['id', object.id],
		['type', object.objectType],
		...declared
			.filter((name) => names.includes(name))
			.map((name): [string, unknown] => [name, sorted(values[name])]),
		...names
			.filter((name) => !declared.includes(name))
			.sort()
			.map((name): [string, unknown] => [name, sorted(values[name])]),
	];
	if (object.created) {
		entries.push(['created', object.created]);
	}
	if (object.updated) {
		entries.push(['updated', object.updated]);
	}
	return `---\n${stringifyYaml(new Map(entries), NOURA_YAML)}---\n${object.body}`;
}

/** The properties a type's schema declares, in order, or none when it isn't one. */
function declaredBy(schema: string): string[] {
	try {
		const { properties } = JSON.parse(schema) as {
			properties?: { name?: unknown }[];
		};
		return (properties ?? []).flatMap((property) =>
			typeof property?.name === 'string' ? [property.name] : [],
		);
	} catch {
		return [];
	}
}

/** `object` as the `workspace` interface reads it. */
function readable(object: MockObject): ReadObject {
	const { id, objectType, title, path, body, properties, revision } = object;
	return { id, objectType, title, path, body, properties, revision };
}

/** Where a new object titled `name` goes without a path, as Noura names it. */
function defaultPath(objectType: string, name: string): string {
	return objectType === 'project'
		? `projects/${name}/${name}.md`
		: `${objectType}s/${name}.md`;
}

export class MockWorkspace {
	/** The workspace's files, by path. Objects live in `objects`. */
	readonly files = new Map<string, Uint8Array>();
	/** The objects of the plugin's types, by ID. */
	readonly objects = new Map<string, MockObject>();
	/** The properties each of the plugin's types declares, in order, by type. */
	private readonly declared = new Map<string, string[]>();

	constructor(files: Record<string, string | Uint8Array> = {}) {
		for (const [path, content] of Object.entries(files)) {
			checkPath(path);
			this.files.set(
				path,
				typeof content === 'string'
					? new TextEncoder().encode(content)
					: content,
			);
		}
	}

	/** Take the property order of `objectType` from its schema, as Noura writes its objects. */
	registerType(objectType: string, schema: string): void {
		this.declared.set(objectType, declaredBy(schema));
	}

	private taken(path: string): boolean {
		return (
			this.files.has(path) ||
			[...this.objects.values()].some((object) => object.path === path)
		);
	}

	/** Every file and folder, with the folders their paths imply, sorted. */
	listEntries() {
		const entries = new Map<
			string,
			{
				path: string;
				kind: 'file' | 'folder';
				objectId?: string;
				objectType?: string;
				revision?: string;
			}
		>();
		const addFolders = (path: string) => {
			const parts = path.split('/');
			for (let end = 1; end < parts.length; end += 1) {
				const folder = parts.slice(0, end).join('/');
				entries.set(folder, { path: folder, kind: 'folder' });
			}
		};
		for (const [path, bytes] of this.files) {
			addFolders(path);
			const object = isMarkdown(path) ? parseObject(path, bytes) : undefined;
			entries.set(path, {
				path,
				kind: 'file',
				objectId: object?.id,
				objectType: object?.objectType,
				revision: revisionOf(bytes),
			});
		}
		for (const object of this.objects.values()) {
			addFolders(object.path);
			entries.set(object.path, {
				path: object.path,
				kind: 'file',
				objectId: object.id,
				objectType: object.objectType,
				revision: object.revision,
			});
		}
		return [...entries.values()]
			.sort((a, b) => a.path.localeCompare(b.path))
			.map((entry) => ({ tag: 'ok' as const, val: entry }));
	}

	readFile(path: string) {
		checkPath(path);
		const bytes = this.files.get(path);
		if (bytes) {
			return { path, bytes, revision: revisionOf(bytes) };
		}
		const object = [...this.objects.values()].find(
			(candidate) => candidate.path === path,
		);
		if (object) {
			const text = serialize(
				object,
				this.declared.get(object.objectType) ?? [],
			);
			return {
				path,
				bytes: new TextEncoder().encode(text),
				revision: object.revision,
			};
		}
		return refuse('not-found', path);
	}

	/** The plugin's own objects and the seeded Markdown files that hold the ID `id`. */
	private holding(id: string): ReadObject[] {
		const found = [...this.objects.values()]
			.filter((object) => object.id === id)
			.map(readable);
		for (const [path, bytes] of this.files) {
			const object = isMarkdown(path) ? parseObject(path, bytes) : undefined;
			if (object?.id === id) {
				found.push(object);
			}
		}
		return found;
	}

	/** The object with the stable ID `id`, refused when more than one file holds it. */
	readObject(id: string): ReadObject {
		const found = this.holding(id);
		if (found.length > 1) {
			refuse('identity-conflict', id);
		}
		return found[0] ?? refuse('not-found', id);
	}

	/** The object the file at `path` holds, whichever other file shares its ID. */
	readObjectAt(path: string): ReadObject {
		checkPath(path);
		const own = [...this.objects.values()].find(
			(object) => object.path === path,
		);
		if (own) {
			return readable(own);
		}
		const bytes = this.files.get(path) ?? refuse('not-found', path);
		return parseObject(path, bytes) ?? refuse('not-found', path);
	}

	create(object: NewObject): MockObject {
		let path = object.path;
		if (path !== undefined) {
			checkPath(path);
			if (!path.endsWith('.md')) {
				refuse('invalid-path', path);
			}
			if (this.taken(path)) {
				refuse('already-exists', path);
			}
		} else {
			const title = object.title.trim() || 'Untitled';
			path = defaultPath(object.objectType, title);
			for (let number = 1; this.taken(path); number += 1) {
				path = defaultPath(object.objectType, `${title} ${number}`);
			}
			checkPath(path);
		}
		const now = instant();
		const created: MockObject = {
			id: `${object.objectType}_${ulid()}`,
			objectType: object.objectType,
			title: titleOf(path),
			path,
			body: object.body,
			properties: JSON.stringify(properties(object.properties)),
			revision: '',
			created: now,
			updated: now,
		};
		created.revision = this.revise(created);
		this.objects.set(created.id, created);
		return created;
	}

	get(id: string): MockObject {
		return this.objects.get(id) ?? refuse('not-found', id);
	}

	list(objectType: string): MockObject[] {
		return [...this.objects.values()].filter(
			(object) => object.objectType === objectType,
		);
	}

	update(id: string, expectedRevision: string, changes: Changes): MockObject {
		const object = this.current(id, expectedRevision);
		const merged = {
			...JSON.parse(object.properties),
			...properties(changes.properties),
		};
		for (const name of changes.remove) {
			delete merged[name];
		}
		const updated = {
			...object,
			body: changes.body ?? object.body,
			properties: JSON.stringify(merged),
			updated: instant(),
		};
		updated.revision = this.revise(updated);
		this.objects.set(id, updated);
		return updated;
	}

	moveTo(id: string, expectedRevision: string, path: string): MockObject {
		const object = this.current(id, expectedRevision);
		checkPath(path);
		if (path !== object.path && this.taken(path)) {
			refuse('already-exists', path);
		}
		const moved = { ...object, path, title: titleOf(path) };
		moved.revision = this.revise(moved);
		this.objects.set(id, moved);
		return moved;
	}

	delete(id: string, expectedRevision: string): void {
		this.current(id, expectedRevision);
		this.objects.delete(id);
	}

	private current(id: string, expectedRevision: string): MockObject {
		const object = this.get(id);
		if (object.revision !== expectedRevision) {
			refuse('revision-conflict', id);
		}
		return object;
	}

	private revise(object: MockObject): string {
		const { revision: _, ...content } = object;
		return revisionOf(new TextEncoder().encode(JSON.stringify(content)));
	}
}
