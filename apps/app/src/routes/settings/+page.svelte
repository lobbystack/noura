<script lang="ts">
	import { getNouraClient, workspace, diagnostics } from '$lib/state.svelte';
	import { aiChats } from '$lib/ai/chat-store.svelte';
	import { AI_POLICY_VERSION } from '$lib/ai/policy';
	import type { AiConsentGrant } from '@noura/workspace';
	import { plugins } from '$lib/plugins.svelte';
	import EmptyState from '$lib/components/empty-state.svelte';
	import SyncAccountSettings from '$lib/components/sync-account-settings.svelte';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { browser } from '$app/environment';
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import GearSix from 'phosphor-svelte/lib/GearSix';

	let providers = $state<
		Awaited<
			ReturnType<ReturnType<typeof getNouraClient>['ai']['listProviders']>
		>
	>([]);
	let loadingProviders = $state(false);
	let toggling = $state('');
	let providerDialogOpen = $state(false);
	let editingProvider = $state<(typeof providers)[number] | null>(null);
	let providerId = $state('');
	let providerKind = $state('');
	let providerName = $state('');
	let providerModel = $state('');
	let providerEndpoint = $state('');
	let providerEnabled = $state(true);
	let credential = $state('');
	let savingProvider = $state(false);
	let providerError = $state<string | null>(null);
	let consents = $state<Record<string, AiConsentGrant | null>>({});
	let changingConsent = $state('');
	const firstParty = [
		'ai',
		'folders',
		'notes',
		'tasks',
		'calendar',
		'projects',
	];

	onMount(async () => {
		if (!browser) return;
		loadingProviders = true;
		providers = await getNouraClient()
			.ai.listProviders()
			.catch(() => []);
		await aiChats.refresh();
		await refreshConsents();
		loadingProviders = false;
		await plugins.init();
	});

	async function rebuildIndex() {
		try {
			await getNouraClient().workspaces.rebuildIndex();
			await workspace.refresh();
		} catch {}
	}

	async function setPluginEnabled(pluginId: string, enabled: boolean) {
		toggling = pluginId;
		try {
			await plugins.setEnabled(pluginId, enabled);
			toast.success(enabled ? `Enabled ${pluginId}` : `Disabled ${pluginId}`);
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not update plugins',
			);
			await plugins.sync();
		} finally {
			toggling = '';
		}
	}

	function openProvider(provider?: (typeof providers)[number]) {
		editingProvider = provider ?? null;
		providerId = provider?.id ?? '';
		providerKind = provider?.kind ?? '';
		providerName = provider?.displayName ?? '';
		providerModel = provider?.model ?? '';
		providerEndpoint = provider?.endpoint ?? '';
		providerEnabled = provider?.enabled ?? true;
		credential = '';
		providerError = null;
		providerDialogOpen = true;
	}

	async function refreshProviders() {
		providers = await getNouraClient().ai.listProviders();
		await aiChats.refresh();
		await refreshConsents();
	}

	async function refreshConsents() {
		try {
			const reads = await Promise.all(
				providers.map(
					async (provider) =>
						[
							provider.id,
							await getNouraClient().ai.readConsent({
								providerId: provider.id,
								policyVersion: AI_POLICY_VERSION,
							}),
						] as const,
				),
			);
			consents = Object.fromEntries(reads);
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not read AI consent.',
			);
		}
	}

	async function grantConsent(providerId: string) {
		changingConsent = providerId;
		try {
			await getNouraClient().ai.grantConsent({
				providerId,
				policyVersion: AI_POLICY_VERSION,
				dataCategory: 'workspace-content',
			});
			await refreshConsents();
			toast.success('Workspace content consent granted');
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not grant consent.',
			);
		} finally {
			changingConsent = '';
		}
	}

	async function revokeConsent(providerId: string) {
		changingConsent = providerId;
		try {
			const outcome = await getNouraClient().ai.revokeConsent({
				providerId,
				policyVersion: AI_POLICY_VERSION,
			});
			await refreshConsents();
			toast.success(
				outcome.cancelledOperations > 0
					? `Consent revoked and ${outcome.cancelledOperations} active request${outcome.cancelledOperations === 1 ? '' : 's'} cancelled`
					: 'Workspace content consent revoked',
			);
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not revoke consent.',
			);
		} finally {
			changingConsent = '';
		}
	}

	async function saveProvider() {
		if (!providerId.trim() || !providerKind.trim() || !providerModel.trim()) {
			providerError = 'Provider ID, kind, and model are required.';
			return;
		}
		savingProvider = true;
		providerError = null;
		try {
			const config = {
				id: providerId.trim(),
				kind: providerKind.trim(),
				displayName: providerName.trim() || providerId.trim(),
				model: providerModel.trim(),
				endpoint: providerEndpoint.trim() || null,
				credentialRef: editingProvider?.credentialRef ?? null,
				enabled: providerEnabled,
			};
			await getNouraClient().ai.saveProvider(config);
			if (credential.trim()) {
				const result = await getNouraClient().ai.setCredential({
					providerId: config.id,
					secret: credential,
				});
				await getNouraClient().ai.saveProvider({
					...config,
					credentialRef: result.credentialRef,
				});
			}
			await refreshProviders();
			providerDialogOpen = false;
			toast.success(editingProvider ? 'Provider updated' : 'Provider added');
		} catch (error) {
			providerError =
				error instanceof Error ? error.message : 'Could not save provider.';
		} finally {
			savingProvider = false;
		}
	}

	async function setProviderEnabled(
		provider: (typeof providers)[number],
		enabled: boolean,
	) {
		try {
			await getNouraClient().ai.saveProvider({ ...provider, enabled });
			await refreshProviders();
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not update provider.',
			);
		}
	}

	async function deleteCredential(provider: (typeof providers)[number]) {
		if (!provider.credentialRef) return;
		try {
			await getNouraClient().ai.deleteCredential({
				credentialRef: provider.credentialRef,
			});
			await getNouraClient().ai.saveProvider({
				...provider,
				credentialRef: null,
			});
			await refreshProviders();
			toast.success('Credential removed');
		} catch (error) {
			toast.error(
				error instanceof Error ? error.message : 'Could not remove credential.',
			);
		}
	}
</script>

<div class="flex h-14 shrink-0 items-center border-b border-border px-6">
	<h1 class="text-sm font-semibold">Settings</h1>
</div>

<div class="flex-1 overflow-y-auto p-6">
	<div class="mx-auto max-w-2xl space-y-8">
		<SyncAccountSettings />
		<section>
			<h2 class="text-sm font-medium">Workspace</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Current workspace and diagnostics
			</p>
			<Separator class="my-4" />
			{#if workspace.state}
				<div class="space-y-3">
					<div class="flex items-center justify-between text-sm">
						<span class="text-muted-foreground">Path</span>
						<span class="truncate font-mono text-xs"
							>{workspace.state.rootPath}</span
						>
					</div>
					<div class="flex items-center justify-between text-sm">
						<span class="text-muted-foreground">Indexed files</span>
						<Badge variant="secondary">{workspace.state.indexedFiles}</Badge>
					</div>
					<div class="flex items-center justify-between text-sm">
						<span class="text-muted-foreground">Issues</span>
						<Badge
							variant={diagnostics.issues.length > 0
								? 'destructive'
								: 'secondary'}>{diagnostics.issues.length}</Badge
						>
					</div>
					<div class="pt-2">
						<Button variant="outline" size="sm" onclick={rebuildIndex}
							>Rebuild index</Button
						>
					</div>
				</div>
			{:else}
				<p class="text-sm text-muted-foreground">No workspace open.</p>
			{/if}
		</section>

		<section>
			<h2 class="text-sm font-medium">Plugins</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Every change rewrites workspace.yaml in your folder
			</p>
			<Separator class="my-4" />
			{#if workspace.isIdle}
				<p class="text-sm text-muted-foreground">
					Open a workspace to manage plugins.
				</p>
			{:else}
				<div class="flex flex-col gap-2">
					{#each firstParty as pluginId (pluginId)}
						{@const active = plugins.activeManifests.find(
							(m) => m.id === pluginId,
						)}
						<div
							class="flex items-center justify-between rounded-lg border border-border p-3"
						>
							<div class="min-w-0">
								<div class="flex items-center gap-2">
									<span class="text-sm font-medium">
										{active?.name ?? pluginId}
									</span>
									{#if active}
										<Badge variant="secondary" class="text-xs"
											>{active.version}</Badge
										>
									{:else if plugins.enabledIds.includes(pluginId)}
										<Badge variant="outline" class="text-xs"
											>Pending restart</Badge
										>
									{/if}
								</div>
								<div class="mt-1 flex flex-wrap gap-1">
									{#each active?.capabilities ?? [] as capability (capability)}
										<Badge variant="outline" class="text-[10px]"
											>{capability}</Badge
										>
									{/each}
								</div>
							</div>
							<Switch
								checked={plugins.enabledIds.includes(pluginId)}
								disabled={toggling !== ''}
								aria-label="Toggle {active?.name ?? pluginId} plugin"
								onCheckedChange={(checked) =>
									setPluginEnabled(pluginId, checked)}
							/>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section>
			<div class="flex items-start justify-between gap-4">
				<div>
					<h2 class="text-sm font-medium">AI Providers</h2>
					<p class="mt-1 text-xs text-muted-foreground">
						Bring your own key. Keys stay in the device credential store.
					</p>
				</div>
				<Button size="sm" onclick={() => openProvider()}>Add provider</Button>
			</div>
			<Separator class="my-4" />
			{#if loadingProviders}
				<p class="text-sm text-muted-foreground">Loading…</p>
			{:else if providers.length === 0}
				<EmptyState
					icon={GearSix}
					title="No AI providers"
					description="Add a provider, model, and credential to enable AI features."
				/>
			{:else}
				<div class="flex flex-col gap-2">
					{#each providers as p (p.id)}
						<div
							class="flex items-center justify-between rounded-lg border border-border p-3"
						>
							<div class="min-w-0">
								<div class="flex items-center gap-2">
									<span class="truncate text-sm font-medium"
										>{p.displayName}</span
									>
									{#if p.enabled}
										<Badge variant="secondary" class="text-xs">Enabled</Badge>
									{:else}
										<Badge variant="outline" class="text-xs">Disabled</Badge>
									{/if}
								</div>
								<p class="truncate text-xs text-muted-foreground">
									{p.kind} · {p.model}
								</p>
							</div>
							<div class="flex shrink-0 items-center gap-2">
								{#if p.credentialRef}
									<Badge variant="secondary" class="text-xs">Key set</Badge>
								{:else}
									<Badge variant="outline" class="text-xs">No key</Badge>
								{/if}
								<Switch
									checked={p.enabled}
									aria-label="Toggle {p.displayName}"
									onCheckedChange={(checked) => setProviderEnabled(p, checked)}
								/>
								<Button
									size="sm"
									variant="outline"
									onclick={() => openProvider(p)}>Edit</Button
								>
								{#if p.credentialRef}
									<Button
										size="sm"
										variant="ghost"
										onclick={() => deleteCredential(p)}>Remove key</Button
									>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>

		<section>
			<h2 class="text-sm font-medium">Workspace content consent</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Granting consent lets the selected provider receive workspace content
				for AI requests. Consent is scoped to this workspace and provider.
			</p>
			<Separator class="my-4" />
			{#if providers.length === 0}
				<p class="text-sm text-muted-foreground">
					Add a provider to manage its consent.
				</p>
			{:else}
				<div class="flex flex-col gap-2">
					{#each providers as provider (provider.id)}
						{@const grant = consents[provider.id]}
						<div
							class="flex items-center justify-between gap-4 rounded-lg border border-border p-3"
						>
							<div class="min-w-0">
								<p class="truncate text-sm font-medium">
									{provider.displayName}
								</p>
								<p class="text-xs text-muted-foreground">
									Policy {AI_POLICY_VERSION} · workspace content
								</p>
								{#if grant}
									<p class="text-xs text-muted-foreground">
										Granted {new Date(grant.grantedAt).toLocaleString()}
									</p>
								{/if}
							</div>
							<div class="flex shrink-0 items-center gap-2">
								<Badge variant={grant ? 'secondary' : 'outline'}
									>{grant ? 'Granted' : 'Not granted'}</Badge
								>
								{#if grant}
									<Button
										size="sm"
										variant="outline"
										disabled={changingConsent !== ''}
										onclick={() => revokeConsent(provider.id)}>Revoke</Button
									>
								{:else}
									<Button
										size="sm"
										disabled={changingConsent !== ''}
										onclick={() => grantConsent(provider.id)}>Grant</Button
									>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</section>
	</div>
</div>

<Dialog.Root bind:open={providerDialogOpen}>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title
				>{editingProvider
					? 'Edit AI provider'
					: 'Add AI provider'}</Dialog.Title
			>
			<Dialog.Description>
				Provider kinds, models, and endpoints are passed to the typed native
				provider configuration API. A provider/model catalog is not currently
				exposed.
			</Dialog.Description>
		</Dialog.Header>
		<form
			class="flex flex-col gap-5"
			onsubmit={(event) => {
				event.preventDefault();
				void saveProvider();
			}}
		>
			<Field.FieldGroup>
				<Field.Field
					data-invalid={Boolean(providerError && !providerId.trim())}
				>
					<Field.FieldLabel for="provider-id">Provider ID</Field.FieldLabel>
					<Input
						id="provider-id"
						bind:value={providerId}
						disabled={Boolean(editingProvider)}
						aria-invalid={Boolean(providerError && !providerId.trim())}
						placeholder="openai-main"
					/>
				</Field.Field>
				<Field.Field
					data-invalid={Boolean(providerError && !providerKind.trim())}
				>
					<Field.FieldLabel for="provider-kind">Provider kind</Field.FieldLabel>
					<Input
						id="provider-kind"
						bind:value={providerKind}
						aria-invalid={Boolean(providerError && !providerKind.trim())}
						placeholder="openai"
					/>
					<Field.FieldDescription
						>Must be a native-supported adapter kind.</Field.FieldDescription
					>
				</Field.Field>
				<Field.Field>
					<Field.FieldLabel for="provider-name">Display name</Field.FieldLabel>
					<Input
						id="provider-name"
						bind:value={providerName}
						placeholder="OpenAI"
					/>
				</Field.Field>
				<Field.Field
					data-invalid={Boolean(providerError && !providerModel.trim())}
				>
					<Field.FieldLabel for="provider-model">Model</Field.FieldLabel>
					<Input
						id="provider-model"
						bind:value={providerModel}
						aria-invalid={Boolean(providerError && !providerModel.trim())}
						placeholder="gpt-4.1-mini"
					/>
				</Field.Field>
				<Field.Field>
					<Field.FieldLabel for="provider-endpoint"
						>Endpoint (optional)</Field.FieldLabel
					>
					<Input
						id="provider-endpoint"
						bind:value={providerEndpoint}
						type="url"
						placeholder="https://api.example.com"
					/>
				</Field.Field>
				<Field.Field>
					<Field.FieldLabel for="provider-credential"
						>{editingProvider?.credentialRef
							? 'Replace credential'
							: 'Credential'}</Field.FieldLabel
					>
					<Input
						id="provider-credential"
						bind:value={credential}
						type="password"
						autocomplete="off"
						placeholder={editingProvider?.credentialRef
							? 'Leave blank to keep the current key'
							: 'API key'}
					/>
					<Field.FieldDescription
						>It is sent only to the native credential API and is never stored in
						this form.</Field.FieldDescription
					>
				</Field.Field>
				<Field.Field orientation="horizontal">
					<Field.FieldContent>
						<Field.FieldLabel for="provider-enabled"
							>Enable this provider</Field.FieldLabel
						>
					</Field.FieldContent>
					<Switch id="provider-enabled" bind:checked={providerEnabled} />
				</Field.Field>
			</Field.FieldGroup>
			{#if providerError}
				<p class="text-sm text-destructive">{providerError}</p>
			{/if}
			<Dialog.Footer>
				<Button
					type="button"
					variant="outline"
					onclick={() => (providerDialogOpen = false)}>Cancel</Button
				>
				<Button type="submit" disabled={savingProvider}
					>{savingProvider ? 'Saving…' : 'Save provider'}</Button
				>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
