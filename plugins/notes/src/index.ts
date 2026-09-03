import { definePlugin, type PluginContext } from '@noura/plugin-sdk';

interface CreateNoteInput {
	title?: unknown;
	body?: unknown;
}

const commandDisposers = new WeakMap<object, Array<() => void>>();

async function createNote(context: PluginContext, input: unknown) {
	const { title, body } = (input ?? {}) as CreateNoteInput;
	if (typeof title !== 'string' || title.trim().length === 0) {
		throw new Error('A note title is required');
	}
	if (body !== undefined && typeof body !== 'string') {
		throw new Error('A note body must be Markdown text');
	}
	const result = await context.objects.create({
		type: 'note',
		title,
		body,
	});
	return result.value;
}

export default definePlugin({
	manifest: {
		id: 'notes',
		name: 'Notes',
		version: '0.1.0',
		capabilities: [
			'workspace.objects',
			'workspace.search',
			'workspace.commands',
			'workspace.events',
			'ai.tools',
		],
	},
	activate(context) {
		const disposers: Array<() => void> = [];
		disposers.push(
			context.commands.register({
				id: 'notes.create',
				title: 'Create note',
				async execute(input) {
					return createNote(context, input);
				},
			}),
		);
		disposers.push(
			context.ai.registerTool({
				name: 'notes.create',
				description: 'Create a Markdown note in the workspace.',
				inputSchema: {
					type: 'object',
					properties: {
						title: { type: 'string', minLength: 1 },
						body: { type: 'string' },
					},
					required: ['title'],
					additionalProperties: false,
				},
				risk: 'high',
				execute: (input) => createNote(context, input),
			}),
		);
		commandDisposers.set(context, disposers);
	},
	deactivate(context) {
		for (const dispose of commandDisposers.get(context)?.splice(0) ?? []) {
			dispose();
		}
	},
});
