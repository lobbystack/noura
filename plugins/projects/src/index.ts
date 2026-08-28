import { definePlugin } from '@noura/plugin-sdk';
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
		],
	},
	activate() {},
});
