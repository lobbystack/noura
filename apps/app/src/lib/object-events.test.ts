import { describe, expect, test } from 'bun:test';
import type { CoreEvent } from '@noura/workspace';
import {
	editorObjectEvent,
	objectEventFor,
	objectEventTouchesPath,
	objectEvents,
} from './object-events';

function event(type: string, payload: unknown): CoreEvent {
	return {
		eventId: 'event',
		type,
		workspaceId: 'workspace',
		occurredAt: '2026-09-28T00:00:00Z',
		source: 'external',
		payload,
	};
}

const bulk = event('objects:changed', {
	changes: [
		{ event: 'object:updated', id: 'note_a', type: 'note', path: 'a.md' },
		{
			event: 'object:moved',
			id: 'note_b',
			type: 'note',
			path: 'new/b.md',
			previousPath: 'b.md',
		},
		{ event: 'object:deleted', id: 'task_c', type: 'task', path: 'c.md' },
		{ event: 'file:changed', id: 'ignored' },
		null,
	],
});

describe('objectEvents', () => {
	test('passes single object events through', () => {
		const single = event('object:updated', { id: 'note_a' });
		expect(objectEvents(single)).toEqual([single]);
	});

	test('ignores events that are not about objects', () => {
		expect(objectEvents(event('file:changed', { paths: ['a.md'] }))).toEqual(
			[],
		);
		expect(objectEvents(event('objects:changed', null))).toEqual([]);
	});

	test('expands a bulk change into the single events it stands for', () => {
		const expanded = objectEvents(bulk);
		expect(expanded.map((change) => change.type)).toEqual([
			'object:updated',
			'object:moved',
			'object:deleted',
		]);
		expect(expanded[1]).toEqual({
			eventId: 'event:1',
			type: 'object:moved',
			workspaceId: 'workspace',
			occurredAt: '2026-09-28T00:00:00Z',
			source: 'external',
			payload: {
				id: 'note_b',
				type: 'note',
				path: 'new/b.md',
				previousPath: 'b.md',
			},
		});
	});
});

describe('objectEventFor', () => {
	test('finds the change for one object inside a bulk change', () => {
		expect(objectEventFor(bulk, 'task_c')?.type).toBe('object:deleted');
		expect(objectEventFor(bulk, 'note_z')).toBeNull();
		expect(
			objectEventFor(event('object:updated', { id: 'note_a' }), 'note_a')?.type,
		).toBe('object:updated');
	});
});

describe('objectEventTouchesPath', () => {
	test('matches the current and the previous path', () => {
		expect(objectEventTouchesPath(bulk, 'a.md')).toBe(true);
		expect(objectEventTouchesPath(bulk, 'b.md')).toBe(true);
		expect(objectEventTouchesPath(bulk, 'new/b.md')).toBe(true);
		expect(objectEventTouchesPath(bulk, 'd.md')).toBe(false);
	});
});

describe('editorObjectEvent', () => {
	test('a bulk external change reaches the editor of each object in it', () => {
		expect(editorObjectEvent(bulk, 'note_a')?.type).toBe('object:updated');
		expect(editorObjectEvent(bulk, 'note_b')?.payload).toMatchObject({
			path: 'new/b.md',
		});
		expect(editorObjectEvent(bulk, 'task_c')?.type).toBe('object:deleted');
		expect(editorObjectEvent(bulk, 'note_z')).toBeNull();
	});

	test('skips the app’s own saves unless the editor asks for them', () => {
		const saved: CoreEvent = {
			...event('object:updated', { id: 'note_a' }),
			source: 'application',
		};
		expect(editorObjectEvent(saved, 'note_a')).toBeNull();
		expect(
			editorObjectEvent(saved, 'note_a', { includeApplication: true })?.type,
		).toBe('object:updated');
		expect(
			editorObjectEvent({ ...bulk, source: 'reconciliation' }, 'note_a')?.type,
		).toBe('object:updated');
	});

	test('ignores creations and other objects', () => {
		expect(
			editorObjectEvent(event('object:created', { id: 'note_a' }), 'note_a'),
		).toBeNull();
		expect(
			editorObjectEvent(event('object:updated', { id: 'note_b' }), 'note_a'),
		).toBeNull();
	});
});
