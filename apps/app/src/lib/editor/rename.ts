import { isCoreError } from '@noura/workspace';

export interface FileNameParts {
	/** Folder path without a trailing slash; empty at the workspace root. */
	folder: string;
	/** The name shown and edited: without `.md`, and without any extension. */
	stem: string;
	/** Extension including the dot, such as `.md`; empty when there is none. */
	extension: string;
}

export function splitFileName(relativePath: string): FileNameParts {
	const cut = relativePath.lastIndexOf('/');
	const folder = cut === -1 ? '' : relativePath.slice(0, cut);
	const name = relativePath.slice(cut + 1);
	const dot = name.lastIndexOf('.');
	if (dot <= 0) return { folder, stem: name, extension: '' };
	return { folder, stem: name.slice(0, dot), extension: name.slice(dot) };
}

/**
 * Turn what was typed into a safe file name: no path separators or
 * characters Windows and macOS reject, no leading dots (which would hide
 * the file), and no surrounding spaces. Normalized only on commit, never
 * while typing.
 */
export function normalizeFileName(input: string): string {
	return (
		input
			// eslint-disable-next-line no-control-regex
			.replace(/[\u0000-\u001f\u007f/\\:*?"<>|]/g, '')
			.trim()
			.replace(/^\.+/, '')
			.trim()
			.slice(0, 200)
	);
}

/** The new path for a rename, or null when nothing changes. */
export function renamedPath(
	relativePath: string,
	typed: string,
): string | null {
	const name = normalizeFileName(typed);
	const parts = splitFileName(relativePath);
	if (!name || name === parts.stem) return null;
	const prefix = parts.folder ? `${parts.folder}/` : '';
	return `${prefix}${name}${parts.extension}`;
}

/** Plain words for a failed rename. */
export function renameErrorMessage(error: unknown): string {
	if (isCoreError(error)) {
		if (error.code === 'path_exists')
			return 'A file with that name is already in this folder.';
		if (error.code === 'collaboration_transaction_required')
			return 'This file is shared right now. Rename it after sharing ends.';
	}
	return 'Couldn’t rename the file. Try again.';
}
