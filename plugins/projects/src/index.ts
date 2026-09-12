import { definePlugin, type PluginContext } from '@noura/plugin-sdk';
import { z } from 'zod';
export const projectPropertiesSchema = z
	.object({
		status: z
			.enum(['planned', 'active', 'on-hold', 'completed', 'cancelled'])
			.default('planned'),
	})
	.passthrough();
export function defaultProjectPath(title: string, shortId: string) {
	const slug =
		title
			.normalize('NFKD')
			.toLowerCase()
			.replace(/[^a-z0-9]+/g, '-')
			.replace(/^-|-$/g, '') || 'project';
	return `projects/${slug}--${shortId}/project.md`;
}

interface CreateProjectInput {
	title?: unknown;
	body?: unknown;
	properties?: unknown;
}

const toolDisposers = new WeakMap<object, Array<() => boolean>>();

async function createProject(context: PluginContext, input: unknown) {
	const { title, body, properties } = (input ?? {}) as CreateProjectInput;
	if (typeof title !== 'string' || title.trim().length === 0) {
		throw new Error('A project title is required');
	}
	if (body !== undefined && typeof body !== 'string') {
		throw new Error('A project body must be Markdown text');
	}
	const result = await context.objects.create({
		type: 'project',
		title,
		body,
		properties:
			properties === undefined
				? undefined
				: projectPropertiesSchema.parse(properties),
	});
	return result.value;
}

export default definePlugin({
	manifest: {
		id: 'projects',
		name: 'Projects',
		version: '0.1.0',
		capabilities: [
			'workspace.objects',
			'workspace.search',
			'workspace.commands',
			'workspace.events',
			'ai.tools',
		],
		platforms: ['desktop'],
	},
	activate(context) {
		const disposers = [
			context.ai.registerTool({
				name: 'projects.create',
				description: 'Create a project in the workspace.',
				inputSchema: {
					type: 'object',
					properties: {
						title: { type: 'string', minLength: 1 },
						body: { type: 'string' },
						properties: { type: 'object' },
					},
					required: ['title'],
					additionalProperties: false,
				},
				risk: 'high',
				execute: (input) => createProject(context, input),
			}),
		];
		toolDisposers.set(context, disposers);
	},
	deactivate(context) {
		for (const dispose of toolDisposers.get(context)?.splice(0) ?? []) {
			dispose();
		}
	},
});
