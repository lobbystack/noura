// The real host: Noura's plugin service from app-core, with a workspace
// open, in the `noura-kit-host` binary that `cargo build -p kit-host`
// builds. Each request goes to it as one JSON line and its answer comes
// back as one.
//
// Without the binary the tests that need it are skipped with a note.
// CI sets NOURA_REQUIRE_KIT_HOST=1, which turns a missing binary into a
// failure.
import { type ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..');

/** The kit host binary, or `undefined` when it isn't built. */
export function kitHostBinary(): string | undefined {
	const target = process.env.CARGO_TARGET_DIR ?? join(REPO, 'target');
	const binary =
		process.env.NOURA_KIT_HOST ??
		join(target, 'debug', process.platform === 'win32' ? 'noura-kit-host.exe' : 'noura-kit-host');
	if (existsSync(binary)) {
		return binary;
	}
	if (process.env.NOURA_REQUIRE_KIT_HOST === '1') {
		throw new Error(`The kit host isn't built at ${binary}. Run cargo build -p kit-host.`);
	}
	console.warn(`Skipping the real host: it isn't built at ${binary}. Run cargo build -p kit-host.`);
	return undefined;
}

/** A plugin as Settings lists it. */
export interface InstalledPlugin {
	id: string;
	name: string;
	version: string;
	running: boolean;
	failed: boolean;
	problem: string | null;
	/** The capabilities you approved, by name. */
	approved: string[];
}

/** What installing a folder or an archive gave. */
export type Install = InstalledPlugin | { problem: string };

/** A rail page a plugin adds, with its ID as Noura names it. */
export interface RailPage {
	id: string;
	title: string;
}

/** A node of a checked view, holding its children. */
export interface ViewTree {
	id: string;
	kind: { tag: string; val?: unknown };
	children: ViewTree[];
}

export type ViewAnswer = { view: ViewTree } | { problem: string } | null;

export interface Tool {
	plugin: string;
	name: string;
	description: string;
	inputSchema: string;
}

export type ToolAnswer = { answer: string } | { failure: string };

export class RealHost {
	private next = 1;
	private readonly waiting = new Map<number, { done(value: unknown): void; fail(error: Error): void }>();

	private constructor(
		private readonly child: ChildProcessWithoutNullStreams,
		private readonly root: string,
		/** The workspace folder the host opened. */
		readonly workspace: string,
	) {
		const lines = createInterface({ input: child.stdout });
		lines.on('line', (line) => {
			const answer = JSON.parse(line) as { id: number; ok?: unknown; error?: string };
			const waiting = this.waiting.get(answer.id);
			this.waiting.delete(answer.id);
			if (answer.error !== undefined) {
				waiting?.fail(new Error(answer.error));
			} else {
				waiting?.done(answer.ok ?? null);
			}
		});
	}

	/**
	 * Start the host with a workspace holding `files`, by path. Returns
	 * `undefined` when the binary isn't built.
	 */
	static async start(files: Record<string, string> = {}): Promise<RealHost | undefined> {
		const binary = kitHostBinary();
		if (!binary) {
			return undefined;
		}
		const root = await mkdtemp(join(tmpdir(), 'noura-kit-host-'));
		const workspace = join(root, 'Workspace');
		for (const [path, text] of Object.entries(files)) {
			await mkdir(dirname(join(workspace, path)), { recursive: true });
			await writeFile(join(workspace, path), text);
		}
		const child = spawn(binary, ['--app-data', join(root, 'app-data'), '--workspace', workspace], {
			stdio: ['pipe', 'pipe', 'pipe'],
		});
		let stderr = '';
		child.stderr.on('data', (chunk: Buffer) => (stderr += chunk.toString()));
		await new Promise<void>((ready, fail) => {
			const lines = createInterface({ input: child.stdout });
			lines.once('line', (line) => {
				lines.close();
				if ((JSON.parse(line) as { ready?: boolean }).ready) {
					ready();
				} else {
					fail(new Error(`The kit host didn't start: ${line}`));
				}
			});
			child.once('exit', (code) => fail(new Error(`The kit host stopped with ${code}: ${stderr}`)));
		});
		return new RealHost(child, root, workspace);
	}

	private request<T>(op: string, fields: Record<string, unknown> = {}): Promise<T> {
		const id = this.next++;
		return new Promise<T>((done, fail) => {
			this.waiting.set(id, { done: done as (value: unknown) => void, fail });
			this.child.stdin.write(`${JSON.stringify({ id, op, ...fields })}\n`);
		});
	}

	/** Install the plugin folder `folder`, approve it and turn it on. */
	install(folder: string): Promise<Install> {
		return this.request('install', { folder });
	}

	/** Install the release archive `archive`, approve it and turn it on. */
	installArchive(archive: string): Promise<Install> {
		return this.request('installArchive', { archive });
	}

	plugin(id: string): Promise<InstalledPlugin | null> {
		return this.request('plugin', { plugin: id });
	}

	pages(): Promise<RailPage[]> {
		return this.request('pages');
	}

	view(page: string): Promise<ViewAnswer> {
		return this.request('view', { page });
	}

	send(page: string, event: unknown): Promise<void> {
		return this.request('event', { page, event });
	}

	tools(): Promise<Tool[]> {
		return this.request('tools');
	}

	callTool(plugin: string, name: string, args: unknown): Promise<ToolAnswer> {
		return this.request('callTool', { plugin, name, arguments: JSON.stringify(args) });
	}

	async close(): Promise<void> {
		this.child.stdin.end();
		await new Promise((stopped) => this.child.once('exit', stopped));
		await rm(this.root, { recursive: true, force: true });
	}
}

/** The text of every text node in `tree`, top to bottom. */
export function texts(tree: ViewTree): string[] {
	const found: string[] = [];
	const walk = (node: ViewTree) => {
		if (node.kind.tag === 'text') {
			found.push(node.kind.val as string);
		}
		node.children.forEach(walk);
	};
	walk(tree);
	return found;
}
