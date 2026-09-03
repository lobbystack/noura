<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { aiChats } from '$lib/ai/chat-store.svelte';
	import { messageLabel } from '$lib/ai/chat-projection';
	import MarkdownPreview from '$lib/components/markdown-preview.svelte';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as ToggleGroup from '$lib/components/ui/toggle-group/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import ArrowCounterClockwise from 'phosphor-svelte/lib/ArrowCounterClockwise';
	import PaperPlaneTilt from 'phosphor-svelte/lib/PaperPlaneTilt';
	import Plus from 'phosphor-svelte/lib/Plus';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import Stop from 'phosphor-svelte/lib/Stop';

	let message = $state('');
	let toolApprovalOpen = $state(false);

	$effect(() => {
		toolApprovalOpen = Boolean(aiChats.pendingToolApproval);
	});

	onMount(() => {
		void aiChats.init();
	});

	async function submit() {
		const value = message.trim();
		if (!value) return;
		message = '';
		await aiChats.send(value);
	}

	function updateRetention(value: string) {
		if (value === 'ephemeral' || value === 'permanent')
			void aiChats.changeRetention(value);
	}

	function toolInput(input: unknown) {
		try {
			return JSON.stringify(input, null, 2) ?? 'null';
		} catch {
			return 'Tool input could not be displayed.';
		}
	}
</script>

<div
	class="flex h-14 shrink-0 items-center justify-between border-b border-border px-4 sm:px-6"
>
	<div class="flex min-w-0 items-center gap-2">
		<h1 class="text-sm font-semibold">AI</h1>
		<Badge variant="secondary">{aiChats.readyProviders.length} ready</Badge>
	</div>
	<div class="flex items-center gap-2">
		<Button
			size="sm"
			variant="outline"
			onclick={() => aiChats.create('ephemeral')}
		>
			<Plus data-icon="inline-start" />
			30-day chat
		</Button>
		<Button size="sm" onclick={() => aiChats.create('permanent')}>
			<Plus data-icon="inline-start" />
			New chat
		</Button>
	</div>
</div>

<AlertDialog.Root
	bind:open={toolApprovalOpen}
	onOpenChange={(open) => {
		if (!open) aiChats.denyToolUse();
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Allow tool use?</AlertDialog.Title>
			<AlertDialog.Description>
				Noura wants to run a tool in this workspace. Approval applies only to
				this call.
			</AlertDialog.Description>
		</AlertDialog.Header>
		{#if aiChats.pendingToolApproval}
			<div class="flex flex-col gap-3 px-4">
				<p class="text-sm">
					<strong>{aiChats.pendingToolApproval.tool.name}</strong>
					from {aiChats.pendingToolApproval.tool.owner}
					<Badge variant="outline" class="ml-2"
						>{aiChats.pendingToolApproval.tool.risk}</Badge
					>
				</p>
				<pre
					class="max-h-48 overflow-auto rounded-md bg-muted p-3 text-xs"><code
						>{toolInput(aiChats.pendingToolApproval.input)}</code
					></pre>
			</div>
		{/if}
		<AlertDialog.Footer>
			<AlertDialog.Cancel onclick={() => aiChats.denyToolUse()}>
				Deny
			</AlertDialog.Cancel>
			<AlertDialog.Action onclick={() => aiChats.allowToolOnce()}>
				Allow once
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<div class="flex min-h-0 flex-1">
	<aside
		class="hidden w-64 shrink-0 border-r border-border md:flex md:flex-col"
		aria-label="Chats"
	>
		<div class="flex items-center justify-between px-4 py-3">
			<span class="text-xs font-medium text-muted-foreground">Chats</span>
			<Button
				size="sm"
				variant="ghost"
				aria-label="New permanent chat"
				onclick={() => aiChats.create('permanent')}
			>
				<Plus />
			</Button>
		</div>
		<div class="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto px-2 pb-3">
			{#each aiChats.chats as chat (chat.id)}
				<button
					type="button"
					class="flex w-full flex-col gap-1 rounded-lg px-3 py-2 text-left text-sm hover:bg-muted aria-[current=true]:bg-muted"
					aria-current={aiChats.selectedChatId === chat.id ? 'true' : undefined}
					onclick={() => aiChats.select(chat.id)}
				>
					<span class="truncate font-medium">{chat.title}</span>
					<span class="text-xs text-muted-foreground">
						{chat.retention === 'ephemeral' ? '30-day retention' : 'Permanent'}
					</span>
				</button>
			{/each}
		</div>
	</aside>

	<section class="flex min-w-0 flex-1 flex-col" aria-label="Chat transcript">
		{#if aiChats.error}
			<Alert.Root variant="destructive" class="m-4 mb-0">
				<Alert.Title>AI request failed</Alert.Title>
				<Alert.Description>{aiChats.error}</Alert.Description>
				{#if !aiChats.running}
					<Alert.Action>
						<Button
							size="sm"
							variant="outline"
							onclick={() =>
								aiChats.conflict && aiChats.selectedChatId
									? aiChats.select(aiChats.selectedChatId)
									: aiChats.retry()}
						>
							<ArrowCounterClockwise data-icon="inline-start" />
							{aiChats.conflict ? 'Reload transcript' : 'Retry'}
						</Button>
					</Alert.Action>
				{/if}
			</Alert.Root>
		{/if}

		{#if aiChats.loading && !aiChats.read}
			<div class="flex flex-1 items-center justify-center">
				<Spinner />
			</div>
		{:else if !aiChats.selectedProvider}
			<Empty.Root class="flex-1">
				<Empty.Header>
					<Empty.Media variant="icon"><Sparkle /></Empty.Media>
					<Empty.Title>No ready AI provider</Empty.Title>
					<Empty.Description>
						Add a provider and its credential in Settings before starting a
						chat.
					</Empty.Description>
				</Empty.Header>
				<Empty.Content>
					<Button onclick={() => goto('/settings')}>Open AI settings</Button>
				</Empty.Content>
			</Empty.Root>
		{:else if !aiChats.read}
			<Empty.Root class="flex-1">
				<Empty.Header>
					<Empty.Media variant="icon"><Sparkle /></Empty.Media>
					<Empty.Title>Start a chat</Empty.Title>
					<Empty.Description>
						Choose permanent storage or a chat eligible for cleanup after 30
						days.
					</Empty.Description>
				</Empty.Header>
				<Empty.Content>
					<Button onclick={() => aiChats.create('permanent')}>
						<Plus data-icon="inline-start" />
						New chat
					</Button>
				</Empty.Content>
			</Empty.Root>
		{:else}
			<div
				class="flex items-center justify-between border-b border-border px-4 py-3 sm:px-6"
			>
				<div class="min-w-0">
					<h2 class="truncate text-sm font-medium">
						{aiChats.read.chat.title}
					</h2>
					<p class="text-xs text-muted-foreground">
						{aiChats.read.chat.retention === 'ephemeral'
							? 'Eligible for cleanup after 30 days'
							: 'Stored permanently in this workspace'}
					</p>
				</div>
				<div class="flex shrink-0 items-center gap-2">
					<ToggleGroup.Root
						type="single"
						variant="outline"
						size="sm"
						value={aiChats.read.chat.retention}
						disabled={aiChats.changingRetention || aiChats.running}
						onValueChange={updateRetention}
						aria-label="Chat retention"
					>
						<ToggleGroup.Item value="permanent">Permanent</ToggleGroup.Item>
						<ToggleGroup.Item value="ephemeral">30 days</ToggleGroup.Item>
					</ToggleGroup.Root>
					<Select.Root
						type="single"
						value={aiChats.selectedProviderId}
						onValueChange={(value) => {
							if (value) aiChats.selectedProviderId = value;
						}}
					>
						<Select.Trigger aria-label="AI provider"
							>{aiChats.selectedProvider.displayName}</Select.Trigger
						>
						<Select.Content>
							<Select.Group>
								{#each aiChats.readyProviders as provider (provider.id)}
									<Select.Item value={provider.id}
										>{provider.displayName} · {provider.model}</Select.Item
									>
								{/each}
							</Select.Group>
						</Select.Content>
					</Select.Root>
				</div>
			</div>

			<div class="min-h-0 flex-1 overflow-y-auto px-4 py-5 sm:px-6">
				<div class="mx-auto flex max-w-3xl flex-col gap-5">
					{#each aiChats.read.messages as item (item.id)}
						<article class="flex flex-col gap-2">
							<div class="flex items-center gap-2">
								<span class="text-xs font-medium text-muted-foreground"
									>{messageLabel(item)}</span
								>
								{#if item.status !== 'completed'}
									<Badge variant="outline">{item.status}</Badge>
								{/if}
							</div>
							{#if item.kind === 'assistant'}
								<MarkdownPreview markdown={item.content} />
							{:else}
								<p class="whitespace-pre-wrap text-sm leading-6">
									{item.content}
								</p>
							{/if}
							{#if item.errorCode}
								<p class="text-xs text-destructive">{item.errorCode}</p>
							{/if}
						</article>
					{/each}
					{#if aiChats.running}
						<article class="flex flex-col gap-2" aria-live="polite">
							<span class="text-xs font-medium text-muted-foreground"
								>Noura is responding</span
							>
							{#if aiChats.streamingText}
								<MarkdownPreview markdown={aiChats.streamingText} />
							{:else}
								<Spinner />
							{/if}
						</article>
					{/if}
				</div>
			</div>

			<form
				class="border-t border-border p-4 sm:px-6"
				onsubmit={(event) => {
					event.preventDefault();
					void submit();
				}}
			>
				<div class="mx-auto flex max-w-3xl flex-col gap-3">
					<Field.Field>
						<Field.FieldLabel for="chat-message">Message</Field.FieldLabel>
						<Textarea
							id="chat-message"
							bind:value={message}
							placeholder="Ask Noura anything"
							disabled={aiChats.running}
							onkeydown={(event) => {
								if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
									event.preventDefault();
									void submit();
								}
							}}
						/>
					</Field.Field>
					<div class="flex items-center justify-between gap-3">
						<p class="text-xs text-muted-foreground">
							Markdown is rendered safely. Cmd+Enter sends.
						</p>
						{#if aiChats.running}
							<Button
								type="button"
								variant="outline"
								onclick={() => aiChats.stop()}
							>
								<Stop data-icon="inline-start" />
								Stop
							</Button>
						{:else}
							<Button type="submit" disabled={!message.trim()}>
								<PaperPlaneTilt data-icon="inline-start" />
								Send
							</Button>
						{/if}
					</div>
				</div>
			</form>
		{/if}
	</section>
</div>
