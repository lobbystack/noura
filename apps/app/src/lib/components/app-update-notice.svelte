<script lang="ts">
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import {
		createAppUpdater,
		createTauriAppUpdater,
		type AppUpdateState,
	} from '@noura/workspace';
	import { getAppPlatform } from '$lib/platform';
	import { flushPendingDrafts } from '$lib/editor/pending-drafts.svelte';

	const FIRST_CHECK_DELAY_MS = 5_000;
	const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
	const TOAST_ID = 'app-update';

	onMount(() => {
		// Development builds and non-desktop hosts have no signed release to
		// compare against.
		if (import.meta.env.DEV || getAppPlatform() !== 'desktop') return;

		const updater = createAppUpdater(createTauriAppUpdater(), {
			flush: flushPendingDrafts,
			onChange: show,
		});

		function show(state: AppUpdateState) {
			if (state.status === 'ready') {
				toast(`Noura ${state.version} is ready`, {
					id: TOAST_ID,
					description:
						state.error ??
						'Restart to finish updating. It takes a few seconds.',
					duration: Number.POSITIVE_INFINITY,
					action: {
						label: 'Restart',
						onClick: () => void updater.installAndRestart(),
					},
				});
			} else if (state.status === 'installing') {
				toast.loading(`Installing Noura ${state.version}…`, {
					id: TOAST_ID,
					duration: Number.POSITIVE_INFINITY,
				});
			}
		}

		const first = setTimeout(() => void updater.check(), FIRST_CHECK_DELAY_MS);
		const interval = setInterval(() => void updater.check(), CHECK_INTERVAL_MS);
		return () => {
			clearTimeout(first);
			clearInterval(interval);
		};
	});
</script>
