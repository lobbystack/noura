<script lang="ts">
	import { preferences, SIDEBAR_MIN_WIDTH } from '$lib/preferences.svelte';

	const KEY_STEP = 16;

	let handle = $state<HTMLDivElement | null>(null);
	let dragging = $state(false);
	let maxWidth = $state(SIDEBAR_MIN_WIDTH);
	let start = { x: 0, width: 0 };

	function sidebarElement(): HTMLElement | null {
		return (
			handle?.closest<HTMLElement>('[data-slot="sidebar-container"]') ?? null
		);
	}

	/**
	 * The width that shows every visible file and folder name in full, and
	 * never less than the default width.
	 */
	function fittedWidth(): number {
		const sidebar = sidebarElement();
		if (!sidebar) return SIDEBAR_MIN_WIDTH;
		const box = sidebar.getBoundingClientRect();
		let needed = SIDEBAR_MIN_WIDTH;
		for (const row of sidebar.querySelectorAll<HTMLElement>(
			'[data-tree-path]',
		)) {
			const rect = row.getBoundingClientRect();
			const style = getComputedStyle(row);
			const parts = [...row.children].filter(
				(part) => !part.classList.contains('sr-only'),
			);
			const gap = parseFloat(style.columnGap) || 0;
			const content =
				parts.reduce(
					(sum, part) =>
						sum +
						(part.classList.contains('truncate')
							? part.scrollWidth
							: part.getBoundingClientRect().width),
					0,
				) +
				gap * Math.max(parts.length - 1, 0);
			const width =
				rect.left -
				box.left +
				parseFloat(style.paddingLeft) +
				content +
				parseFloat(style.paddingRight) +
				(box.right - rect.right);
			needed = Math.max(needed, Math.ceil(width));
		}
		// Leave most of the window to the document.
		return Math.min(
			needed,
			Math.max(SIDEBAR_MIN_WIDTH, Math.floor(innerWidth * 0.6)),
		);
	}

	function clamp(width: number) {
		// A sidebar already wider than its names may stay that wide, but
		// can't grow further.
		const max = Math.max(maxWidth, start.width);
		return Math.round(Math.min(Math.max(width, SIDEBAR_MIN_WIDTH), max));
	}

	function setResizing(active: boolean) {
		dragging = active;
		// Width transitions would trail the pointer.
		if (active) document.documentElement.dataset.resizingSidebar = '';
		else delete document.documentElement.dataset.resizingSidebar;
	}

	function handlePointerDown(event: PointerEvent) {
		if (event.button !== 0) return;
		event.preventDefault();
		handle?.setPointerCapture(event.pointerId);
		maxWidth = fittedWidth();
		start = { x: event.clientX, width: preferences.sidebarWidth };
		setResizing(true);
	}

	// Apply at most one width per frame; pointer events can arrive faster
	// than the page can lay out.
	let pendingX: number | null = null;
	let frame = 0;

	function handlePointerMove(event: PointerEvent) {
		if (!dragging) return;
		pendingX = event.clientX;
		frame ||= requestAnimationFrame(applyPending);
	}

	function applyPending() {
		frame = 0;
		if (pendingX === null) return;
		preferences.sidebarWidth = clamp(start.width + pendingX - start.x);
		pendingX = null;
	}

	function handlePointerUp() {
		if (!dragging) return;
		cancelAnimationFrame(frame);
		applyPending();
		setResizing(false);
		preferences.update({ sidebarWidth: preferences.sidebarWidth });
	}

	function resizeTo(width: number) {
		start.width = preferences.sidebarWidth;
		preferences.update({ sidebarWidth: clamp(width) });
	}

	function handleKeydown(event: KeyboardEvent) {
		maxWidth = fittedWidth();
		const width = preferences.sidebarWidth;
		const next =
			event.key === 'ArrowRight'
				? width + KEY_STEP
				: event.key === 'ArrowLeft'
					? width - KEY_STEP
					: event.key === 'Home'
						? SIDEBAR_MIN_WIDTH
						: event.key === 'End'
							? maxWidth
							: null;
		if (next === null) return;
		event.preventDefault();
		resizeTo(next);
	}
</script>

<!-- A focusable separator is the ARIA pattern for a resize handle. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
	bind:this={handle}
	role="separator"
	aria-orientation="vertical"
	aria-label="Resize sidebar"
	aria-valuemin={SIDEBAR_MIN_WIDTH}
	aria-valuemax={Math.max(maxWidth, preferences.sidebarWidth)}
	aria-valuenow={preferences.sidebarWidth}
	tabindex="0"
	title="Drag to resize. Double-click to fit the longest name."
	data-dragging={dragging || undefined}
	class="group/resize absolute inset-y-0 end-0 z-20 flex w-2 cursor-col-resize justify-end outline-none"
	onpointerdown={handlePointerDown}
	onpointermove={handlePointerMove}
	onpointerup={handlePointerUp}
	onpointercancel={handlePointerUp}
	ondblclick={() => {
		maxWidth = fittedWidth();
		preferences.update({ sidebarWidth: maxWidth });
	}}
	onfocus={() => (maxWidth = fittedWidth())}
	onkeydown={handleKeydown}
>
	<div
		class="h-full w-0.5 transition-colors group-hover/resize:bg-sidebar-ring group-focus-visible/resize:bg-sidebar-ring group-data-dragging/resize:bg-sidebar-ring"
	></div>
</div>
