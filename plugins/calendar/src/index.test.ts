import { describe, expect, test } from 'bun:test';
import {
	PluginHost,
	type PluginCapability,
	type PluginHostServices,
} from '@noura/plugin-sdk';
import type { WorkspaceObject } from '@noura/shared';
import calendar, { upcomingCalendarEntries } from './index';

function services(
	tools: string[],
	providers: string[],
	objects: WorkspaceObject[] = [],
): PluginHostServices {
	return {
		files: {
			list: async () => [],
			listNonManagedMarkdown: async () => [],
			createFolder: async () => {},
			moveFolder: async () => {},
			removeEmptyFolder: async () => {},
		},
		objects: {
			list: async () => objects,
			get: async () => {
				throw new Error('not used');
			},
			create: async () => {
				throw new Error('not used');
			},
			update: async () => {
				throw new Error('not used');
			},
		},
		search: { query: async () => [] },
		events: { subscribe: async () => () => {} },
		commands: { register: () => () => {} },
		storage: {
			get: async () => undefined,
			set: async () => {},
			delete: async () => false,
		},
		ai: {
			registerTool: (definition) => {
				tools.push(definition.name);
				return () => true;
			},
			registerContextProvider: (definition) => {
				providers.push(definition.id);
				return () => true;
			},
			registerInstructionProvider: () => () => true,
		},
	};
}

const tomorrow = () =>
	new Date(Date.now() + 24 * 60 * 60 * 1000).toISOString().slice(0, 10);

describe('calendar plugin', () => {
	test('advertises desktop and web but requires no AI on web', () => {
		expect(calendar.manifest.platforms).toEqual(['desktop', 'web']);
		expect(calendar.manifest.activationCapabilities?.web).toEqual([
			'workspace.objects',
			'workspace.events',
		]);
		expect(
			calendar.manifest.activationCapabilities?.web?.some((capability) =>
				capability.startsWith('ai.'),
			),
		).toBe(false);
	});

	test('activates on web without AI capabilities and does not throw', async () => {
		const tools: string[] = [];
		const providers: string[] = [];
		const host = new PluginHost(services(tools, providers), {
			platform: 'web',
			supportedCapabilities: [
				'workspace.objects',
				'workspace.events',
			] satisfies PluginCapability[],
		});
		await host.activate(calendar);
		expect(host.isActive('calendar')).toBe(true);
		expect(tools).toEqual([]);
		expect(providers).toEqual([]);
		expect(await host.deactivate('calendar')).toBe(true);
	});

	test('still registers the AI context provider and tool on desktop', async () => {
		const tools: string[] = [];
		const providers: string[] = [];
		const host = new PluginHost(services(tools, providers), {
			platform: 'desktop',
		});
		await host.activate(calendar);
		expect(providers).toEqual(['calendar.upcoming-week']);
		expect(tools).toEqual(['calendar.upcoming']);
	});

	test('a web host refuses an AI-capability attempt from the calendar manifest', async () => {
		const host = new PluginHost(services([], []), {
			platform: 'web',
			supportedCapabilities: ['workspace.objects', 'workspace.events'],
		});
		await expect(
			host.activate({
				manifest: calendar.manifest,
				activate(context) {
					context.ai.registerTool({
						name: 'calendar.upcoming',
						description: 'Should not register on web.',
						inputSchema: { type: 'object', additionalProperties: false },
						risk: 'low',
						execute: async () => [],
					});
				},
			}),
		).rejects.toMatchObject({ code: 'plugin_capability_unsupported' });
	});

	test('exposes the read-only upcoming view without AI', () => {
		const task = (
			id: string,
			title: string,
			status: string,
		): WorkspaceObject => ({
			id,
			type: 'task',
			title,
			body: '',
			relativePath: `${id}.md`,
			revision: 'rev',
			created: null,
			updated: null,
			properties: { due: tomorrow(), status },
		});
		const entries = upcomingCalendarEntries(
			[
				task('task_due', 'Due soon', 'todo'),
				task('task_done', 'Settled', 'done'),
			],
			new Date(),
			7,
		);
		expect(entries).toEqual([
			{
				title: 'Due soon',
				content: `${tomorrow()} (task)`,
				sourceId: 'task_due',
			},
		]);
	});
});
