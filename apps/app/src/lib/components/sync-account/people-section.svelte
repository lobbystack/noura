<script lang="ts">
	import type { SyncAccount } from '@noura/workspace';
	import { workspace } from '$lib/state.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import {
		invitationReady,
		recipientKind,
		recipientLabels,
		type SyncAccountSettings,
	} from '$lib/sync-account-settings.svelte';

	let {
		settings,
		account,
	}: { settings: SyncAccountSettings; account: SyncAccount } = $props();
</script>

{#if workspace.state?.rootPath && settings.statusRoot === workspace.state.rootPath && settings.syncConfigured}
	<div class="flex flex-col gap-3" aria-labelledby="sync-invitations-heading">
		<h3 id="sync-invitations-heading" class="text-sm font-medium">
			Workspace invitations
		</h3>
		<p class="text-xs text-muted-foreground">
			Workspace owners can invite another account. After acceptance, compare
			every device fingerprint. Approving the final fingerprint automatically
			rotates keys and prepares fresh encrypted checkpoints.
		</p>
		<Field.Field>
			<Field.FieldLabel for="invitation-role">Invited role</Field.FieldLabel>
			<Select.Root
				type="single"
				bind:value={settings.invitationRole}
				disabled={settings.invitationsBusy ||
					settings.busy ||
					settings.importingRecovery}
			>
				<Select.Trigger id="invitation-role"
					>{settings.invitationRole}</Select.Trigger
				>
				<Select.Content
					><Select.Group>
						<Select.Item value="viewer">Viewer</Select.Item>
						<Select.Item value="editor">Editor</Select.Item>
						<Select.Item value="admin">Admin</Select.Item>
					</Select.Group></Select.Content
				>
			</Select.Root>
		</Field.Field>
		<div class="flex flex-wrap gap-2">
			<Button
				disabled={settings.invitationsBusy ||
					settings.busy ||
					settings.importingRecovery ||
					settings.joining ||
					settings.syncChanging}
				onclick={() => settings.createInvitation()}>Create invite link</Button
			>
			<Button
				variant="outline"
				disabled={settings.invitationsBusy ||
					settings.busy ||
					settings.importingRecovery ||
					settings.joining ||
					settings.syncChanging}
				onclick={() => settings.reviewInvitations()}
				>{settings.invitationsBusy
					? 'Updating invitations…'
					: 'Review invitations'}</Button
			>
		</div>
		{#if settings.invitationsRoot === workspace.state.rootPath && settings.invitationsAccount === account.deviceId}
			{#if settings.newInvitation}
				<p class="break-all font-mono text-xs select-text">
					{settings.newInvitation.inviteUrl}
				</p>
				<p class="text-xs text-muted-foreground">
					Share this link with the intended recipient. Expires {new Date(
						settings.newInvitation.expiresAt,
					).toLocaleString()}.
				</p>
			{/if}
			<ul class="flex flex-col gap-3">
				{#each settings.invitations as invitation (invitation.id)}
					<li class="flex flex-col gap-3 rounded-xl border p-4">
						<div class="flex flex-wrap items-center justify-between gap-2">
							<span class="text-xs">{invitation.role}</span><Badge
								variant="secondary">{invitation.status}</Badge
							>
						</div>
						{#if invitation.accountId}<p class="break-all text-xs">
								Account: {invitation.accountId}
							</p>{/if}
						{#if invitation.status === 'accepted'}
							{#each invitation.devices as device (device.deviceId)}
								{@const verificationKey = `${invitation.id}:${device.deviceId}`}
								{@const recipient = recipientKind(device)}
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
								<p class="break-all font-mono text-xs select-text">
									{device.fingerprint}
								</p>
								{#if device.approved}<Badge variant="secondary"
										>Fingerprint approved</Badge
									>
								{:else}
									{#if recipient === 'browser'}
										<p class="text-xs text-muted-foreground">
											Approving pins this browser device’s signing key. The next
											automatic sync delivers the encrypted workspace keys to
											it. No separate delivery step is needed.
										</p>
									{/if}
									<Field.Field orientation="horizontal">
										<Checkbox
											id={`invite-${verificationKey}`}
											checked={settings.inviteVerified[verificationKey] ===
												device.fingerprint}
											onCheckedChange={(checked) =>
												settings.verifyInvitationDevice(
													verificationKey,
													device,
													checked,
												)}
											disabled={settings.invitationsBusy ||
												settings.busy ||
												settings.importingRecovery}
										/>
										<Field.FieldLabel for={`invite-${verificationKey}`}
											>I compared the full fingerprint with the recipient’s
											device and it matches.</Field.FieldLabel
										>
									</Field.Field>
									<Button
										variant="outline"
										disabled={settings.invitationsBusy ||
											settings.busy ||
											settings.importingRecovery ||
											settings.inviteVerified[verificationKey] !==
												device.fingerprint}
										onclick={() =>
											settings.approveInvitationDevice(invitation, device)}
										>Approve fingerprint</Button
									>
								{/if}
							{/each}
							{#if invitation.devices.length === 0}<p
									class="text-xs text-muted-foreground"
								>
									No recipient devices are available yet. Desktop and browser
									devices can both be approved and receive workspace keys. Ask
									the recipient to connect or open a device, then review
									invitations again.
								</p>{/if}
							{#if invitationReady(invitation)}
								<Badge variant="secondary">
									{settings.activatingInvitation === invitation.id
										? 'Preparing encrypted access…'
										: 'Fingerprints approved'}
								</Badge>
								{#if settings.activatingInvitation !== invitation.id}
									<Button
										variant="outline"
										disabled={settings.invitationsBusy ||
											settings.busy ||
											settings.importingRecovery}
										onclick={() =>
											settings.retryInvitationActivation(invitation)}
										>Retry access preparation</Button
									>
								{/if}
							{/if}
						{/if}
						{#if invitation.status === 'pending' || invitation.status === 'accepted'}<Button
								variant="outline"
								disabled={settings.invitationsBusy ||
									settings.busy ||
									settings.importingRecovery}
								onclick={() => settings.revokeInvitation(invitation)}
								>Revoke invitation</Button
							>{/if}
					</li>
				{/each}
			</ul>
		{/if}
		{#if settings.invitationsError}<p
				role="alert"
				class="text-sm text-destructive"
			>
				{settings.invitationsError}
			</p>{/if}
	</div>
{/if}
{#if !settings.syncConfigured}
	<p class="text-sm text-muted-foreground">
		Turn on sync for this workspace in Sync &amp; devices, then invite people
		here.
	</p>
{/if}
