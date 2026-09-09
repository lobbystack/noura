import { z } from 'zod';

const reserved = new Set([
	'.noura',
	'.git',
	'node_modules',
	'target',
]);
const portablePath = z
	.string()
	.refine(
		(path) =>
			new TextEncoder().encode(path).length <= 4096 &&
			!/[\x00-\x1f\x7f\\:<>"|?*]/.test(path) &&
			!reserved.has(
				path.split('/')[0]!.replace(/[A-Z]/g, (letter) => letter.toLowerCase()),
			) &&
			path
				.split('/')
				.every(
					(part) =>
						part !== '' &&
						part !== '.' &&
						part !== '..' &&
						!/[. ]$/.test(part) &&
						!/^(CON|CONIN\$|CONOUT\$|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])$/i.test(
							part.split('.')[0]!.replace(/^ +| +$/g, ''),
						),
				),
	);
const content = z.string().refine((value) => {
	try {
		return btoa(atob(value)) === value;
	} catch {
		return false;
	}
});

/** Public validation only. Rust owns deterministic serialization of sync file records. */
const revision = z
	.string()
	.regex(/^[0-9a-f]{64}$/)
	.nullable();
const common = {
	path: portablePath,
	previousPath: portablePath.nullish().transform((value) => value ?? null),
	baseRevision: revision.optional().transform((value) => value ?? null),
	content: content.nullish().transform((value) => value ?? null),
};
export const syncFileChangeSchema = z
	.union([
		z
			.object({
				version: z.literal(3),
				...common,
				blob: z
					.object({
						id: z.string().regex(/^[0-9a-f]{64}$/),
						size: z
							.number()
							.int()
							.min(1)
							.max(1024 * 1024 * 1024),
						plaintextSize: z.number().int().min(0),
						revision: z.string().regex(/^[0-9a-f]{64}$/),
					})
					.strict(),
			})
			.strict()
			.refine(
				(value) =>
					value.content === null && value.blob.plaintextSize < value.blob.size,
			),
		z.object({ version: z.literal(1), ...common }).strict(),
		z
			.object({
				version: z.literal(2),
				...common,
				acceptedRevisions: z.array(revision).min(1).max(2),
			})
			.strict()
			.refine(
				(value) =>
					value.previousPath === null &&
					value.acceptedRevisions.includes(value.baseRevision) &&
					new Set(value.acceptedRevisions).size ===
						value.acceptedRevisions.length,
			),
	])
	.refine(
		(value) =>
			value.previousPath !== value.path &&
			(value.previousPath === null ||
				value.content !== null ||
				value.version === 3),
	);
