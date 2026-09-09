<script lang="ts">
	import { setMode, userPrefersMode } from 'mode-watcher';
	import * as Field from '$lib/components/ui/field';
	import * as ToggleGroup from '$lib/components/ui/toggle-group';
	import Sun from 'phosphor-svelte/lib/Sun';
	import Moon from 'phosphor-svelte/lib/Moon';
	import Desktop from 'phosphor-svelte/lib/Desktop';
</script>

<Field.FieldGroup>
	<Field.Field>
		<Field.FieldLabel id="appearance-label">Appearance</Field.FieldLabel>
		<Field.FieldDescription id="appearance-description">
			Use System to match your device’s appearance.
		</Field.FieldDescription>
		<ToggleGroup.Root
			type="single"
			variant="outline"
			aria-labelledby="appearance-label"
			aria-describedby="appearance-description"
			value={userPrefersMode.current}
			onValueChange={(value) => {
				if (value === 'light' || value === 'dark' || value === 'system')
					setMode(value);
			}}
		>
			<ToggleGroup.Item value="light" aria-label="Light appearance"
				><Sun />Light</ToggleGroup.Item
			>
			<ToggleGroup.Item value="dark" aria-label="Dark appearance"
				><Moon />Dark</ToggleGroup.Item
			>
			<ToggleGroup.Item value="system" aria-label="System appearance"
				><Desktop />System</ToggleGroup.Item
			>
		</ToggleGroup.Root>
	</Field.Field>
</Field.FieldGroup>
