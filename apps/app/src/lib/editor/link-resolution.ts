import type { LinkSuggestion } from '@noura/editor/types';

/** A link target without its `#heading` or `#^block` part. */
export function linkPath(target: string): { path: string; fragment: string } {
	const hash = target.indexOf('#');
	const raw = (hash >= 0 ? target.slice(0, hash) : target).trim();
	let path = raw;
	try {
		// Markdown links percent-encode spaces; wikilinks never do.
		path = decodeURIComponent(raw);
	} catch {
		path = raw;
	}
	return { path, fragment: hash >= 0 ? target.slice(hash + 1) : '' };
}

function folderOf(path: string) {
	const cut = path.lastIndexOf('/');
	return cut === -1 ? '' : path.slice(0, cut);
}

/** Join and normalize `..` and `.` segments; null when it leaves the workspace. */
function joinPath(folder: string, path: string): string | null {
	const parts = folder ? folder.split('/') : [];
	for (const segment of path.split('/')) {
		if (segment === '' || segment === '.') continue;
		if (segment === '..') {
			if (parts.length === 0) return null;
			parts.pop();
		} else parts.push(segment);
	}
	return parts.join('/');
}

function hasExtension(path: string) {
	const name = path.slice(path.lastIndexOf('/') + 1);
	return /\.[a-z0-9]{1,8}$/i.test(name);
}

/**
 * Find the file a link points to, the way Obsidian does: a path relative to
 * the note, then from the workspace root, then any file with that name
 * anywhere, preferring the note's own folder and then the shortest path.
 * `[[Note]]` matches `Note.md`. Matching by name ignores case.
 */
export function resolveLinkPath(
	sourceRelativePath: string,
	target: string,
	files: readonly string[],
): string | null {
	const { path } = linkPath(target);
	if (!path || /^[a-z][a-z\d+.-]*:/i.test(path)) return null;
	const known = new Set(files);
	const variants = (candidate: string) =>
		hasExtension(candidate)
			? [candidate, `${candidate}.md`]
			: [`${candidate}.md`, candidate];
	const sourceFolder = folderOf(sourceRelativePath);
	const direct = path.startsWith('/')
		? [joinPath('', path)]
		: [joinPath(sourceFolder, path), joinPath('', path)];
	for (const candidate of direct) {
		if (candidate === null) continue;
		for (const variant of variants(candidate)) {
			if (known.has(variant)) return variant;
		}
	}
	const wanted = variants(path.replace(/^\/+/, '')).map((variant) =>
		variant.toLowerCase(),
	);
	const matches = files.filter((file) => {
		const lower = file.toLowerCase();
		return wanted.some(
			(variant) => lower === variant || lower.endsWith(`/${variant}`),
		);
	});
	if (matches.length === 0) return null;
	const rank = (file: string) => (folderOf(file) === sourceFolder ? 0 : 1);
	return (
		[...matches].sort(
			(left, right) =>
				rank(left) - rank(right) ||
				left.split('/').length - right.split('/').length ||
				left.localeCompare(right),
		)[0] ?? null
	);
}

/** The path from the note's folder to `path`, for APIs that resolve relatively. */
export function relativeTarget(sourceRelativePath: string, path: string) {
	const from = folderOf(sourceRelativePath).split('/').filter(Boolean);
	const to = path.split('/');
	let shared = 0;
	while (
		shared < from.length &&
		shared < to.length - 1 &&
		from[shared] === to[shared]
	)
		shared += 1;
	return [...from.slice(shared).map(() => '..'), ...to.slice(shared)].join('/');
}

/**
 * `[[` completions: Markdown files by name without `.md`, other files by full
 * name. When two files share a name, the path tells them apart.
 */
export function linkSuggestions(files: readonly string[]): LinkSuggestion[] {
	const nameOf = (file: string) => {
		const name = file.slice(file.lastIndexOf('/') + 1);
		return /\.md$/i.test(name) ? name.slice(0, -3) : name;
	};
	const counts = new Map<string, number>();
	for (const file of files) {
		const name = nameOf(file).toLowerCase();
		counts.set(name, (counts.get(name) ?? 0) + 1);
	}
	return files.map((file) => {
		const label = nameOf(file);
		const unique = counts.get(label.toLowerCase()) === 1;
		const folder = folderOf(file);
		return {
			label,
			target: unique ? label : file.replace(/\.md$/i, ''),
			...(folder ? { detail: folder } : {}),
		};
	});
}
