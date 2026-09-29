import { isCoreError } from '@noura/workspace';

/** Why a save failed, in words that tell the user what to do. */
export function saveErrorMessage(error: unknown): string {
	if (isCoreError(error)) {
		switch (error.code) {
			case 'object_not_found':
			case 'raw_markdown_missing':
				return 'The file was moved or deleted outside noura.';
			case 'invalid_utf8':
				return 'This file isn’t text noura can read.';
			case 'revision_conflict':
				return 'The file changed while saving. Try again.';
			case 'workspace_not_open':
				return 'The workspace isn’t open.';
		}
		if (error.category === 'permission')
			return 'noura isn’t allowed to write to this file. Check its permissions.';
		if (error.category === 'filesystem')
			return 'noura couldn’t write to the disk. Check that it has free space.';
		return error.message;
	}
	if (error instanceof Error && error.message) return error.message;
	return 'Something went wrong while saving. Try again.';
}
