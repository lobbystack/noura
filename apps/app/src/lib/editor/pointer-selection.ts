type PointerSelectionEvent = Pick<
	PointerEvent,
	'button' | 'buttons' | 'isPrimary' | 'pointerId'
>;

interface FrameScheduler {
	request: (callback: FrameRequestCallback) => number;
	cancel: (handle: number) => void;
}

export function createPointerSelectionTracker(
	onChange: (active: boolean) => void,
	scheduler: FrameScheduler = {
		request: (callback) => requestAnimationFrame(callback),
		cancel: (handle) => cancelAnimationFrame(handle),
	},
) {
	let activePointerId: number | null = null;
	let releaseFrame: number | null = null;

	function cancelPendingRelease() {
		if (releaseFrame === null) return;
		scheduler.cancel(releaseFrame);
		releaseFrame = null;
	}

	function pointerDown(event: PointerSelectionEvent) {
		if (!event.isPrimary || event.button !== 0) return;
		cancelPendingRelease();
		activePointerId = event.pointerId;
		onChange(true);
	}

	function pointerUp(event: PointerSelectionEvent) {
		if (event.pointerId !== activePointerId) return;
		activePointerId = null;
		cancelPendingRelease();
		releaseFrame = scheduler.request(() => {
			releaseFrame = null;
			onChange(false);
		});
	}

	function pointerMove(event: PointerSelectionEvent) {
		if (event.pointerId === activePointerId && (event.buttons & 1) === 0) {
			pointerUp(event);
		}
	}

	function pointerCancel(event: PointerSelectionEvent) {
		if (event.pointerId !== activePointerId) return;
		cancel();
	}

	function cancel() {
		activePointerId = null;
		cancelPendingRelease();
		onChange(false);
	}

	return { pointerDown, pointerMove, pointerUp, pointerCancel, cancel };
}
