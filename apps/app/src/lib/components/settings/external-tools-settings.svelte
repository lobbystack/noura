<script lang="ts">
	import { onMount } from 'svelte';
	import type { McpConnection } from '@noura/workspace';
	import { workspace, getNouraClient } from '$lib/state.svelte';
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';
	import Copy from 'phosphor-svelte/lib/Copy';
	import Check from 'phosphor-svelte/lib/Check';
	import Warning from 'phosphor-svelte/lib/Warning';

	const client = getNouraClient();

	let connection = $state.raw<McpConnection | null>(null);
	let loadError = $state('');
	let copied = $state(false);
	let testing = $state(false);
	let testResult = $state<'ok' | 'failed' | null>(null);

	const config = $derived(
		connection
			? JSON.stringify(
					{
						mcpServers: {
							noura: { command: connection.command, args: connection.args },
						},
					},
					null,
					2,
				)
			: '',
	);

	onMount(() => {
		if (!workspace.isReady) return;
		void client.workspaces
			.mcpConnection()
			.then((value) => (connection = value))
			.catch(() => (loadError = 'Couldn’t prepare the connection details.'));
	});

	async function copy() {
		try {
			await navigator.clipboard.writeText(config);
			copied = true;
			setTimeout(() => (copied = false), 2000);
		} catch {
			copied = false;
		}
	}

	async function test() {
		testing = true;
		testResult = null;
		try {
			testResult = (await client.workspaces.testMcpConnection())
				? 'ok'
				: 'failed';
		} catch {
			testResult = 'failed';
		} finally {
			testing = false;
		}
	}
</script>

{#if workspace.isReady}
	<div class="flex flex-col gap-6">
		<p class="text-sm leading-relaxed text-muted-foreground">
			Connect an AI assistant that supports the Model Context Protocol, such as
			Claude Desktop or Cursor. It can then search this workspace, read notes,
			list projects, and create or update notes and tasks.
		</p>

		<Alert.Root>
			<Warning />
			<Alert.Title>The assistant changes files directly</Alert.Title>
			<Alert.Description>
				noura doesn’t ask for approval before an external tool edits notes or
				tasks. Only connect assistants you trust.
			</Alert.Description>
		</Alert.Root>

		<section class="flex flex-col gap-3" aria-labelledby="mcp-config-heading">
			<div class="flex flex-wrap items-end justify-between gap-3">
				<div class="flex flex-col gap-1">
					<h3 id="mcp-config-heading" class="text-sm font-medium">
						Configuration
					</h3>
					<p class="text-sm text-muted-foreground">
						Paste this into your assistant’s MCP settings, then restart it.
					</p>
				</div>
				<Button
					variant="outline"
					size="sm"
					disabled={!connection}
					onclick={copy}
				>
					{#if copied}<Check data-icon="inline-start" />Copied{:else}<Copy
							data-icon="inline-start"
						/>Copy{/if}
				</Button>
			</div>
			{#if connection}
				<pre
					class="overflow-x-auto rounded-lg border bg-muted p-4 font-mono text-xs leading-relaxed select-text">{config}</pre>
			{:else if loadError}
				<p role="alert" class="text-sm text-destructive">{loadError}</p>
			{:else}
				<p role="status" class="text-sm text-muted-foreground">
					Preparing the configuration…
				</p>
			{/if}
			<p class="text-xs text-muted-foreground">
				The configuration points at this workspace and at noura’s location on
				this computer. If you move either, copy it again.
			</p>
		</section>

		<section class="flex flex-col gap-3" aria-labelledby="mcp-test-heading">
			<h3 id="mcp-test-heading" class="text-sm font-medium">Check the setup</h3>
			<div class="flex flex-wrap items-center gap-3">
				<Button
					variant="outline"
					disabled={!connection || testing}
					onclick={test}
				>
					{#if testing}<Spinner data-icon="inline-start" />{/if}
					Test connection
				</Button>
				<p role="status" class="text-sm">
					{#if testResult === 'ok'}
						Working. Your assistant can connect with this configuration.
					{:else if testResult === 'failed'}
						<span class="text-destructive"
							>noura didn’t answer. Restart noura and try again.</span
						>
					{/if}
				</p>
			</div>
		</section>
	</div>
{:else}
	<Empty.Root>
		<Empty.Header>
			<Empty.Title>No workspace open</Empty.Title>
			<Empty.Description
				>Open a workspace to connect an assistant to it.</Empty.Description
			>
		</Empty.Header>
	</Empty.Root>
{/if}
