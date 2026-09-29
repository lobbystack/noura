<script lang="ts">
	import { afterNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { tabHref, tabsStore } from '$lib/tabs.svelte';
	import { activateTab, closeTab } from '$lib/tab-actions';
	import { onMount } from 'svelte';
	import { lastRoute } from '$lib/last-route';
	import { workspace } from '$lib/state.svelte';
	import { cn } from '$lib/utils.js';
	import type { Attachment } from 'svelte/attachments';
	import X from 'phosphor-svelte/lib/X';

	// Leave room for the window buttons when the sidebar is closed.
	const MIN_LEFT = 80;
	let left = $state(MIN_LEFT);

	// The bar sits in the title bar row, lined up with the content column
	// below it, wherever the rail and sidebar currently end. Pages without a
	// sidebar put the workspace switcher in the title bar; the bar starts
	// after it.
	const followContent: Attachment<HTMLElement> = (anchor) => {
		const content = anchor.parentElement ?? anchor;
		const titleBar = document.querySelector('header[data-tauri-drag-region]');
		const measure = () => {
			const occupied = [...(titleBar?.children ?? [])].map(
				(child) => child.getBoundingClientRect().right,
			);
			left = Math.round(
				Math.max(MIN_LEFT, anchor.getBoundingClientRect().left, ...occupied),
			);
		};
		measure();
		const resizes = new ResizeObserver(measure);
		resizes.observe(content);
		const changes = new MutationObserver(measure);
		if (titleBar) changes.observe(titleBar, { childList: true });
		window.addEventListener('resize', measure);
		return () => {
			resizes.disconnect();
			changes.disconnect();
			window.removeEventListener('resize', measure);
		};
	};

	function arrived(url: URL) {
		const redirect = lastRoute.arrived(workspace.state?.workspaceId, url);
		if (redirect) void goto(redirect, { replaceState: true });
	}

	// The bar mounts once the workspace is ready, after the navigation that
	// opened the app has finished, so it handles that page on mount.
	onMount(() => arrived(page.url));
	afterNavigate(({ to }) => {
		if (to) arrived(to.url);
	});

	// The bar has no visible scrollbar, which would squeeze the tabs in the
	// title bar row. A mouse wheel scrolls it sideways; trackpads already do.
	// The listener is not passive, so it can stop the page from scrolling.
	const scrollTabs: Attachment<HTMLElement> = (bar) => {
		const scroll = (event: WheelEvent) => {
			if (bar.scrollWidth <= bar.clientWidth) return;
			if (Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
			event.preventDefault();
			bar.scrollLeft += event.deltaY;
		};
		bar.addEventListener('wheel', scroll, { passive: false });
		return () => bar.removeEventListener('wheel', scroll);
	};

	function focusTab(index: number) {
		const tab = tabsStore.tabs[index];
		if (tab) document.getElementById(`tab-${tab.id}`)?.focus();
	}

	function handleKeydown(event: KeyboardEvent, index: number, id: string) {
		const last = tabsStore.tabs.length - 1;
		const moves: Record<string, number> = {
			ArrowRight: index === last ? 0 : index + 1,
			ArrowLeft: index === 0 ? last : index - 1,
			Home: 0,
			End: last,
		};
		if (event.key in moves) {
			event.preventDefault();
			focusTab(moves[event.key]!);
		} else if (event.key === 'Delete' || event.key === 'Backspace') {
			event.preventDefault();
			void closeTab(id).then(() =>
				focusTab(Math.min(index, tabsStore.tabs.length - 1)),
			);
		}
	}

	// Typing in the open document keeps its preview tab open. Key presses
	// count too: editors built on EditContext fire no input events.
	function isTypingKey(event: KeyboardEvent) {
		if (event.metaKey || event.ctrlKey || event.altKey) return false;
		return (
			event.key.length === 1 ||
			event.key === 'Backspace' ||
			event.key === 'Delete' ||
			event.key === 'Enter'
		);
	}

	function keepEditedTab(event: Event) {
		const target = event.target;
		if (!(target instanceof HTMLElement) || !target.closest('main')) return;
		if (
			event instanceof KeyboardEvent &&
			(!isTypingKey(event) ||
				!(
					target.isContentEditable ||
					target instanceof HTMLInputElement ||
					target instanceof HTMLTextAreaElement
				))
		)
			return;
		if (target.closest('[role="tablist"]')) return;
		const active = tabsStore.active;
		if (!active?.preview) return;
		const href = new URL(tabHref(active), page.url);
		if (href.pathname === page.url.pathname) tabsStore.keep(active.id);
	}
</script>

<svelte:document oninput={keepEditedTab} onkeydown={keepEditedTab} />

<div aria-hidden="true" class="h-0" {@attach followContent}></div>

{#if tabsStore.tabs.length > 0}
	<div
		role="tablist"
		aria-label="Open files"
		class="fixed top-0 right-0 z-20 flex h-(--app-titlebar-height) min-w-0 items-center gap-0.5 overflow-x-auto overflow-y-hidden border-b border-sidebar-border bg-sidebar px-2 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
		style="left: {left}px"
		data-tauri-drag-region
		{@attach scrollTabs}
	>
		{#each tabsStore.tabs as tab, index (tab.id)}
			{@const isActive = tabsStore.activeId === tab.id}
			<div
				role="presentation"
				class={cn(
					'group flex h-7 shrink-0 items-center gap-1 rounded-lg pr-1 pl-2.5 text-sm transition-colors',
					isActive
						? 'bg-background text-foreground shadow-sm ring-1 ring-border'
						: 'text-muted-foreground hover:bg-background hover:text-foreground',
				)}
				onmousedown={(event) => {
					// Middle-click closes instead of starting auto-scroll.
					if (event.button === 1) event.preventDefault();
				}}
				onmouseup={(event) => {
					// On Linux a middle-click release pastes the last selected text
					// into the focused note; closing a tab must not do that.
					if (event.button === 1) event.preventDefault();
				}}
				onauxclick={(event) => {
					if (event.button !== 1) return;
					event.preventDefault();
					void closeTab(tab.id);
				}}
			>
				<button
					role="tab"
					id="tab-{tab.id}"
					aria-selected={isActive}
					tabindex={isActive || (!tabsStore.activeId && index === 0) ? 0 : -1}
					title={tab.preview ? `${tab.title} (preview)` : tab.title}
					class={cn(
						'max-w-40 truncate rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring',
						tab.preview && 'italic',
					)}
					onclick={() => void activateTab(tab.id)}
					ondblclick={() => tabsStore.keep(tab.id)}
					onkeydown={(event) => handleKeydown(event, index, tab.id)}
				>
					{tab.title}
				</button>
				<button
					type="button"
					tabindex={-1}
					aria-label="Close {tab.title}"
					class={cn(
						'flex size-4 shrink-0 items-center justify-center rounded-sm text-muted-foreground/60 hover:text-foreground',
						!isActive && 'opacity-0 group-hover:opacity-100',
					)}
					onclick={() => void closeTab(tab.id)}
				>
					<X />
				</button>
			</div>
		{/each}
	</div>
{/if}
