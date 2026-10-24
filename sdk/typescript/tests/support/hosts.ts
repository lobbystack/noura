// One way to drive a plugin on the mock host and on the real host, so a
// test can run the same steps on both and compare what each shows.
import { createMockHost, type MockHost, PluginFailure } from '@noura/mock-host';

import { RealHost, type ViewTree } from './real-host.js';

/** A plugin on a host, through what both hosts offer. */
export interface Host {
	readonly name: string;
	/** The plugin's pages, by the plugin's own page IDs. */
	pages(): Promise<{ id: string; title: string }[]>;
	view(page: string): Promise<ViewTree>;
	send(page: string, event: { tag: string; val?: unknown }): Promise<void>;
	tools(): Promise<{ name: string; description: string; inputSchema: string }[]>;
	/** The tool's answer, or the message it failed with. */
	callTool(name: string, args: unknown): Promise<{ answer: string } | { failure: string }>;
	close(): Promise<void>;
}

/** `value` without null fields, as the mock host leaves empty options out. */
function clean(value: unknown): unknown {
	if (Array.isArray(value)) {
		return value.map(clean);
	}
	if (value && typeof value === 'object') {
		return Object.fromEntries(
			Object.entries(value)
				.filter(([, field]) => field !== null && field !== undefined)
				.map(([name, field]) => [name, clean(field)]),
		);
	}
	return value;
}

export async function mockHost(folder: string, files: Record<string, string>): Promise<Host> {
	const host: MockHost = await createMockHost({ folder, files });
	return {
		name: 'mock host',
		pages: () => host.pages(),
		view: (page) => host.view(page),
		send: (page, event) => host.send(page, event),
		tools: () => host.tools(),
		async callTool(name, args) {
			try {
				return { answer: await host.callTool(name, args) };
			} catch (error) {
				if (error instanceof PluginFailure) {
					return { failure: error.message };
				}
				throw error;
			}
		},
		close: () => host.close(),
	};
}

/** The plugin in `folder` on the real host, or `undefined` when it isn't built. */
export async function realHost(folder: string, files: Record<string, string>): Promise<Host | undefined> {
	const host = await RealHost.start(files);
	if (!host) {
		return undefined;
	}
	const installed = await host.install(folder);
	if (!('id' in installed) || installed.problem) {
		await host.close();
		throw new Error(`The plugin didn't install: ${JSON.stringify(installed)}`);
	}
	const id = installed.id;
	const prefix = `plugin:${id}:`;
	return {
		name: 'real host',
		async pages() {
			return (await host.pages())
				.filter((page) => page.id.startsWith(prefix))
				.map((page) => ({ id: page.id.slice(prefix.length), title: page.title }));
		},
		async view(page) {
			const answer = await host.view(prefix + page);
			if (!answer || !('view' in answer)) {
				throw new Error(`The page ${page} shows no view: ${JSON.stringify(answer)}`);
			}
			return clean(answer.view) as ViewTree;
		},
		send: (page, event) => host.send(prefix + page, event),
		async tools() {
			return (await host.tools())
				.filter((tool) => tool.plugin === id)
				.map(({ name, description, inputSchema }) => ({ name, description, inputSchema }));
		},
		callTool: (name, args) => host.callTool(id, name, args),
		close: () => host.close(),
	};
}
