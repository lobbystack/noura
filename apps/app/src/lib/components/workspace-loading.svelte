<script lang="ts">
	import { Spinner } from '$lib/components/ui/spinner/index.js';
</script>

<!-- The one loading screen between launch and the workspace. The layout
shows it while the shell code loads and the shell keeps it while the
workspace opens, so the spinner never jumps. It fades in late, so a fast
start shows nothing at all. -->
<div class="loading-delayed flex min-h-0 flex-1 items-center justify-center">
	<Spinner
		class="size-6 text-muted-foreground"
		aria-label="Opening workspace"
	/>
</div>
