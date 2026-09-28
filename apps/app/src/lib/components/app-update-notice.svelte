<script lang="ts">
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { getAppUpdates } from '$lib/app-updates.svelte';

	const FIRST_CHECK_DELAY_MS = 5_000;
	const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
	const TOAST_ID = 'app-update';

	const updates = getAppUpdates();

	// Mirror the updater's state into the toast, a DOM-external surface.
	$effect(() => {
		const state = updates.state;
		if (state.status === 'ready') {
			toast(`noura ${state.version} is ready`, {
				id: TOAST_ID,
				description: state.error ?? 'Restart to finish updating.',
				duration: Number.POSITIVE_INFINITY,
				action: {
					label: 'Restart',
					onClick: () => void updates.installAndRestart(),
				},
			});
		} else if (state.status === 'installing') {
			toast.loading(`Installing noura ${state.version}…`, {
				id: TOAST_ID,
				duration: Number.POSITIVE_INFINITY,
			});
		}
	});

	onMount(() => {
		if (!updates.supported) return;
		const first = setTimeout(() => void updates.check(), FIRST_CHECK_DELAY_MS);
		const interval = setInterval(() => void updates.check(), CHECK_INTERVAL_MS);
		return () => {
			clearTimeout(first);
			clearInterval(interval);
		};
	});
</script>
