<script lang="ts">
	import type { SyncAccount } from '@noura/workspace';
	import { workspace } from '$lib/state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import SyncConflictReview from '$lib/components/sync-conflict-review.svelte';
	import {
		phaseLabels,
		recipientKind,
		recipientLabels,
		syncErrorMessages,
		transitionPhaseLabels,
		type SyncAccountSettings,
	} from '$lib/sync-account-settings.svelte';

	let {
		settings,
		account,
	}: { settings: SyncAccountSettings; account: SyncAccount } = $props();
</script>

<div class="flex flex-col gap-3" aria-labelledby="workspace-sync-heading">
	<h3 id="workspace-sync-heading" class="text-sm font-medium">
		Workspace synchronization
	</h3>
	{#if !workspace.state?.rootPath}
		<p class="text-xs text-muted-foreground">
			Open a workspace to manage synchronization.
		</p>
	{:else if settings.statusRoot !== workspace.state.rootPath}
		<p role="status" class="text-xs text-muted-foreground">
			Checking this workspace’s synchronization status…
		</p>
	{:else}
		{#if settings.syncStatus}
			<p role="status" class="text-sm">
				{phaseLabels[settings.syncStatus.phase]}
			</p>
			<p class="text-xs text-muted-foreground">
				{settings.syncStatus.pending} pending · {settings.syncStatus.conflicts} conflicts
			</p>
			{#if settings.syncStatus.transition}
				<div class="flex flex-col gap-2" aria-live="polite">
					<p class="text-xs font-medium">
						{transitionPhaseLabels[settings.syncStatus.transition.phase]}
					</p>
					<ul class="flex flex-col gap-1">
						{#each settings.syncStatus.transition.objects as object (object.objectId)}
							<li class="flex items-center justify-between gap-2 text-xs">
								<span class="truncate font-mono"
									>{object.path ?? object.objectId}</span
								>
								<Badge variant="secondary"
									>{object.installed ? 'Installed' : 'Pending'}</Badge
								>
							</li>
						{/each}
					</ul>
				</div>
			{/if}
			{#if settings.syncStatus.activation}
				<div class="flex flex-col gap-2" aria-live="polite">
					<p class="text-xs font-medium">Preparing collaborative object</p>
					<div class="flex items-center justify-between gap-2 text-xs">
						<span class="truncate font-mono"
							>{settings.syncStatus.activation.path ??
								settings.syncStatus.activation.objectId}</span
						>
						<Badge variant="secondary"
							>{settings.syncStatus.activation.installed
								? 'Installed'
								: 'Pending'}</Badge
						>
					</div>
				</div>
			{/if}
			{#if settings.syncStatus.conflicts > 0}<p
					class="text-xs text-muted-foreground"
				>
					Conflicting versions are preserved. Review them before resolving the
					differences.
				</p>{/if}
			{#key workspace.state.rootPath + ':' + account.deviceId}
				<SyncConflictReview
					disabled={settings.busy ||
						settings.syncChanging ||
						settings.joining ||
						settings.importingRecovery}
					onresolved={(status) => settings.conflictsResolved(status)}
				/>
			{/key}
			{#if settings.syncStatus.errorCode}<p
					role="alert"
					class="break-all text-sm text-destructive"
				>
					{syncErrorMessages[settings.syncStatus.errorCode] ??
						`Synchronization error: ${settings.syncStatus.errorCode}`}
				</p>{/if}
			{#if settings.syncStatus.lastSuccess}<p
					class="text-xs text-muted-foreground"
				>
					Last successful sync: {new Date(
						settings.syncStatus.lastSuccess,
					).toLocaleString()}
				</p>{/if}
			<div>
				{#if settings.syncStatus.phase === 'disabled'}<Button
						disabled={settings.busy ||
							settings.syncChanging ||
							settings.joining ||
							settings.importingRecovery}
						onclick={() => settings.changeWorkspaceSync('enable')}
						>{settings.syncChanging ? 'Enabling…' : 'Enable sync'}</Button
					>
				{:else if settings.syncStatus.phase === 'paused'}<Button
						disabled={settings.busy ||
							settings.syncChanging ||
							settings.joining ||
							settings.importingRecovery}
						onclick={() => settings.changeWorkspaceSync('resume')}
						>{settings.syncChanging ? 'Resuming…' : 'Resume sync'}</Button
					>
				{:else}<Button
						variant="outline"
						disabled={settings.busy ||
							settings.syncChanging ||
							settings.joining ||
							settings.importingRecovery}
						onclick={() => settings.changeWorkspaceSync('pause')}
						>{settings.syncChanging ? 'Pausing…' : 'Pause sync'}</Button
					>{/if}
			</div>
		{/if}
		{#if settings.syncError}<p role="alert" class="text-sm text-destructive">
				{settings.syncError}
			</p>{/if}
	{/if}
</div>
{#if workspace.state?.rootPath && settings.statusRoot === workspace.state.rootPath && settings.syncConfigured}
	<div class="flex flex-col gap-3" aria-labelledby="sync-devices-heading">
		<h3 id="sync-devices-heading" class="text-sm font-medium">
			Workspace devices
		</h3>
		<p class="text-xs text-muted-foreground">
			Compare each fingerprint with the fingerprint shown on the actual other
			device. The server’s device list alone does not establish trust. Both
			devices must approve each other to exchange keys.
		</p>
		<div>
			<Button
				variant="outline"
				disabled={settings.busy ||
					settings.syncChanging ||
					settings.devicesBusy ||
					settings.joining ||
					settings.importingRecovery}
				onclick={() => settings.reviewDevices()}
				>{settings.devicesBusy ? 'Updating devices…' : 'Review devices'}</Button
			>
		</div>
		{#if settings.devicesRoot === workspace.state.rootPath && settings.devicesAccount === account.deviceId}
			{#if settings.devicesError}<p
					role="alert"
					class="text-sm text-destructive"
				>
					{settings.devicesError}
				</p>{/if}
			{#if settings.devices.length > 0}
				<ul class="flex flex-col gap-3">
					{#each settings.devices as device (device.deviceId)}
						{@const recipient = recipientKind(device)}
						<li class="flex flex-col gap-3 rounded-xl border p-4">
							<div class="flex flex-wrap items-center justify-between gap-2">
								<div class="flex flex-wrap items-center gap-2">
									<p class="break-all text-xs">
										Device: {device.deviceId}
									</p>
									{#if recipient}<Badge
											variant="outline"
											aria-label={`Device type: ${recipientLabels[recipient]}`}
											>{recipientLabels[recipient]}</Badge
										>{/if}
								</div>
								{#if device.deviceId === account.deviceId}<Badge
										variant="secondary">This device</Badge
									>{:else if device.approved}<Badge variant="secondary"
										>Approved</Badge
									>{/if}
							</div>
							<p class="break-all font-mono text-xs select-text">
								{device.fingerprint}
							</p>
							{#if recipient === 'browser'}
								<p class="text-xs text-muted-foreground">
									Approving pins this browser device’s signing key. The next
									automatic sync delivers the encrypted workspace keys to it. No
									separate delivery step is needed.
								</p>
							{/if}
							{#if device.deviceId !== account.deviceId && !device.approved}
								<Field.FieldGroup
									><Field.Field orientation="horizontal">
										<Checkbox
											id={`verify-device-${device.deviceId}`}
											checked={settings.verified[device.deviceId] ===
												device.fingerprint}
											onCheckedChange={(checked) =>
												settings.verifyDevice(device, checked)}
											disabled={settings.devicesBusy ||
												settings.busy ||
												settings.importingRecovery}
										/>
										<Field.FieldLabel for={`verify-device-${device.deviceId}`}
											>I compared the full fingerprint on the other device and
											it matches.</Field.FieldLabel
										>
									</Field.Field></Field.FieldGroup
								>
								<div>
									<Button
										disabled={settings.devicesBusy ||
											settings.busy ||
											settings.verified[device.deviceId] !== device.fingerprint}
										onclick={() => settings.approveDevice(device)}
										>Approve device</Button
									>
								</div>
							{/if}
						</li>
					{/each}
				</ul>
			{:else if !settings.devicesError && !settings.devicesBusy}<p
					class="text-xs text-muted-foreground"
				>
					No workspace devices were returned.
				</p>{/if}
		{/if}
	</div>
{/if}
<div class="flex flex-col gap-2">
	<div class="flex flex-wrap gap-2">
		<Button
			variant="outline"
			disabled={settings.busy ||
				settings.exportingRecovery ||
				settings.importingRecovery ||
				settings.joining ||
				settings.syncChanging ||
				!workspace.state?.rootPath ||
				settings.statusRoot !== workspace.state.rootPath ||
				!settings.syncConfigured}
			onclick={() => settings.exportRecoveryKit()}
			>{settings.exportingRecovery
				? 'Saving recovery kit…'
				: 'Export recovery kit'}</Button
		>
		<Button
			variant="outline"
			disabled={settings.busy ||
				settings.exportingRecovery ||
				settings.importingRecovery ||
				settings.joining ||
				settings.syncChanging ||
				!workspace.state?.rootPath ||
				settings.statusRoot !== workspace.state.rootPath ||
				!settings.syncConfigured}
			onclick={() => settings.importRecoveryKit()}
			>{settings.importingRecovery
				? 'Importing recovery kit…'
				: 'Import recovery kit'}</Button
		>
	</div>
	<p class="text-xs text-muted-foreground">
		The recovery kit is a secret file. Store it securely outside your workspace.
		To import, sign in and join or open the matching workspace first. Import
		restores workspace keys and files, and leaves sync paused for device
		verification.
	</p>
	{#if !workspace.state?.rootPath || settings.statusRoot !== workspace.state.rootPath || !settings.syncConfigured}<p
			class="text-xs text-muted-foreground"
		>
			Open a workspace configured for synchronization to use recovery kits.
		</p>{/if}
</div>
<div class="flex flex-col gap-3" aria-labelledby="join-workspace-heading">
	<h3 id="join-workspace-heading" class="text-sm font-medium">
		Join a workspace
	</h3>
	<p class="text-xs text-muted-foreground">
		Download a workspace you were invited to into an empty folder on this
		computer. The server only knows workspace IDs, so choose a name for your
		copy.
	</p>
	<div>
		<Button
			variant="outline"
			disabled={settings.busy ||
				settings.remoteBusy ||
				settings.joining ||
				settings.importingRecovery}
			onclick={() => settings.loadRemoteWorkspaces()}
			>{settings.remoteBusy
				? 'Loading workspaces…'
				: 'Load my workspaces'}</Button
		>
	</div>
	{#if settings.remoteAccount === account.deviceId}
		{#if settings.remoteWorkspaces.length > 0}
			<form
				onsubmit={(event) => {
					event.preventDefault();
					void settings.joinRemoteWorkspace();
				}}
			>
				<Field.FieldGroup>
					<Field.Field
						><Field.FieldLabel for="remote-workspace-id"
							>Workspace ID</Field.FieldLabel
						>
						<Select.Root
							type="single"
							bind:value={settings.selectedRemote}
							disabled={settings.joining ||
								settings.remoteBusy ||
								settings.importingRecovery}
						>
							<Select.Trigger id="remote-workspace-id" class="w-full"
								><span class="truncate"
									>{settings.selectedMembership
										? `${settings.selectedMembership.id} · ${settings.selectedMembership.role}`
										: 'Choose a workspace'}</span
								></Select.Trigger
							>
							<Select.Content
								><Select.Group
									>{#each settings.remoteWorkspaces as remote (remote.id)}<Select.Item
											value={remote.id}
											label={`${remote.id} · ${remote.role}`}
											>{remote.id} · {remote.role}</Select.Item
										>{/each}</Select.Group
								></Select.Content
							>
						</Select.Root>
					</Field.Field>
					{#if settings.selectedMembership?.role === 'viewer'}<p
							class="text-xs text-muted-foreground"
						>
							As a viewer, this device downloads shared updates. Edits you make
							locally stay on this device and are not uploaded.
						</p>{/if}
					<Field.Field
						><Field.FieldLabel for="joined-workspace-name"
							>Local display name</Field.FieldLabel
						><Input
							id="joined-workspace-name"
							bind:value={settings.localName}
							required
							disabled={settings.joining ||
								settings.remoteBusy ||
								settings.importingRecovery}
							autocomplete="off"
						/></Field.Field
					>
					<div>
						<Button
							type="submit"
							disabled={settings.busy ||
								settings.joining ||
								settings.importingRecovery ||
								settings.remoteBusy ||
								!settings.selectedRemote ||
								!settings.localName.trim()}
							>{settings.joining
								? 'Joining workspace…'
								: 'Choose empty folder and join'}</Button
						>
					</div>
				</Field.FieldGroup>
			</form>
		{:else if !settings.remoteBusy}<p class="text-xs text-muted-foreground">
				No workspaces available to this account were returned.
			</p>{/if}
	{/if}
	{#if settings.joinError}<p role="alert" class="text-sm text-destructive">
			{settings.joinError}
		</p>{/if}
	{#if settings.joinNotice}<p role="status" class="text-sm">
			{settings.joinNotice}
		</p>{/if}
</div>
