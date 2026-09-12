import {
	definePlugin,
	type AiContextProvider,
	type PluginContext,
} from '@noura/plugin-sdk';
import type { WorkspaceObject } from '@noura/shared';

const DAY_MS = 24 * 60 * 60 * 1000;

/** Statuses that make a dated object settled rather than upcoming work. */
const settledStatuses: Record<string, ReadonlySet<string>> = {
	task: new Set(['done', 'cancelled']),
	project: new Set(['completed', 'cancelled']),
};

function calendarTimestamp(object: WorkspaceObject): string | null {
	const { due, date, start } = object.properties;
	for (const value of [start, date, due]) {
		if (typeof value === 'string' && value.trim().length > 0) return value;
	}
	return null;
}

function isSettled(object: WorkspaceObject): boolean {
	const statuses = settledStatuses[object.type];
	const status = object.properties.status;
	return (
		statuses !== undefined && typeof status === 'string' && statuses.has(status)
	);
}

/**
 * Outstanding dated objects within a horizon — upcoming work, not a
 * calendar view of everything with a date: settled tasks and projects
 * (done, cancelled, completed) drop out.
 */
export function upcomingCalendarEntries(
	objects: WorkspaceObject[],
	now: Date,
	days = 7,
): Array<{ title: string; content: string; sourceId: string }> {
	const limit = now.getTime() + days * DAY_MS;
	return objects
		.flatMap((object) => {
			if (isSettled(object)) return [];
			const startsAt = calendarTimestamp(object);
			if (!startsAt) return [];
			const start = new Date(
				startsAt.length === 10 ? `${startsAt}T00:00:00Z` : startsAt,
			);
			if (Number.isNaN(start.getTime())) return [];
			if (start.getTime() < now.getTime() - DAY_MS) return [];
			if (start.getTime() > limit) return [];
			return [
				{
					title: object.title,
					content: `${startsAt} (${object.type})`,
					sourceId: object.id,
				},
			];
		})
		.sort((left, right) => left.content.localeCompare(right.content));
}

/**
 * Makes the workspace's dated objects available to configured AI providers
 * through the public capability surface. Nothing leaves the client until a
 * provider the user explicitly enabled consumes it.
 */
const contextProvider: (
	objects: () => Promise<WorkspaceObject[]>,
) => AiContextProvider = (listObjects) => ({
	id: 'calendar.upcoming-week',
	provide() {
		return listObjects().then((objects) =>
			upcomingCalendarEntries(objects, new Date()),
		);
	},
});

interface RegisteredProvider {
	dispose: () => void;
	disposeTool: () => boolean;
}

const registrations = new WeakMap<object, RegisteredProvider>();

export default definePlugin({
	manifest: {
		id: 'calendar',
		name: 'Calendar',
		version: '0.1.0',
		capabilities: [
			'workspace.objects',
			'workspace.events',
			'ai.context',
			'ai.tools',
		],
		platforms: ['desktop'],
	},
	activate(context) {
		const dispose = context.ai.registerContextProvider(
			contextProvider(() => context.objects.list()),
		);
		const disposeTool = context.ai.registerTool(calendarTool(context));
		registrations.set(context, { dispose, disposeTool });
	},
	deactivate(context) {
		const registration = registrations.get(context);
		registration?.dispose();
		registration?.disposeTool();
		registrations.delete(context);
	},
});

function calendarTool(context: PluginContext) {
	return {
		name: 'calendar.upcoming',
		description: 'List outstanding dated workspace objects in the next days.',
		inputSchema: {
			type: 'object',
			properties: { days: { type: 'integer', minimum: 1, maximum: 31 } },
			additionalProperties: false,
		},
		risk: 'low' as const,
		execute: async (input: unknown) => {
			const { days } = (input ?? {}) as { days?: unknown };
			if (
				days !== undefined &&
				(typeof days !== 'number' ||
					!Number.isInteger(days) ||
					days < 1 ||
					days > 31)
			) {
				throw new Error('Calendar days must be an integer between 1 and 31');
			}
			const horizon = days === undefined ? 7 : days;
			return upcomingCalendarEntries(
				await context.objects.list(),
				new Date(),
				horizon,
			);
		},
	};
}
