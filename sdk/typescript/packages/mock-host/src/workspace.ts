// The mock host's workspace: files and folders by path, and the objects of
// the plugin's own types, all in memory. It answers the `workspace` and
// `objects` interfaces with the errors Noura gives, though it doesn't check
// properties against the types' schemas.
import { createHash, randomBytes } from 'node:crypto';

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
		path.split('/').every((part) => part !== '' && part !== '.' && part !== '..' && !/\p{Cc}/u.test(part)) &&
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

/** A ULID: the time in milliseconds, then randomness, in Crockford's base 32. */
function ulid(): string {
	const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
	let time = Date.now();
	let head = '';
	for (let i = 0; i < 10; i += 1) {
		head = alphabet[time % 32] + head;
		time = Math.floor(time / 32);
	}
	const random = [...randomBytes(16)].map((byte) => alphabet[byte % 32]).join('');
	return head + random;
}

function instant(): string {
	return new Date().toISOString().replace(/\.\d{3}Z$/, 'Z');
}

function titleOf(path: string): string {
	return (path.split('/').pop() ?? path).replace(/\.md$/, '');
}

export class MockWorkspace {
	/** The workspace's files, by path. Objects live in `objects`. */
	readonly files = new Map<string, Uint8Array>();
	/** The objects of the plugin's types, by ID. */
	readonly objects = new Map<string, MockObject>();

	constructor(files: Record<string, string | Uint8Array> = {}) {
		for (const [path, content] of Object.entries(files)) {
			checkPath(path);
			this.files.set(path, typeof content === 'string' ? new TextEncoder().encode(content) : content);
		}
	}

	private taken(path: string): boolean {
		return this.files.has(path) || [...this.objects.values()].some((object) => object.path === path);
	}

	/** Every file and folder, with the folders their paths imply, sorted. */
	listEntries() {
		const entries = new Map<string, { path: string; kind: 'file' | 'folder'; objectId?: string; objectType?: string; revision?: string }>();
		const addFolders = (path: string) => {
			const parts = path.split('/');
			for (let end = 1; end < parts.length; end += 1) {
				const folder = parts.slice(0, end).join('/');
				entries.set(folder, { path: folder, kind: 'folder' });
			}
		};
		for (const [path, bytes] of this.files) {
			addFolders(path);
			entries.set(path, { path, kind: 'file', revision: revisionOf(bytes) });
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
		const object = [...this.objects.values()].find((candidate) => candidate.path === path);
		if (object) {
			const text = `---\nid: ${object.id}\ntype: ${object.objectType}\n---\n${object.body}`;
			return { path, bytes: new TextEncoder().encode(text), revision: object.revision };
		}
		return refuse('not-found', path);
	}

	readObject(id: string) {
		const object = this.objects.get(id) ?? refuse('not-found', id);
		const { id: objectId, objectType, title, path, body, properties: props, revision } = object;
		return { id: objectId, objectType, title, path, body, properties: props, revision };
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
			const folder = `${object.objectType}s`;
			path = `${folder}/${object.title}.md`;
			for (let number = 2; this.taken(path); number += 1) {
				path = `${folder}/${object.title} ${number}.md`;
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
		return [...this.objects.values()].filter((object) => object.objectType === objectType);
	}

	update(id: string, expectedRevision: string, changes: Changes): MockObject {
		const object = this.current(id, expectedRevision);
		const merged = { ...JSON.parse(object.properties), ...properties(changes.properties) };
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
