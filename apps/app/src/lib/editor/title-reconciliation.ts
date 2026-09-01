export type TitleReconciliation =
	{ status: 'resolved'; title: string } | { status: 'conflict' };

export function reconcileNoteTitle(
	baseTitle: string,
	localTitle: string,
	fileTitle: string,
): TitleReconciliation {
	const localChanged = localTitle !== baseTitle;
	const fileChanged = fileTitle !== baseTitle;
	if (localChanged && fileChanged && localTitle !== fileTitle) {
		return { status: 'conflict' };
	}
	return {
		status: 'resolved',
		title: localChanged ? localTitle : fileTitle,
	};
}
