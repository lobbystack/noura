<script lang="ts">
	import * as Field from '$lib/components/ui/field';
	import * as ToggleGroup from '$lib/components/ui/toggle-group';
	import { Switch } from '$lib/components/ui/switch';
	import {
		preferences,
		type LineWidth,
		type TextSize,
	} from '$lib/preferences.svelte';

	const textSizes: { value: TextSize; label: string }[] = [
		{ value: 'small', label: 'Small' },
		{ value: 'default', label: 'Default' },
		{ value: 'large', label: 'Large' },
	];
	const lineWidths: { value: LineWidth; label: string }[] = [
		{ value: 'narrow', label: 'Narrow' },
		{ value: 'default', label: 'Default' },
		{ value: 'full', label: 'Full width' },
	];
</script>

<Field.FieldGroup>
	<Field.Field>
		<Field.FieldLabel id="text-size-label">Text size</Field.FieldLabel>
		<Field.FieldDescription id="text-size-description">
			Changes the text in notes, tasks, and projects on this computer.
		</Field.FieldDescription>
		<ToggleGroup.Root
			type="single"
			variant="outline"
			aria-labelledby="text-size-label"
			aria-describedby="text-size-description"
			value={preferences.textSize}
			onValueChange={(value) => {
				if (textSizes.some((size) => size.value === value))
					preferences.update({ textSize: value as TextSize });
			}}
		>
			{#each textSizes as size (size.value)}
				<ToggleGroup.Item value={size.value}>{size.label}</ToggleGroup.Item>
			{/each}
		</ToggleGroup.Root>
	</Field.Field>

	<Field.Field>
		<Field.FieldLabel id="line-width-label">Line width</Field.FieldLabel>
		<Field.FieldDescription id="line-width-description">
			Shorter lines are easier to read. Full width uses the whole window.
		</Field.FieldDescription>
		<ToggleGroup.Root
			type="single"
			variant="outline"
			aria-labelledby="line-width-label"
			aria-describedby="line-width-description"
			value={preferences.lineWidth}
			onValueChange={(value) => {
				if (lineWidths.some((width) => width.value === value))
					preferences.update({ lineWidth: value as LineWidth });
			}}
		>
			{#each lineWidths as width (width.value)}
				<ToggleGroup.Item value={width.value}>{width.label}</ToggleGroup.Item>
			{/each}
		</ToggleGroup.Root>
	</Field.Field>

	<Field.Field orientation="horizontal">
		<Field.FieldContent>
			<Field.FieldLabel for="spellcheck">Check spelling</Field.FieldLabel>
			<Field.FieldDescription>
				Underline misspelled words while you type.
			</Field.FieldDescription>
		</Field.FieldContent>
		<Switch
			id="spellcheck"
			checked={preferences.spellcheck}
			onCheckedChange={(checked) => preferences.update({ spellcheck: checked })}
		/>
	</Field.Field>
</Field.FieldGroup>
