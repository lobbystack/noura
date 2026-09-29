<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { getSettingsDialog } from '$lib/settings.svelte';
	import { aiChats } from '$lib/ai/chat-store.svelte';
	import {
		messageErrorText,
		messageLabel,
		messageStatusLabel,
	} from '$lib/ai/chat-projection';
	import { getNouraClient } from '$lib/state.svelte';
	import { cn } from '$lib/utils';
	import ChatMarkdown from '$lib/components/chat-markdown.svelte';
	import * as Alert from '$lib/components/ui/alert/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import ArrowCounterClockwise from 'phosphor-svelte/lib/ArrowCounterClockwise';
	import ChatsCircle from 'phosphor-svelte/lib/ChatsCircle';
	import DotsThree from 'phosphor-svelte/lib/DotsThree';
	import PaperPlaneTilt from 'phosphor-svelte/lib/PaperPlaneTilt';
	import Plus from 'phosphor-svelte/lib/Plus';
	import Sparkle from 'phosphor-svelte/lib/Sparkle';
	import Stop from 'phosphor-svelte/lib/Stop';

	const settings = getSettingsDialog();

	let message = $state('');
	let chatsOpen = $state(false);
	let renaming = $state(false);
	let titleDraft = $state('');

	onMount(() => {
		void aiChats.init();
	});

	async function submit() {
		const value = message.trim();
		if (!value || aiChats.running) return;
		message = '';
		await aiChats.send(value);
	}

	function handleComposerKeydown(event: KeyboardEvent) {
		// Enter sends; Shift+Enter adds a line. Composition keeps IME input intact.
		if (event.key !== 'Enter' || event.shiftKey || event.isComposing) return;
		event.preventDefault();
		void submit();
	}

	async function selectChat(chatId: string) {
		chatsOpen = false;
		await aiChats.select(chatId);
	}

	async function newChat() {
		chatsOpen = false;
		await aiChats.create('permanent');
	}

	function setRetention(value: string) {
		if (value === 'ephemeral' || value === 'permanent')
			void aiChats.changeRetention(value);
	}

	async function startRename() {
		if (!aiChats.read) return;
		titleDraft = aiChats.read.chat.title;
		renaming = true;
		await tick();
	}

	async function commitRename() {
		const chat = aiChats.read?.chat;
		renaming = false;
		if (chat) await aiChats.rename(chat.id, titleDraft);
	}

	function openLink(href: string) {
		void getNouraClient()
			.openLink(href)
			.catch(() => {});
	}

	function toolInput(input: unknown) {
		try {
			return JSON.stringify(input, null, 2) ?? 'null';
		} catch {
			return 'This input can’t be shown.';
		}
	}
</script>

{#snippet chatList()}
	<nav class="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto p-2">
		{#each aiChats.chats as chat (chat.id)}
			<button
				type="button"
				class={cn(
					'flex w-full flex-col gap-0.5 rounded-lg px-3 py-2 text-left text-sm hover:bg-muted',
					aiChats.selectedChatId === chat.id && 'bg-muted',
				)}
				aria-current={aiChats.selectedChatId === chat.id ? 'true' : undefined}
				disabled={aiChats.running}
				onclick={() => selectChat(chat.id)}
			>
				<span class="truncate">{chat.title}</span>
				{#if chat.retention === 'ephemeral'}
					<span class="text-xs text-muted-foreground"
						>Expires after 30 days</span
					>
				{/if}
			</button>
		{:else}
			<p class="px-3 py-2 text-sm text-muted-foreground">No chats yet.</p>
		{/each}
	</nav>
{/snippet}

<div
	class="flex h-14 shrink-0 items-center justify-between gap-2 border-b border-border px-4 sm:px-6"
>
	<div class="flex min-w-0 items-center gap-2">
		<Button
			variant="ghost"
			size="icon-sm"
			class="md:hidden"
			aria-label="Chats"
			onclick={() => (chatsOpen = true)}><ChatsCircle /></Button
		>
		<h1 class="text-sm font-semibold">AI</h1>
	</div>
	<Button size="sm" disabled={aiChats.running} onclick={newChat}>
		<Plus data-icon="inline-start" />
		New chat
	</Button>
</div>

<Sheet.Root bind:open={chatsOpen}>
	<Sheet.Content side="left" class="flex w-72 flex-col gap-0 p-0">
		<Sheet.Header class="border-b border-border">
			<Sheet.Title>Chats</Sheet.Title>
		</Sheet.Header>
		{@render chatList()}
	</Sheet.Content>
</Sheet.Root>

<AlertDialog.Root
	bind:open={
		() => Boolean(aiChats.pendingToolApproval),
		(open) => {
			if (!open) aiChats.denyToolUse();
		}
	}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Allow this tool?</AlertDialog.Title>
			<AlertDialog.Description>
				noura wants to run a tool in this workspace. Your answer applies to this
				call only.
			</AlertDialog.Description>
		</AlertDialog.Header>
		{#if aiChats.pendingToolApproval}
			<div class="flex flex-col gap-3">
				<p class="text-sm">
					<strong>{aiChats.pendingToolApproval.tool.name}</strong>
					from {aiChats.pendingToolApproval.tool.owner}
					{#if aiChats.pendingToolApproval.tool.risk !== 'low'}
						<Badge variant="outline" class="ml-2"
							>{aiChats.pendingToolApproval.tool.risk} risk</Badge
						>
					{/if}
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
		{@render chatList()}
	</aside>

	<section class="flex min-w-0 flex-1 flex-col" aria-label="Chat">
		{#if aiChats.error}
			<Alert.Root variant="destructive" class="m-4 mb-0">
				<Alert.Title>Something went wrong</Alert.Title>
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
							{aiChats.conflict ? 'Reload chat' : 'Try again'}
						</Button>
					</Alert.Action>
				{/if}
			</Alert.Root>
		{/if}

		{#if aiChats.loading && !aiChats.read}
			<div class="flex flex-1 items-center justify-center">
				<Spinner class="loading-delayed" />
			</div>
		{:else if !aiChats.selectedProvider}
			<Empty.Root class="flex-1">
				<Empty.Header>
					<Empty.Media variant="icon"><Sparkle /></Empty.Media>
					<Empty.Title>Connect an AI provider</Empty.Title>
					<Empty.Description>
						Add a provider and its key to start chatting.
					</Empty.Description>
				</Empty.Header>
				<Empty.Content>
					<Button onclick={() => settings.show('ai')}>Open AI settings</Button>
				</Empty.Content>
			</Empty.Root>
		{:else if !aiChats.read}
			<Empty.Root class="flex-1">
				<Empty.Header>
					<Empty.Media variant="icon"><Sparkle /></Empty.Media>
					<Empty.Title>Start a chat</Empty.Title>
				</Empty.Header>
				<Empty.Content>
					<Button onclick={newChat}>
						<Plus data-icon="inline-start" />
						New chat
					</Button>
				</Empty.Content>
			</Empty.Root>
		{:else}
			<div
				class="flex items-center justify-between gap-3 border-b border-border px-4 py-2 sm:px-6"
			>
				{#if renaming}
					<form
						class="min-w-0 flex-1"
						onsubmit={(event) => {
							event.preventDefault();
							void commitRename();
						}}
					>
						<Input
							bind:value={titleDraft}
							aria-label="Chat name"
							class="h-8"
							onblur={() => void commitRename()}
							onkeydown={(event) => {
								if (event.key === 'Escape') {
									event.preventDefault();
									renaming = false;
								}
							}}
							{@attach (node: HTMLInputElement) => node.select()}
						/>
					</form>
				{:else}
					<div class="min-w-0">
						<h2 class="truncate text-sm font-medium">
							{aiChats.read.chat.title}
						</h2>
						{#if aiChats.read.chat.retention === 'ephemeral'}
							<p class="text-xs text-muted-foreground">Expires after 30 days</p>
						{/if}
					</div>
				{/if}
				<div class="flex shrink-0 items-center gap-2">
					{#if aiChats.readyProviders.length > 1}
						<Select.Root
							type="single"
							value={aiChats.selectedProviderId}
							onValueChange={(value) => {
								if (value) aiChats.selectedProviderId = value;
							}}
						>
							<Select.Trigger size="sm" aria-label="AI provider"
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
					{/if}
					<DropdownMenu.Root>
						<DropdownMenu.Trigger>
							{#snippet child({ props })}
								<Button
									{...props}
									variant="ghost"
									size="icon-sm"
									aria-label="Chat options"
									disabled={aiChats.running}><DotsThree /></Button
								>
							{/snippet}
						</DropdownMenu.Trigger>
						<DropdownMenu.Content align="end" class="w-52">
							<DropdownMenu.Group>
								<DropdownMenu.Item onSelect={() => void startRename()}
									>Rename</DropdownMenu.Item
								>
							</DropdownMenu.Group>
							<DropdownMenu.Separator />
							<DropdownMenu.RadioGroup
								value={aiChats.read.chat.retention}
								onValueChange={setRetention}
							>
								<DropdownMenu.GroupHeading
									>Keep this chat</DropdownMenu.GroupHeading
								>
								<DropdownMenu.RadioItem
									value="permanent"
									disabled={aiChats.changingRetention}
									>Until I delete it</DropdownMenu.RadioItem
								>
								<DropdownMenu.RadioItem
									value="ephemeral"
									disabled={aiChats.changingRetention}
									>For 30 days</DropdownMenu.RadioItem
								>
							</DropdownMenu.RadioGroup>
						</DropdownMenu.Content>
					</DropdownMenu.Root>
				</div>
			</div>

			<div class="min-h-0 flex-1 overflow-y-auto px-4 py-5 sm:px-6">
				<div class="mx-auto flex max-w-3xl flex-col gap-5">
					{#each aiChats.read.messages as item (item.id)}
						{@const status = messageStatusLabel(item)}
						{@const problem = messageErrorText(item)}
						<article class="flex flex-col gap-2">
							<div class="flex items-center gap-2">
								<span class="text-xs font-medium text-muted-foreground"
									>{messageLabel(item)}</span
								>
								{#if status}
									<Badge variant="outline">{status}</Badge>
								{/if}
							</div>
							{#if item.kind === 'assistant'}
								<ChatMarkdown markdown={item.content} onlink={openLink} />
							{:else}
								<p class="text-sm leading-6 whitespace-pre-wrap">
									{item.content}
								</p>
							{/if}
							{#if problem}
								<p class="text-xs text-destructive">{problem}</p>
							{/if}
						</article>
					{/each}
					{#if aiChats.running}
						<article class="flex flex-col gap-2" aria-live="polite">
							<span class="text-xs font-medium text-muted-foreground"
								>noura</span
							>
							{#if aiChats.streamingText}
								<ChatMarkdown
									markdown={aiChats.streamingText}
									streaming
									onlink={openLink}
								/>
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
				<div class="mx-auto flex max-w-3xl items-end gap-2">
					<label for="chat-message" class="sr-only">Message</label>
					<Textarea
						id="chat-message"
						bind:value={message}
						placeholder="Ask noura"
						rows={1}
						class="max-h-48 min-h-10 flex-1"
						onkeydown={handleComposerKeydown}
					/>
					{#if aiChats.running}
						<Button
							type="button"
							variant="outline"
							size="icon"
							aria-label="Stop"
							onclick={() => aiChats.stop()}
						>
							<Stop />
						</Button>
					{:else}
						<Button
							type="submit"
							size="icon"
							aria-label="Send"
							disabled={!message.trim()}
						>
							<PaperPlaneTilt />
						</Button>
					{/if}
				</div>
			</form>
		{/if}
	</section>
</div>
