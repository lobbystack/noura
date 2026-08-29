<script lang="ts">
	import { workspace } from '$lib/state.svelte';
	import { House } from 'phosphor-svelte';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
</script>

<div class="flex flex-1 flex-col items-center justify-center gap-4 p-6">
	<Empty.Root>
		<Empty.Media variant="icon">
			<House />
		</Empty.Media>
		<Empty.Header>
			<Empty.Title>Noura is ready</Empty.Title>
			<Empty.Description>
				The local workspace is open and indexed. Start writing, planning, or
				exploring.
			</Empty.Description>
		</Empty.Header>
		<Empty.Content>
			<Button variant="default" size="sm" href="/tasks">Open tasks</Button>
		</Empty.Content>
	</Empty.Root>
</div>
