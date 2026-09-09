import { describe, expect, test } from 'bun:test';
import { createPointerSelectionTracker } from './pointer-selection';

function pointer(
	pointerId: number,
	options: Partial<Pick<PointerEvent, 'button' | 'buttons' | 'isPrimary'>> = {},
) {
	return {
		pointerId,
		button: options.button ?? 0,
		buttons: options.buttons ?? 1,
		isPrimary: options.isPrimary ?? true,
	};
}

function harness() {
	const changes: boolean[] = [];
	const frames = new Map<number, FrameRequestCallback>();
	let nextFrame = 1;
	const tracker = createPointerSelectionTracker(
		(active) => changes.push(active),
		{
			request(callback) {
				const handle = nextFrame++;
				frames.set(handle, callback);
				return handle;
			},
			cancel(handle) {
				frames.delete(handle);
			},
		},
	);
	return {
		tracker,
		changes,
		flushFrame() {
			for (const [handle, callback] of frames) {
				frames.delete(handle);
				callback(0);
			}
		},
	};
}

describe('pointer selection tracker', () => {
	test('stays active through a primary drag and ends after the release frame', () => {
		const { tracker, changes, flushFrame } = harness();
		tracker.pointerDown(pointer(1));
		tracker.pointerMove(pointer(1));
		expect(changes).toEqual([true]);

		tracker.pointerUp(pointer(1, { buttons: 0 }));
		expect(changes).toEqual([true]);
		flushFrame();
		expect(changes).toEqual([true, false]);
	});

	test('ignores secondary and unrelated pointers', () => {
		const { tracker, changes, flushFrame } = harness();
		tracker.pointerDown(pointer(1, { button: 2 }));
		tracker.pointerDown(pointer(2));
		tracker.pointerUp(pointer(3, { buttons: 0 }));
		flushFrame();
		expect(changes).toEqual([true]);
	});

	test('recovers a missed release and cancels stale release frames', () => {
		const { tracker, changes, flushFrame } = harness();
		tracker.pointerDown(pointer(1));
		tracker.pointerMove(pointer(1, { buttons: 0 }));
		tracker.pointerDown(pointer(2));
		flushFrame();
		expect(changes).toEqual([true, true]);

		tracker.pointerCancel(pointer(2, { buttons: 0 }));
		expect(changes).toEqual([true, true, false]);
	});

	test('clears active and pending gestures during teardown or window blur', () => {
		const { tracker, changes, flushFrame } = harness();
		tracker.pointerDown(pointer(1));
		tracker.pointerUp(pointer(1, { buttons: 0 }));
		tracker.cancel();
		flushFrame();
		expect(changes).toEqual([true, false]);
	});
});
