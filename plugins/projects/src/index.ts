import { definePlugin, type PluginContext } from '@noura/plugin-sdk';

interface CreateProjectInput {
	title?: unknown;
	body?: unknown;
	properties?: unknown;
}

const commandDisposers = new WeakMap<object, Array<() => void>>();

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
		properties: properties === undefined ? undefined : properties,
	});
	return result.value;
}

export default definePlugin({
	manifest: {
		id: 'projects',
		name: 'Projects',
		version: '0.1.0',
		capabilities: ['workspace.objects', 'workspace.commands', 'ai.tools'],
		platforms: ['desktop', 'web'],
		activationCapabilities: {
			desktop: ['workspace.objects', 'workspace.commands', 'ai.tools'],
			web: ['workspace.objects', 'workspace.commands'],
		},
	},
	activate(context) {
		const disposers = [
			context.commands.register({
				id: 'projects.create',
				title: 'Create project',
				execute: (input) => createProject(context, input),
			}),
		];
		if (context.platform !== 'web') {
			disposers.push(
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
			);
		}
		commandDisposers.set(context, disposers);
	},
	deactivate(context) {
		for (const dispose of commandDisposers.get(context)?.splice(0) ?? []) {
			dispose();
		}
	},
});
