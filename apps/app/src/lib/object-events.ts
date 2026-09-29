import type { CoreEvent } from '@noura/workspace';

/** Events the native side emits for one managed object. */
export const OBJECT_EVENT_TYPES: ReadonlySet<string> = new Set([
	'object:created',
	'object:updated',
	'object:deleted',
	'object:moved',
]);

/**
 * A bulk external change (a git checkout, a sync pull) arrives as one
 * `objects:changed` event. Its `payload.changes` holds the usual object
 * payloads, each with the original event type in `event`.
 */
export const BULK_OBJECT_EVENT = 'objects:changed';

export interface ObjectEventPayload {
	id?: string;
	type?: string;
	path?: string;
	previousPath?: string | null;
	revision?: string;
}

/**
 * The single-object events an event stands for: the event itself for an
 * `object:*` event, one event per change for `objects:changed`, and none
 * for anything else. Expanded events keep the batch's source, workspace and
 * time, so handlers can treat them like the ones they already know.
 */
export function objectEvents(event: CoreEvent): CoreEvent[] {
	if (OBJECT_EVENT_TYPES.has(event.type)) return [event];
	if (event.type !== BULK_OBJECT_EVENT) return [];
	const changes = (event.payload as { changes?: unknown } | null)?.changes;
	if (!Array.isArray(changes)) return [];
	return changes.flatMap((change, index) => {
		if (!change || typeof change !== 'object') return [];
		const { event: type, ...payload } = change as { event?: unknown };
		if (typeof type !== 'string' || !OBJECT_EVENT_TYPES.has(type)) return [];
		return [
			{
				...event,
				eventId: `${event.eventId}:${index}`,
				type,
				payload,
			} as CoreEvent,
		];
	});
}

/** The event about object `id` within `event`, if there is one. */
export function objectEventFor(event: CoreEvent, id: string): CoreEvent | null {
	return (
		objectEvents(event).find(
			(candidate) => (candidate.payload as ObjectEventPayload)?.id === id,
		) ?? null
	);
}

/** Whether `event` changes, moves or removes the file at `path`. */
export function objectEventTouchesPath(
	event: CoreEvent,
	path: string,
): boolean {
	return objectEvents(event).some((candidate) => {
		const payload = candidate.payload as ObjectEventPayload;
		return payload?.path === path || payload?.previousPath === path;
	});
}

const EDITOR_EVENT_TYPES = new Set([
	'object:updated',
	'object:moved',
	'object:deleted',
]);

/**
 * The change an open editor for object `id` must react to: an update, move
 * or deletion of that object, alone or inside a bulk change. Changes the
 * app made itself are skipped unless `includeApplication` is set, since the
 * editor already knows about its own saves.
 */
export function editorObjectEvent(
	event: CoreEvent,
	id: string,
	options: { includeApplication?: boolean } = {},
): CoreEvent | null {
	if (
		!options.includeApplication &&
		event.source !== 'external' &&
		event.source !== 'reconciliation'
	)
		return null;
	const own = objectEventFor(event, id);
	return own && EDITOR_EVENT_TYPES.has(own.type) ? own : null;
}
