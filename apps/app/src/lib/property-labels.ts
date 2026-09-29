const LABELS: Record<string, string> = {
	todo: 'To do',
	'in-progress': 'In progress',
	'on-hold': 'On hold',
};

/**
 * A property value as words: "in-progress" reads "In progress", "todo"
 * reads "To do". Values are stored as IDs; people see labels.
 */
export function choiceLabel(value: string): string {
	const known = LABELS[value];
	if (known) return known;
	const words = value.replaceAll('-', ' ');
	return words.charAt(0).toUpperCase() + words.slice(1);
}
