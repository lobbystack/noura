/** Page zoom steps, as in a browser. */
export const ZOOM_LEVELS = [
	0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2,
] as const;

/** The zoom step after `current`, one level in `direction`. */
export function nextZoom(current: number, direction: 1 | -1): number {
	const index = ZOOM_LEVELS.findIndex((level) => level === current);
	const from = index === -1 ? ZOOM_LEVELS.indexOf(1) : index;
	const next = Math.min(Math.max(from + direction, 0), ZOOM_LEVELS.length - 1);
	return ZOOM_LEVELS[next]!;
}
