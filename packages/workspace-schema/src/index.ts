import { z } from 'zod';

export const objectIdSchema = z
	.string()
	.regex(/^[a-z][a-z-]*_[0-9a-hjkmnp-tv-z]{26}$/);
export const dateValueSchema = z
	.string()
	.refine(
		(value) =>
			/^\d{4}-\d{2}-\d{2}$/.test(value) ||
			(!Number.isNaN(Date.parse(value)) &&
				/(?:Z|[+-]\d{2}:\d{2})$/.test(value)),
		'Use YYYY-MM-DD or RFC 3339 with an explicit offset',
	);
export const taskStatusSchema = z.enum([
	'todo',
	'in-progress',
	'done',
	'cancelled',
]);
export const taskPrioritySchema = z.enum(['low', 'medium', 'high', 'urgent']);
export const projectStatusSchema = z.enum([
	'planned',
	'active',
	'on-hold',
	'completed',
	'cancelled',
]);
export const workspaceManifestSchema = z.object({
	id: objectIdSchema,
	format_version: z.literal(1),
	name: z.string().min(1),
	created: z.string().datetime({ offset: true }),
	updated: z.string().datetime({ offset: true }),
	enabled_plugins: z.array(z.string()),
	ignore: z.array(z.string()),
});
