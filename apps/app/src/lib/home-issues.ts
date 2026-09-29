import type { Diagnostic } from '@noura/workspace';

/** A workspace problem, worded for people rather than logs. */
export interface HomeIssue {
	key: string;
	message: string;
	/** Files the problem is about; each gets an Open link. */
	paths: string[];
}

function fileName(path: string): string {
	return path.split('/').pop() || path;
}

/** The files the engine lists in an identity-conflict message. */
function conflictPaths(message: string): string[] {
	const list = message.slice(message.indexOf(':') + 1);
	return list
		.split(',')
		.map((path) => path.trim())
		.filter((path) => path.length > 0);
}

function describe(diagnostic: Diagnostic): Omit<HomeIssue, 'key'> {
	const path = diagnostic.relativePath;
	switch (diagnostic.code) {
		case 'parse_error':
			return {
				message: path
					? `noura can’t read the properties at the top of ${fileName(path)}.`
					: 'noura can’t read the properties of a file.',
				paths: path ? [path] : [],
			};
		case 'identity_conflict': {
			const paths = conflictPaths(diagnostic.message);
			return {
				message:
					paths.length > 1
						? `${paths.map(fileName).join(' and ')} have the same ID. Remove the id line from the copy.`
						: 'Two files have the same ID. Remove the id line from the copy.',
				paths,
			};
		}
		case 'broken_reference':
			return {
				message: path
					? `${fileName(path)} links to something that no longer exists.`
					: 'A file links to something that no longer exists.',
				paths: path ? [path] : [],
			};
		default:
			return { message: diagnostic.message, paths: path ? [path] : [] };
	}
}

/**
 * Plain-language issues for Home, one per distinct problem. The engine can
 * report the same problem twice (a file linking twice to one missing item),
 * which also made list keys collide.
 */
export function homeIssues(diagnostics: readonly Diagnostic[]): HomeIssue[] {
	const issues = new Map<string, HomeIssue>();
	for (const diagnostic of diagnostics) {
		const key = [
			diagnostic.code,
			diagnostic.relativePath ?? '',
			diagnostic.objectId ?? '',
			diagnostic.message,
		].join('\u0000');
		if (!issues.has(key)) issues.set(key, { key, ...describe(diagnostic) });
	}
	return [...issues.values()];
}
