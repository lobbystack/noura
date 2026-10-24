// The mock host: a built plugin running in Node against a mock of Noura,
// for tests. It implements every interface of noura:plugin from the same
// WIT the plugin was built against, and it refuses what the plugin's
// manifest doesn't declare as Noura does: the call fails with
// `capability-not-declared` and names the capability.
//
// What it doesn't do: check views against Noura's drawing rules, check
// object properties against their schemas, reach the network, or run
// other plugins. Pass `fetch` and `commands` to answer for those.
import { readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { type Capability, type Manifest, readManifest } from '@noura/plugin-cli';

import { type Exports, type Loaded, load } from './component.js';
import { type MockObject, MockWorkspace } from './workspace.js';

export type { MockObject } from './workspace.js';

/** A node of a view as a tree: each node holds its children. */
export interface ViewNode {
	id: string;
	kind: { tag: string; val?: unknown };
	children: ViewNode[];
}

/** A rail page, by the plugin's own page ID. */
export interface Page {
	id: string;
	title: string;
}

export interface Command {
	id: string;
	title: string;
	palette: boolean;
	plugins: boolean;
}

export interface Tool {
	name: string;
	description: string;
	inputSchema: string;
}

export interface Request {
	method: 'get' | 'head' | 'post' | 'put' | 'patch' | 'delete';
	url: string;
	headers: { name: string; value: string }[];
	body: Uint8Array;
}

export interface Response {
	status: number;
	url: string;
	headers: { name: string; value: string }[];
	body: Uint8Array;
}

/** Something the plugin asked Noura to show. */
export type Opening =
	| { kind: 'object'; id: string; placement: 'preview' | 'new-tab' }
	| { kind: 'page'; page: string }
	| { kind: 'trash'; id: string };

export interface MockHostOptions {
	/** The plugin's folder, holding `plugin.toml` and the built component. Defaults to the current folder. */
	folder?: string;
	/** The workspace's files, by workspace-relative path. */
	files?: Record<string, string | Uint8Array>;
	/** What the clock reads. Defaults to this computer's clock and time zone. */
	clock?: () => { instant: string; timeZone: string };
	/** Answers the requests that pass the manifest's `network` hosts. Without it they fail. */
	fetch?: (request: Request) => Response;
	/** Other plugins' commands the plugin may call, by `plugin/command`. A thrown string fails the call. */
	commands?: Record<string, (input: string) => string>;
}

/** A call the manifest doesn't allow: Noura never makes it. */
export class CapabilityError extends Error {
	override name = 'CapabilityError';
	constructor(readonly capability: string) {
		super(`plugin.toml doesn't declare the ${capability} capability, so Noura never makes this call.`);
	}
}

/** A command or a tool that failed, with the plugin's message. */
export class PluginFailure extends Error {
	override name = 'PluginFailure';
}

/** What the host offers a plugin, by interface name without its version. */
interface Interfaces {
	'noura:plugin/types': Record<string, never>;
	'noura:plugin/ui': Record<string, never>;
	'noura:plugin/workspace': typeof import('noura:plugin/workspace@0.1.0');
	'noura:plugin/objects': typeof import('noura:plugin/objects@0.1.0');
	'noura:plugin/network': typeof import('noura:plugin/network@0.1.0');
	'noura:plugin/command-calls': typeof import('noura:plugin/command-calls@0.1.0');
	'noura:plugin/clock': typeof import('noura:plugin/clock@0.1.0');
	'noura:plugin/navigation': typeof import('noura:plugin/navigation@0.1.0');
}

/** The kernel's own object types, which no plugin registers. */
const KERNEL_TYPES = ['note', 'workspace', 'project', 'chat', 'chat-message'];

/** Types held for one plugin, by the plugin's ID. */
const PLUGIN_TYPES: Record<string, string> = { task: 'tasks' };

function refused(capability: string): never {
	throw { tag: 'capability-not-declared', val: capability };
}

/** The flat node list a plugin returns, as a tree from its first node. */
export function tree(nodes: { id: string; kind: { tag: string; val?: unknown }; children: ArrayLike<number> }[]): ViewNode {
	const build = (index: number, seen: Set<number>): ViewNode => {
		const node = nodes[index];
		if (!node || seen.has(index)) {
			throw new Error(`The view's node list reaches node ${index} twice or doesn't hold it.`);
		}
		seen.add(index);
		return { id: node.id, kind: clean(node.kind) as ViewNode['kind'], children: Array.from(node.children).map((child) => build(child, seen)) };
	};
	if (nodes.length === 0) {
		throw new Error('The plugin returned no nodes.');
	}
	return build(0, new Set());
}

/** `value` without its empty options, so `undefined` and `null` compare alike. */
function clean(value: unknown): unknown {
	if (Array.isArray(value)) {
		return value.map(clean);
	}
	if (value && typeof value === 'object' && !(value instanceof Uint8Array)) {
		return Object.fromEntries(
			Object.entries(value)
				.filter(([, field]) => field !== undefined && field !== null)
				.map(([name, field]) => [name, clean(field)]),
		);
	}
	return value;
}

/** The text of every text node in `view`, top to bottom. */
export function texts(view: ViewNode): string[] {
	const found: string[] = [];
	const walk = (node: ViewNode) => {
		if (node.kind.tag === 'text') {
			found.push(node.kind.val as string);
		}
		node.children.forEach(walk);
	};
	walk(view);
	return found;
}

export class MockHost {
	/** The workspace the plugin reads and writes. */
	readonly workspace: MockWorkspace;
	/** What the plugin asked Noura to open or trash, in order. */
	readonly openings: Opening[] = [];
	private exports: Exports | undefined;
	/** The object types the plugin registered. */
	private types = new Set<string>();
	/** Whether the call running answers something you did, so navigation acts. */
	private acting = false;

	private constructor(
		readonly manifest: Manifest,
		private readonly loaded: Loaded,
		private readonly options: MockHostOptions,
	) {
		this.workspace = new MockWorkspace(options.files);
	}

	/** Load the plugin built in `options.folder` and start it. */
	static async create(options: MockHostOptions = {}): Promise<MockHost> {
		const folder = resolve(options.folder ?? '.');
		const manifest = await readManifest(folder);
		let bytes: Uint8Array;
		try {
			bytes = await readFile(join(folder, manifest.entry));
		} catch {
			throw new Error(`${manifest.entry} isn't there. Build the plugin first.`);
		}
		const host = new MockHost(manifest, await load(bytes), options);
		await host.start();
		return host;
	}

	private declares(name: string): boolean {
		return this.manifest.capabilities.some((capability) => capability.name === name);
	}

	private capability(name: string): Capability | undefined {
		return this.manifest.capabilities.find((capability) => capability.name === name);
	}

	private async start(): Promise<void> {
		this.exports = await this.loaded.instantiate(this.imports());
		this.types.clear();
		if (this.declares('object-types')) {
			for (const declared of this.call('objectTypes', 'types') as { name: string }[]) {
				const held = PLUGIN_TYPES[declared.name];
				if (!KERNEL_TYPES.includes(declared.name) && (held === undefined || held === this.manifest.id)) {
					this.types.add(declared.name);
				}
			}
		}
	}

	/** Call the export `name` of the interface `face`. */
	private call(face: string, name: string, ...args: unknown[]): unknown {
		const exported = this.exports?.[face]?.[name];
		if (!exported) {
			throw new Error(`The plugin doesn't export ${face}.${name}.`);
		}
		return exported(...args);
	}

	/** Run `call` as Noura runs an answer to something you did. */
	private act<T>(call: () => T): T {
		this.acting = true;
		try {
			return call();
		} finally {
			this.acting = false;
		}
	}

	private needs(capability: string): void {
		if (!this.declares(capability)) {
			throw new CapabilityError(capability);
		}
	}

	/**
	 * The host's side of each interface the plugin imports. Its type comes
	 * from the declarations jco generates from the WIT, so the build fails
	 * when the mock leaves out a function of the interface or answers in
	 * another shape.
	 */
	private imports(): Interfaces {
		const workspace = () => {
			if (!this.declares('workspace-read')) {
				refused('workspace-read');
			}
			return this.workspace;
		};
		const objects = (objectType: string) => {
			if (!this.declares('object-types') || !this.types.has(objectType)) {
				refused('object-types');
			}
			return this.workspace;
		};
		const objectsFor = (id: string) => objects(id.split('_')[0] ?? '');
		// Noura opens and trashes things only in answer to an event in one
		// of the plugin's views or a command you ran from the palette.
		const navigation = (refusal = 'Noura opens things only in answer to something you did.') => {
			if (!this.declares('views')) {
				refused('views');
			}
			if (!this.acting) {
				throw { tag: 'failed', val: refusal };
			}
		};
		return {
			'noura:plugin/types': {},
			'noura:plugin/ui': {},
			'noura:plugin/workspace': {
				listEntries: () => workspace().listEntries(),
				readFile: (path: string) => workspace().readFile(path),
				readObject: (id: string) => workspace().readObject(id),
			},
			'noura:plugin/objects': {
				create: (object: Parameters<MockWorkspace['create']>[0]) => objects(object.objectType).create(object),
				get: (id: string) => objectsFor(id).get(id),
				list: (objectType: string) => objects(objectType).list(objectType),
				update: (id: string, revision: string, changes: Parameters<MockWorkspace['update']>[2]) =>
					objectsFor(id).update(id, revision, changes),
				moveTo: (id: string, revision: string, path: string) => objectsFor(id).moveTo(id, revision, path),
				delete: (id: string, revision: string) => objectsFor(id).delete(id, revision),
			},
			'noura:plugin/network': {
				fetch: (request: Request) => {
					const network = this.capability('network') ?? refused('network');
					let url: URL;
					try {
						url = new URL(request.url);
					} catch {
						throw { tag: 'network-refused', val: `${request.url} isn't a URL.` };
					}
					if (url.protocol !== 'https:') {
						throw { tag: 'network-refused', val: 'Noura sends requests only over HTTPS.' };
					}
					if (!network.hosts.includes(url.hostname)) {
						throw { tag: 'network-refused', val: `plugin.toml doesn't list ${url.hostname}.` };
					}
					if (!this.options.fetch) {
						throw { tag: 'failed', val: 'The mock host has no network. Pass fetch to createMockHost.' };
					}
					return this.options.fetch(request);
				},
			},
			'noura:plugin/command-calls': {
				call: (plugin: string, command: string, input: string) => {
					const commands = this.capability('commands') ?? refused('commands');
					const target = `${plugin}/${command}`;
					if (!commands.targets.some((allowed) => allowed === plugin || allowed === target)) {
						throw { tag: 'target-not-declared', val: target };
					}
					const run = this.options.commands?.[target];
					if (!run) {
						throw { tag: 'target-unavailable', val: target };
					}
					try {
						return run(input);
					} catch (error) {
						throw { tag: 'failed', val: String(error) };
					}
				},
			},
			'noura:plugin/clock': {
				now: () =>
					this.options.clock?.() ?? {
						instant: new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
						timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
					},
			},
			'noura:plugin/navigation': {
				openObject: (id: string, placement: 'preview' | 'new-tab') => {
					navigation();
					this.openings.push({ kind: 'object', id, placement });
				},
				openPage: (page: string) => {
					navigation();
					this.openings.push({ kind: 'page', page });
				},
				trashObject: (id: string) => {
					const store = objectsFor(id);
					navigation('Noura moves things to the trash only in answer to something you did.');
					const object = store.get(id);
					store.delete(id, object.revision);
					this.openings.push({ kind: 'trash', id });
				},
			},
		};
	}

	/** The pages the plugin adds to the rail: none without the `views` capability. */
	async pages(): Promise<Page[]> {
		return this.declares('views') ? (this.call('view', 'pages') as Page[]) : [];
	}

	private async page(page: string): Promise<void> {
		this.needs('views');
		if (!(await this.pages()).some((known) => known.id === page)) {
			throw new Error(`The plugin has no page ${page}.`);
		}
	}

	/** The view of the page `page`. */
	async view(page: string): Promise<ViewNode> {
		await this.page(page);
		return tree(this.call('view', 'render', page) as never);
	}

	/** The text of every text node in the page's view, top to bottom. */
	async texts(page: string): Promise<string[]> {
		return texts(await this.view(page));
	}

	/** The section the page adds to the sidebar, or `null` for none. */
	async sidebar(page: string): Promise<ViewNode | null> {
		await this.page(page);
		const nodes = this.call('view', 'sidebar', page) as never[];
		return nodes.length === 0 ? null : tree(nodes);
	}

	/** Rows `start` up to `start + count` of the page's virtual list `node`. */
	async rows(page: string, node: string, start: number, count: number): Promise<unknown[]> {
		await this.page(page);
		return clean(this.call('view', 'rows', page, node, start, count)) as unknown[];
	}

	/** Tell the plugin about `event` in the page's view or its sidebar section. */
	async send(page: string, event: { tag: string; val?: unknown }): Promise<void> {
		await this.page(page);
		this.act(() => this.call('view', 'handleEvent', page, event));
	}

	/** Press the button `node` in the page's view. */
	async press(page: string, node: string): Promise<void> {
		await this.send(page, { tag: 'pressed', val: node });
	}

	/** The commands the plugin offers: none without the `commands` capability. */
	async commands(): Promise<Command[]> {
		return this.declares('commands') ? (this.call('commands', 'commands') as Command[]) : [];
	}

	/** Run the command `id` as the command palette does, with empty input. */
	async runCommand(id: string): Promise<string> {
		this.needs('commands');
		return this.act(() => this.answer(() => this.call('commands', 'run', id, '')));
	}

	/** Run the command `id` with `input`, as another plugin calls it. */
	async callCommand(id: string, input: string): Promise<string> {
		this.needs('commands');
		const offered = (await this.commands()).find((command) => command.id === id);
		if (!offered?.plugins) {
			throw new Error(`The plugin doesn't offer ${id} to other plugins.`);
		}
		return this.answer(() => this.call('commands', 'run', id, input));
	}

	/** The plugin's Inbox sections: none without the `inbox-sections` capability. */
	async inboxSections(): Promise<Page[]> {
		return this.declares('inbox-sections') ? (this.call('inbox', 'sections') as Page[]) : [];
	}

	async inboxSection(section: string): Promise<ViewNode> {
		this.needs('inbox-sections');
		return tree(this.call('inbox', 'renderSection', section) as never);
	}

	async sendInbox(section: string, event: { tag: string; val?: unknown }): Promise<void> {
		this.needs('inbox-sections');
		this.act(() => this.call('inbox', 'handleSectionEvent', section, event));
	}

	/** The title of the plugin's Settings page, or `null` for none. */
	async settingsPage(): Promise<string | null> {
		return this.declares('settings') ? ((this.call('settings', 'page') as string | undefined) ?? null) : null;
	}

	async settings(): Promise<ViewNode> {
		this.needs('settings');
		return tree(this.call('settings', 'renderSettings') as never);
	}

	async sendSettings(event: { tag: string; val?: unknown }): Promise<void> {
		this.needs('settings');
		this.act(() => this.call('settings', 'handleSettingsEvent', event));
	}

	/** The tools the plugin offers: none without the `tools` capability. */
	async tools(): Promise<Tool[]> {
		return this.declares('tools') ? (this.call('tools', 'tools') as Tool[]) : [];
	}

	/** Call the tool `name` with `args`, a value or its JSON text. */
	async callTool(name: string, args: unknown): Promise<string> {
		this.needs('tools');
		if (!(await this.tools()).some((tool) => tool.name === name)) {
			throw new Error(`The plugin has no tool ${name}.`);
		}
		const text = typeof args === 'string' ? args : JSON.stringify(args);
		return this.answer(() => this.call('tools', 'callTool', name, text));
	}

	/** The object types the plugin registered. */
	async objectTypes(): Promise<string[]> {
		return [...this.types].sort();
	}

	/** The properties panel of the object `id`. */
	async panel(id: string): Promise<ViewNode> {
		this.needs('object-types');
		return tree(this.call('objectTypes', 'renderPanel', id) as never);
	}

	async sendPanel(id: string, event: { tag: string; val?: unknown }): Promise<void> {
		this.needs('object-types');
		this.act(() => this.call('objectTypes', 'handlePanelEvent', id, event));
	}

	/** The objects of the plugin's types now. */
	objects(): MockObject[] {
		return [...this.workspace.objects.values()];
	}

	/** Remove the files the host wrote to load the plugin. */
	async close(): Promise<void> {
		await this.loaded.dispose();
	}

	/** A command's or a tool's answer, or its failure as a {@link PluginFailure}. */
	private answer(run: () => unknown): string {
		try {
			return run() as string;
		} catch (error) {
			const payload = (error as { payload?: unknown }).payload;
			if (typeof payload === 'string') {
				throw new PluginFailure(payload);
			}
			throw error;
		}
	}
}

/** Load the plugin built in `options.folder`, the current folder by default, and start it. */
export function createMockHost(options: MockHostOptions = {}): Promise<MockHost> {
	return MockHost.create(options);
}
