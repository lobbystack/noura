import { z } from 'zod';
export { syncFileChangeSchema } from './sync';

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
export const pluginIdSchema = z
	.string()
	.regex(
		/^[a-z][a-z0-9-]{0,63}$/,
		'Plugin identifiers use a lowercase letter, then lowercase letters, digits, or hyphens',
	);
export const workspaceManifestSchema = z.object({
	id: objectIdSchema,
	format_version: z.literal(1),
	name: z.string().min(1),
	created: z.string().datetime({ offset: true }),
	updated: z.string().datetime({ offset: true }),
	enabled_plugins: z.array(pluginIdSchema),
	ignore: z.array(z.string()),
});
export const chatIdSchema = z.string().regex(/^chat_[0-9a-hjkmnp-tv-z]{26}$/);
export const chatMessageIdSchema = z
	.string()
	.regex(/^chat-message_[0-9a-hjkmnp-tv-z]{26}$/);
export const chatFrontmatterSchema = z
	.object({
		id: chatIdSchema,
		type: z.literal('chat'),
		title: z.string().min(1),
		retention: z.enum(['ephemeral', 'permanent']),
		retention_days: z
			.number()
			.int()
			.nonnegative()
			.nullable()
			.optional()
			.transform((value) => value ?? null),
		created: z.string().datetime({ offset: true }),
		updated: z.string().datetime({ offset: true }),
	})
	.passthrough()
	.superRefine((chat, context) => {
		if (chat.retention === 'ephemeral' && chat.retention_days !== 30) {
			context.addIssue({
				code: 'custom',
				message: 'Ephemeral chats require retention_days: 30',
			});
		}
		if (chat.retention === 'permanent' && chat.retention_days !== null) {
			context.addIssue({
				code: 'custom',
				message: 'Permanent chats require retention_days: null',
			});
		}
	});
const optionalMessageMetadataSchema = z
	.string()
	.refine(
		(value) => value.length > 0 && !/\p{Cc}/u.test(value),
		'Optional message metadata must be non-empty and single-line',
	)
	.nullable()
	.optional()
	.transform((value) => value ?? null);
const errorCodeSchema = z
	.string()
	.refine(
		(value) => value.length <= 128 && /^[A-Za-z0-9._-]+$/.test(value),
		'error_code must use at most 128 ASCII letters, digits, dots, hyphens, or underscores',
	)
	.nullable()
	.optional()
	.transform((value) => value ?? null);
const chatMessageBaseSchema = z
	.object({
		id: chatMessageIdSchema,
		type: z.literal('chat-message'),
		chat_id: chatIdSchema,
		run_id: z
			.string()
			.refine(
				(value) => !/^\p{White_Space}*$/u.test(value) && !/\p{Cc}/u.test(value),
				'run_id must be a non-empty single-line string',
			),
		kind: z.enum([
			'user',
			'assistant',
			'tool-call',
			'tool-result',
			'context-summary',
		]),
		status: z.enum([
			'in-progress',
			'completed',
			'interrupted',
			'cancelled',
			'failed',
		]),
		content_type: z.enum(['text/markdown', 'application/json']),
		provider_id: optionalMessageMetadataSchema,
		model_id: optionalMessageMetadataSchema,
		tool_call_id: optionalMessageMetadataSchema,
		tool_name: optionalMessageMetadataSchema,
		error_code: errorCodeSchema,
		summarizes_through_message_id: chatMessageIdSchema
			.nullable()
			.optional()
			.transform((value) => value ?? null),
		created: z.string().datetime({ offset: true }),
		updated: z.string().datetime({ offset: true }),
	})
	.passthrough();
export const chatMessageFrontmatterSchema = chatMessageBaseSchema.superRefine(
	(message, context) => {
		const tool = message.kind === 'tool-call' || message.kind === 'tool-result';
		const assistant = message.kind === 'assistant';
		const summary = message.kind === 'context-summary';
		if (tool !== (message.content_type === 'application/json'))
			context.addIssue({
				code: 'custom',
				message: 'Only tool messages use application/json',
			});
		if (tool !== (message.tool_call_id !== null && message.tool_name !== null))
			context.addIssue({
				code: 'custom',
				message: 'Tool metadata must match tool messages',
			});
		if (!tool && (message.tool_call_id !== null || message.tool_name !== null))
			context.addIssue({
				code: 'custom',
				message: 'Only tool messages may include tool metadata',
			});
		if (
			assistant !== (message.provider_id !== null && message.model_id !== null)
		)
			context.addIssue({
				code: 'custom',
				message: 'Assistant provider metadata must match assistant messages',
			});
		if (
			!assistant &&
			(message.provider_id !== null || message.model_id !== null)
		)
			context.addIssue({
				code: 'custom',
				message: 'Only assistant messages may include provider metadata',
			});
		const errorCodeIsValid =
			message.status === 'failed'
				? message.error_code !== null
				: message.status === 'interrupted' || message.error_code === null;
		if (!errorCodeIsValid)
			context.addIssue({
				code: 'custom',
				message:
					'Failed messages require error_code; interrupted messages may include it; other statuses omit it',
			});
		if (summary !== (message.summarizes_through_message_id !== null))
			context.addIssue({
				code: 'custom',
				message: 'Context summaries require summarizes_through_message_id',
			});
	},
);
