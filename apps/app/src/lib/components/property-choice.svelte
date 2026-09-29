<script lang="ts">
	import * as Select from '$lib/components/ui/select';
	import { choiceLabel } from '$lib/property-labels';
	let {
		id,
		label,
		value,
		options,
		disabled = false,
		onchange,
		size = 'default',
		class: className,
	}: {
		id?: string;
		label: string;
		value: string;
		options: readonly string[];
		disabled?: boolean;
		onchange: (value: string) => void;
		size?: 'sm' | 'default';
		class?: string;
	} = $props();
</script>

<Select.Root type="single" {value} {disabled} onValueChange={onchange}>
	<Select.Trigger {id} {size} class={className} aria-label={label}
		>{choiceLabel(value)}</Select.Trigger
	>
	<Select.Content
		><Select.Group>
			{#each options as option (option)}<Select.Item
					value={option}
					label={choiceLabel(option)}>{choiceLabel(option)}</Select.Item
				>{/each}
		</Select.Group></Select.Content
	>
</Select.Root>
