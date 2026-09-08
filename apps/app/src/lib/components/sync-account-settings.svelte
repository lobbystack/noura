<script lang="ts">
	import { onMount } from 'svelte';
	import type {
		DeviceSignInInfo,
		SyncAccount,
		SyncDevice,
		SyncInvitation,
		SyncInvitationLink,
		SyncInvitationRole,
		RemoteSyncWorkspace,
		WorkspaceSyncStatus,
	} from '@noura/workspace';
	import { getNouraClient, workspace } from '$lib/state.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import SyncConflictReview from '$lib/components/sync-conflict-review.svelte';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Field from '$lib/components/ui/field/index.js';
	import * as Select from '$lib/components/ui/select/index.js';

	let account = $state<SyncAccount | null>(null);
	let request = $state<DeviceSignInInfo | null>(null);
	let origin = $state('');
	let loading = $state(true);
	let busy = $state(false);
	let opening = $state(false);
	let exportingRecovery = $state(false);
	let importingRecovery = $state(false);
	let error = $state('');
	let notice = $state('');
	let syncStatus = $state<WorkspaceSyncStatus | null>(null);
	let statusRoot = $state<string | null>(null);
	const syncConfigured = $derived(
		syncStatus !== null && syncStatus.phase !== 'disabled',
	);
	let syncError = $state('');
	let syncChanging = $state(false);
	let devices = $state<SyncDevice[]>([]);
	let devicesRoot = $state<string | null>(null);
	let devicesAccount = $state('');
	let devicesBusy = $state(false);
	let devicesError = $state('');
	let verified = $state<Record<string, string>>({});
	let invitations = $state<SyncInvitation[]>([]);
	let invitationsRoot = $state<string | null>(null);
	let invitationsAccount = $state('');
	let invitationsBusy = $state(false);
	let activatingInvitation = $state<string | null>(null);
	let invitationsError = $state('');
	let invitationRole = $state<SyncInvitationRole>('editor');
	let newInvitation = $state<SyncInvitationLink | null>(null);
	let inviteVerified = $state<Record<string, string>>({});
	let remoteWorkspaces = $state<RemoteSyncWorkspace[]>([]);
	let remoteAccount = $state('');
	let remoteBusy = $state(false);
	let joining = $state(false);
	let joinError = $state('');
	let joinNotice = $state('');
	let selectedRemote = $state('');
	let localName = $state('');
	const selectedMembership = $derived(
		remoteWorkspaces.find((item) => item.id === selectedRemote),
	);
	async function loadRemoteWorkspaces() {
		const ownId = account?.deviceId;
		if (!ownId) return;
		remoteBusy = true;
		joinError = '';
		joinNotice = '';
		selectedRemote = '';
		try {
			const values = await getNouraClient().sync.remoteWorkspaces();
			if (!disposed && account?.deviceId === ownId) {
				remoteWorkspaces = values;
				remoteAccount = ownId;
			}
		} catch (cause) {
			if (!disposed && account?.deviceId === ownId) joinError = message(cause);
		} finally {
			if (!disposed) remoteBusy = false;
		}
	}
	async function joinRemoteWorkspace() {
		const ownId = account?.deviceId;
		if (
			!ownId ||
			remoteAccount !== ownId ||
			!localName.trim() ||
			!remoteWorkspaces.some((item) => item.id === selectedRemote)
		)
			return;
		joining = true;
		joinError = '';
		joinNotice = '';
		try {
			const joined = await getNouraClient().sync.joinWorkspace(
				selectedRemote,
				localName.trim(),
			);
			if (joined) {
				await workspace.refresh();
				await workspace.refreshRecents();
				if (!disposed && account?.deviceId === ownId) {
					selectedRemote = '';
					localName = '';
					joinNotice =
						'Workspace joined and paused. Compare and approve the full fingerprints on both devices, then choose Resume sync.';
				}
			}
		} catch (cause) {
			if (!disposed && account?.deviceId === ownId) joinError = message(cause);
		} finally {
			if (!disposed) joining = false;
		}
	}

	async function reviewDevices() {
		const root = workspace.state?.rootPath;
		const deviceId = account?.deviceId;
		if (!root || !deviceId) return;
		devicesBusy = true;
		devicesError = '';
		verified = {};
		try {
			const result = await getNouraClient().sync.workspaceDevices();
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === deviceId
			) {
				devices = result;
				devicesRoot = root;
				devicesAccount = deviceId;
			}
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === deviceId
			) {
				devicesError = message(cause);
				devicesRoot = root;
				devicesAccount = deviceId;
				devices = [];
			}
		} finally {
			if (!disposed) devicesBusy = false;
		}
	}
	async function approveDevice(device: SyncDevice) {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		if (
			!root ||
			!ownId ||
			devicesRoot !== root ||
			devicesAccount !== ownId ||
			device.approved ||
			device.deviceId === ownId ||
			verified[device.deviceId] !== device.fingerprint
		)
			return;
		devicesBusy = true;
		devicesError = '';
		try {
			await getNouraClient().sync.approveWorkspaceDevice(
				device.deviceId,
				device.fingerprint,
			);
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				await reviewDevices();
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				devicesError = message(cause);
		} finally {
			if (!disposed) {
				devicesBusy = false;
				verified = {};
			}
		}
	}

	async function reviewInvitations() {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		if (!root || !ownId) return;
		invitationsBusy = true;
		invitationsError = '';
		inviteVerified = {};
		try {
			const result = await getNouraClient().sync.workspaceInvitations();
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			) {
				invitations = result;
				invitationsRoot = root;
				invitationsAccount = ownId;
			}
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			) {
				invitationsError = message(cause);
				invitationsRoot = root;
				invitationsAccount = ownId;
				invitations = [];
			}
		} finally {
			if (!disposed) invitationsBusy = false;
		}
	}
	async function createInvitation() {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		if (!root || !ownId) return;
		invitationsBusy = true;
		invitationsError = '';
		newInvitation = null;
		try {
			const client = getNouraClient().sync;
			const created = await client.createWorkspaceInvitation(invitationRole);
			if (
				disposed ||
				workspace.state?.rootPath !== root ||
				account?.deviceId !== ownId
			)
				return;
			newInvitation = created;
			invitationsRoot = root;
			invitationsAccount = ownId;
			const result = await client.workspaceInvitations();
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			) {
				newInvitation = created;
				invitations = result;
				invitationsRoot = root;
				invitationsAccount = ownId;
				inviteVerified = {};
			}
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				invitationsError = message(cause);
		} finally {
			if (!disposed) invitationsBusy = false;
		}
	}
	async function approveInvitationDevice(
		invitation: SyncInvitation,
		device: SyncDevice,
	) {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		const verificationKey = `${invitation.id}:${device.deviceId}`;
		if (
			!root ||
			!ownId ||
			invitationsRoot !== root ||
			invitationsAccount !== ownId ||
			device.approved ||
			inviteVerified[verificationKey] !== device.fingerprint
		)
			return;
		invitationsBusy = true;
		if (
			invitation.devices.every(
				(candidate) =>
					candidate.approved || candidate.deviceId === device.deviceId,
			)
		)
			activatingInvitation = invitation.id;
		invitationsError = '';
		try {
			const client = getNouraClient().sync;
			await client.approveInvitedDevice(
				invitation.id,
				device.deviceId,
				device.fingerprint,
			);
			const result = await client.workspaceInvitations();
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			) {
				invitations = result;
				inviteVerified = {};
			}
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				invitationsError = message(cause);
		} finally {
			if (!disposed) {
				invitationsBusy = false;
				activatingInvitation = null;
			}
		}
	}
	function invitationReady(invitation: SyncInvitation) {
		return (
			invitation.status === 'accepted' &&
			invitation.devices.length > 0 &&
			invitation.devices.every((device) => device.approved)
		);
	}
	async function retryInvitationActivation(invitation: SyncInvitation) {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		if (
			!root ||
			!ownId ||
			invitationsRoot !== root ||
			invitationsAccount !== ownId ||
			!invitationReady(invitation)
		)
			return;
		invitationsBusy = true;
		activatingInvitation = invitation.id;
		invitationsError = '';
		try {
			const client = getNouraClient().sync;
			await client.finalizeWorkspaceInvitation(invitation.id);
			const result = await client.workspaceInvitations();
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			) {
				invitations = result;
				inviteVerified = {};
			}
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				invitationsError = message(cause);
		} finally {
			if (!disposed) {
				invitationsBusy = false;
				activatingInvitation = null;
			}
		}
	}
	async function revokeInvitation(invitation: SyncInvitation) {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		if (
			!root ||
			!ownId ||
			invitationsRoot !== root ||
			invitationsAccount !== ownId ||
			!['pending', 'accepted'].includes(invitation.status)
		)
			return;
		invitationsBusy = true;
		invitationsError = '';
		try {
			const client = getNouraClient().sync;
			await client.revokeWorkspaceInvitation(invitation.id);
			if (!disposed && newInvitation?.id === invitation.id)
				newInvitation = null;
			const result = await client.workspaceInvitations();
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			) {
				invitations = result;
				inviteVerified = {};
			}
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				invitationsError = message(cause);
		} finally {
			if (!disposed) invitationsBusy = false;
		}
	}

	const syncErrorMessages: Record<string, string> = {
		sync_connection_changed:
			'Reconnect the original account and device identity to resume this workspace.',
	};
	let statusTimer: ReturnType<typeof setTimeout> | undefined;
	let statusGeneration = 0;
	const phaseLabels: Record<WorkspaceSyncStatus['phase'], string> = {
		disabled: 'Not enabled',
		paused: 'Paused',
		idle: 'Waiting for changes',
		syncing: 'Syncing',
		error: 'Needs attention',
	};
	const transitionPhaseLabels: Record<
		NonNullable<WorkspaceSyncStatus['transition']>['phase'],
		string
	> = {
		prepare: 'Preparing local recovery records',
		stage: 'Staging signed access change',
		resolve_commit: 'Verifying relay transaction',
		install: 'Installing fresh checkpoints',
		rebase: 'Rebasing retained local drafts',
		complete: 'Access preparation complete',
	};
	async function refreshWorkspaceStatus() {
		const root = workspace.state?.rootPath;
		const attempt = statusGeneration;
		try {
			if (!root || syncChanging || importingRecovery) return;
			const value = await getNouraClient().sync.workspaceStatus();
			if (
				!disposed &&
				attempt === statusGeneration &&
				workspace.state?.rootPath === root
			) {
				syncStatus = value;
				statusRoot = root;
				syncError = '';
			}
		} catch (cause) {
			if (
				!disposed &&
				attempt === statusGeneration &&
				workspace.state?.rootPath === root
			) {
				syncStatus = null;
				statusRoot = root ?? null;
				syncError = message(cause);
			}
		} finally {
			if (!disposed)
				statusTimer = setTimeout(() => void refreshWorkspaceStatus(), 5000);
		}
	}
	async function changeWorkspaceSync(action: 'enable' | 'pause' | 'resume') {
		const root = workspace.state?.rootPath;
		if (!root) return;
		syncChanging = true;
		syncError = '';
		statusGeneration += 1;
		try {
			const sync = getNouraClient().sync;
			const value = await (action === 'enable'
				? sync.enableWorkspace()
				: action === 'pause'
					? sync.pauseWorkspace()
					: sync.resumeWorkspace());
			if (!disposed && workspace.state?.rootPath === root) {
				syncStatus = value;
				statusRoot = root;
			}
		} catch (cause) {
			if (!disposed && workspace.state?.rootPath === root) {
				statusRoot = root;
				syncError = message(cause);
			}
		} finally {
			if (!disposed) syncChanging = false;
		}
	}

	let timer: ReturnType<typeof setTimeout> | undefined;
	let generation = 0;
	let disposed = false;
	let expiresAt = 0;
	function message(cause: unknown): string {
		if (
			cause &&
			typeof cause === 'object' &&
			'code' in cause &&
			typeof cause.code === 'string'
		) {
			const messages: Record<string, string> = {
				sync_owner_required: 'Only a workspace owner can manage invitations.',
				sync_forbidden:
					'This account does not have permission to perform this action.',
				sync_invitation_limit:
					'This workspace has 100 active invitations. Revoke an unused invitation before creating another.',
				sync_invitation_not_ready:
					'Refresh invitations. The recipient must accept before you can grant access.',
				sync_invitation_already_member:
					'This account already belongs to the workspace.',
				sync_device_approval_required:
					'Compare and approve every recipient device fingerprint before granting access.',
				sync_device_changed:
					'The device details changed. Refresh the list and compare the full fingerprint again.',
				sync_key_rotation_required:
					'Access cannot change until the workspace keys are rotated.',
				sync_access_transition_unavailable:
					'Encrypted collaboration activation is not enabled on this sync server yet. No workspace keys were shared.',
				sync_incompatible_collaboration_capability:
					'This app or sync server is not compatible with the workspace collaboration capability.',
				sync_activation_stale:
					'The workspace changed while preparing a collaborative object. Sync will rebuild it against the current access policy.',
				sync_activation_commit_uncertain:
					'The server may have committed a collaborative object. Keep sync enabled so this device can verify and finish installing it.',
				sync_activation_blob_incomplete:
					'The encrypted attachment is not fully staged yet. Sync will resume the upload without discarding local bytes.',
				sync_collaboration_capability_changed:
					'The signed workspace collaboration capability changed. Verify the workspace owner before continuing.',
				collaboration_attachment_transition_required:
					'This workspace contains a file that must remain in attachment mode. Access was not changed.',
				collaboration_needs_review:
					'A local draft or external file change needs review before access preparation can finish.',
				sync_rate_limited:
					'The server request limit was reached. Wait a minute, then try again.',
			};
			if (messages[cause.code]) return messages[cause.code];
		}
		if (
			cause &&
			typeof cause === 'object' &&
			'message' in cause &&
			typeof cause.message === 'string'
		)
			return cause.message;
		return 'The account request could not be completed. Please try again.';
	}
	onMount(() => {
		void refreshWorkspaceStatus();
		void getNouraClient()
			.sync.account()
			.then((value) => {
				if (!disposed) {
					account = value;
					origin = value?.origin ?? '';
				}
			})
			.catch((cause) => {
				if (!disposed) error = message(cause);
			})
			.finally(() => {
				if (!disposed) loading = false;
			});
		return () => {
			disposed = true;
			generation += 1;
			clearTimeout(timer);
			clearTimeout(statusTimer);
			if (request || busy)
				void getNouraClient()
					.sync.cancelSignIn()
					.catch(() => {});
		};
	});
	async function poll(attempt: number) {
		if (disposed || generation !== attempt) return;
		if (Date.now() >= expiresAt) {
			await cancel('The sign-in code expired. Start again to get a new code.');
			return;
		}
		try {
			const result = await getNouraClient().sync.pollSignIn();
			if (disposed || generation !== attempt) return;
			if (result.status === 'connected') {
				account = result.account;
				request = null;
				notice =
					'Account connected. You can manage synchronization for the open workspace below.';
				return;
			}
			const delay = Number.isFinite(result.retryAfter)
				? Math.max(1, result.retryAfter) * 1000
				: 5000;
			timer = setTimeout(
				() => void poll(attempt),
				Math.min(delay, Math.max(0, expiresAt - Date.now())),
			);
		} catch (cause) {
			if (disposed || generation !== attempt) return;
			error = message(cause);
			request = null;
			busy = true;
			try {
				await getNouraClient().sync.cancelSignIn();
			} catch {
				/* The public polling error remains visible. */
			} finally {
				if (!disposed) busy = false;
			}
		}
	}
	async function begin() {
		busy = true;
		error = '';
		notice = '';
		const attempt = ++generation;
		try {
			const value = await getNouraClient().sync.beginSignIn(origin.trim());
			if (disposed || generation !== attempt) {
				await getNouraClient().sync.cancelSignIn();
				return;
			}
			request = value;
			const lifetime = Number.isFinite(value.expiresIn)
				? Math.min(900, Math.max(0, value.expiresIn))
				: 0;
			expiresAt = Date.now() + lifetime * 1000;
			timer = setTimeout(
				() => void poll(attempt),
				Math.min(5000, lifetime * 1000),
			);
		} catch (cause) {
			if (!disposed) error = message(cause);
		} finally {
			if (!disposed) busy = false;
		}
	}
	async function openBrowser() {
		opening = true;
		error = '';
		try {
			await getNouraClient().sync.openSignInBrowser();
		} catch (cause) {
			if (!disposed) error = message(cause);
		} finally {
			if (!disposed) opening = false;
		}
	}
	async function cancel(reason = '') {
		generation += 1;
		clearTimeout(timer);
		busy = true;
		error = '';
		notice = '';
		try {
			await getNouraClient().sync.cancelSignIn();
			const current = await getNouraClient().sync.account();
			if (!disposed) {
				account = current;
				request = null;
				notice = current
					? 'Account connected. You can manage synchronization for the open workspace below.'
					: reason || 'Sign-in cancelled.';
			}
		} catch (cause) {
			if (!disposed) error = message(cause);
		} finally {
			if (!disposed) busy = false;
		}
	}
	async function exportRecoveryKit() {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		if (!root || !ownId) return;
		exportingRecovery = true;
		error = '';
		notice = '';
		try {
			const saved = await getNouraClient().sync.exportRecoveryIdentity();
			if (
				!disposed &&
				saved &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				notice =
					'Workspace recovery kit saved. Keep this secret file securely outside your workspace.';
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				error = message(cause);
		} finally {
			if (!disposed) exportingRecovery = false;
		}
	}
	async function importRecoveryKit() {
		const root = workspace.state?.rootPath;
		const ownId = account?.deviceId;
		if (!root || !ownId) return;
		importingRecovery = true;
		error = '';
		notice = '';
		statusGeneration += 1;
		try {
			const restored = await getNouraClient().sync.importRecoveryKit();
			if (restored) {
				await workspace.refresh();
				if (
					disposed ||
					workspace.state?.rootPath !== root ||
					account?.deviceId !== ownId
				)
					return;
				const status = await getNouraClient().sync.workspaceStatus();
				if (
					!disposed &&
					workspace.state?.rootPath === root &&
					account?.deviceId === ownId
				) {
					syncStatus = status;
					statusRoot = root;
					syncError = '';
					devices = [];
					devicesRoot = null;
					verified = {};
					invitations = [];
					invitationsRoot = null;
					inviteVerified = {};
					newInvitation = null;
					notice =
						'Workspace recovery kit imported. Sync remains paused. Compare and approve the device fingerprints before resuming.';
				}
			}
		} catch (cause) {
			if (
				!disposed &&
				workspace.state?.rootPath === root &&
				account?.deviceId === ownId
			)
				error = message(cause);
		} finally {
			if (!disposed) importingRecovery = false;
		}
	}
	async function disconnect() {
		busy = true;
		error = '';
		notice = '';
		try {
			await getNouraClient().sync.disconnect();
			if (!disposed) {
				account = null;
				devices = [];
				devicesRoot = null;
				devicesAccount = '';
				remoteWorkspaces = [];
				remoteAccount = '';
				selectedRemote = '';
				localName = '';
				joinError = '';
				joinNotice = '';
				verified = {};
				devicesError = '';
				invitations = [];
				invitationsRoot = null;
				invitationsAccount = '';
				invitationsError = '';
				inviteVerified = {};
				newInvitation = null;
				notice = 'This device is disconnected from the account.';
			}
		} catch (cause) {
			if (!disposed) error = message(cause);
		} finally {
			if (!disposed) busy = false;
		}
	}
</script>

<section aria-labelledby="sync-account-heading">
	<h2 id="sync-account-heading" class="text-sm font-medium">Server account</h2>
	<p class="mt-1 text-xs text-muted-foreground">
		Connect this device to a Noura server and choose whether to synchronize the
		open workspace.
	</p>
	<Separator class="my-4" />
	<div class="flex flex-col gap-4">
		{#if loading}<p role="status" class="text-sm text-muted-foreground">
				Checking this device’s account…
			</p>
		{:else if account}
			<div class="flex flex-wrap items-center justify-between gap-3">
				<p class="break-all text-sm">{account.origin}</p>
				<Badge variant="secondary">Account connected</Badge>
			</div>
			<p class="break-all text-xs text-muted-foreground">
				Device ID: {account.deviceId}
			</p>
			<div class="flex flex-col gap-1">
				<p class="text-xs font-medium">This device’s fingerprint</p>
				<p class="break-all font-mono text-xs select-text">
					{account.fingerprint}
				</p>
			</div>
			<p class="text-xs text-muted-foreground">
				Credentials stay in your operating system’s credential store. Your local
				workspace remains available when disconnected.
			</p>
			<div class="flex flex-col gap-3" aria-labelledby="join-workspace-heading">
				<h3 id="join-workspace-heading" class="text-sm font-medium">
					Join a workspace
				</h3>
				<p class="text-xs text-muted-foreground">
					Create a local copy of a workspace you belong to on this server. The
					server provides workspace IDs and your role, not readable names.
				</p>
				<div>
					<Button
						variant="outline"
						disabled={busy || remoteBusy || joining || importingRecovery}
						onclick={loadRemoteWorkspaces}
						>{remoteBusy ? 'Loading workspaces…' : 'Load my workspaces'}</Button
					>
				</div>
				{#if remoteAccount === account.deviceId}
					{#if remoteWorkspaces.length > 0}
						<form
							onsubmit={(event) => {
								event.preventDefault();
								void joinRemoteWorkspace();
							}}
						>
							<Field.FieldGroup>
								<Field.Field
									><Field.FieldLabel for="remote-workspace-id"
										>Workspace ID</Field.FieldLabel
									>
									<Select.Root
										type="single"
										bind:value={selectedRemote}
										disabled={joining || remoteBusy || importingRecovery}
									>
										<Select.Trigger id="remote-workspace-id" class="w-full"
											><span class="truncate"
												>{selectedMembership
													? `${selectedMembership.id} · ${selectedMembership.role}`
													: 'Choose a workspace'}</span
											></Select.Trigger
										>
										<Select.Content
											><Select.Group
												>{#each remoteWorkspaces as remote (remote.id)}<Select.Item
														value={remote.id}
														label={`${remote.id} · ${remote.role}`}
														>{remote.id} · {remote.role}</Select.Item
													>{/each}</Select.Group
											></Select.Content
										>
									</Select.Root>
								</Field.Field>
								{#if selectedRemote}<p
										class="break-all text-xs text-muted-foreground"
									>
										Selected ID: {selectedRemote}
										Role: {selectedMembership?.role}
									</p>{/if}
								{#if selectedMembership?.role === 'viewer'}<p
										class="text-xs text-muted-foreground"
									>
										As a viewer, this device downloads shared updates. Edits you
										make locally stay on this device and are not uploaded.
									</p>{/if}
								<Field.Field
									><Field.FieldLabel for="joined-workspace-name"
										>Local display name</Field.FieldLabel
									><Input
										id="joined-workspace-name"
										bind:value={localName}
										required
										disabled={joining || remoteBusy || importingRecovery}
										autocomplete="off"
									/></Field.Field
								>
								<div>
									<Button
										type="submit"
										disabled={busy ||
											joining ||
											importingRecovery ||
											remoteBusy ||
											!selectedRemote ||
											!localName.trim()}
										>{joining
											? 'Joining workspace…'
											: 'Choose empty folder and join'}</Button
									>
								</div>
							</Field.FieldGroup>
						</form>
					{:else if !remoteBusy}<p class="text-xs text-muted-foreground">
							No workspaces available to this account were returned.
						</p>{/if}
				{/if}
				{#if joinError}<p role="alert" class="text-sm text-destructive">
						{joinError}
					</p>{/if}
				{#if joinNotice}<p role="status" class="text-sm">{joinNotice}</p>{/if}
			</div>
			<div class="flex flex-col gap-3" aria-labelledby="workspace-sync-heading">
				<h3 id="workspace-sync-heading" class="text-sm font-medium">
					Workspace synchronization
				</h3>
				{#if !workspace.state?.rootPath}
					<p class="text-xs text-muted-foreground">
						Open a workspace to manage synchronization.
					</p>
				{:else if statusRoot !== workspace.state.rootPath}
					<p role="status" class="text-xs text-muted-foreground">
						Checking this workspace’s synchronization status…
					</p>
				{:else}
					{#if syncStatus}
						<p role="status" class="text-sm">{phaseLabels[syncStatus.phase]}</p>
						<p class="text-xs text-muted-foreground">
							{syncStatus.pending} pending · {syncStatus.conflicts} conflicts
						</p>
						{#if syncStatus.transition}
							<div class="flex flex-col gap-2" aria-live="polite">
								<p class="text-xs font-medium">
									{transitionPhaseLabels[syncStatus.transition.phase]}
								</p>
								<ul class="flex flex-col gap-1">
									{#each syncStatus.transition.objects as object (object.objectId)}
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
						{#if syncStatus.activation}
							<div class="flex flex-col gap-2" aria-live="polite">
								<p class="text-xs font-medium">
									Preparing collaborative object
								</p>
								<div class="flex items-center justify-between gap-2 text-xs">
									<span class="truncate font-mono"
										>{syncStatus.activation.path ??
											syncStatus.activation.objectId}</span
									>
									<Badge variant="secondary"
										>{syncStatus.activation.installed
											? 'Installed'
											: 'Pending'}</Badge
									>
								</div>
							</div>
						{/if}
						{#if syncStatus.conflicts > 0}<p
								class="text-xs text-muted-foreground"
							>
								Conflicting versions are preserved. Review them before resolving
								the differences.
							</p>{/if}
						{#key workspace.state.rootPath + ':' + account.deviceId}
							<SyncConflictReview
								disabled={busy || syncChanging || joining || importingRecovery}
								onresolved={(status) => {
									syncStatus = status;
									statusRoot = workspace.state?.rootPath ?? null;
									statusGeneration += 1;
									syncError = '';
								}}
							/>
						{/key}
						{#if syncStatus.errorCode}<p
								role="alert"
								class="break-all text-sm text-destructive"
							>
								{syncErrorMessages[syncStatus.errorCode] ??
									`Synchronization error: ${syncStatus.errorCode}`}
							</p>{/if}
						{#if syncStatus.lastSuccess}<p
								class="text-xs text-muted-foreground"
							>
								Last successful sync: {new Date(
									syncStatus.lastSuccess,
								).toLocaleString()}
							</p>{/if}
						<div>
							{#if syncStatus.phase === 'disabled'}<Button
									disabled={busy ||
										syncChanging ||
										joining ||
										importingRecovery}
									onclick={() => changeWorkspaceSync('enable')}
									>{syncChanging
										? 'Enabling…'
										: 'Enable workspace sync'}</Button
								>
							{:else if syncStatus.phase === 'paused'}<Button
									disabled={busy ||
										syncChanging ||
										joining ||
										importingRecovery}
									onclick={() => changeWorkspaceSync('resume')}
									>{syncChanging ? 'Resuming…' : 'Resume sync'}</Button
								>
							{:else}<Button
									variant="outline"
									disabled={busy ||
										syncChanging ||
										joining ||
										importingRecovery}
									onclick={() => changeWorkspaceSync('pause')}
									>{syncChanging ? 'Pausing…' : 'Pause sync'}</Button
								>{/if}
						</div>
					{/if}
					{#if syncError}<p role="alert" class="text-sm text-destructive">
							{syncError}
						</p>{/if}
				{/if}
			</div>
			{#if workspace.state?.rootPath && statusRoot === workspace.state.rootPath && syncConfigured}
				<div class="flex flex-col gap-3" aria-labelledby="sync-devices-heading">
					<h3 id="sync-devices-heading" class="text-sm font-medium">
						Workspace devices
					</h3>
					<p class="text-xs text-muted-foreground">
						Compare each fingerprint with the fingerprint shown on the actual
						other device. The server’s device list alone does not establish
						trust. Both devices must approve each other to exchange keys.
					</p>
					<div>
						<Button
							variant="outline"
							disabled={busy ||
								syncChanging ||
								devicesBusy ||
								joining ||
								importingRecovery}
							onclick={reviewDevices}
							>{devicesBusy ? 'Updating devices…' : 'Review devices'}</Button
						>
					</div>
					{#if devicesRoot === workspace.state.rootPath && devicesAccount === account.deviceId}
						{#if devicesError}<p role="alert" class="text-sm text-destructive">
								{devicesError}
							</p>{/if}
						{#if devices.length > 0}
							<ul class="flex flex-col gap-3">
								{#each devices as device (device.deviceId)}
									<li class="flex flex-col gap-3 rounded-xl border p-4">
										<div
											class="flex flex-wrap items-center justify-between gap-2"
										>
											<p class="break-all text-xs">Device: {device.deviceId}</p>
											{#if device.deviceId === account.deviceId}<Badge
													variant="secondary">This device</Badge
												>{:else if device.approved}<Badge variant="secondary"
													>Approved</Badge
												>{/if}
										</div>
										<p class="break-all font-mono text-xs select-text">
											{device.fingerprint}
										</p>
										{#if device.deviceId !== account.deviceId && !device.approved}
											<Field.FieldGroup
												><Field.Field orientation="horizontal">
													<Checkbox
														id={`verify-device-${device.deviceId}`}
														checked={verified[device.deviceId] ===
															device.fingerprint}
														onCheckedChange={(checked) => {
															verified[device.deviceId] = checked
																? device.fingerprint
																: '';
														}}
														disabled={devicesBusy || busy || importingRecovery}
													/>
													<Field.FieldLabel
														for={`verify-device-${device.deviceId}`}
														>I compared the full fingerprint on the other device
														and it matches.</Field.FieldLabel
													>
												</Field.Field></Field.FieldGroup
											>
											<div>
												<Button
													disabled={devicesBusy ||
														busy ||
														verified[device.deviceId] !== device.fingerprint}
													onclick={() => approveDevice(device)}
													>Approve device</Button
												>
											</div>
										{/if}
									</li>
								{/each}
							</ul>
						{:else if !devicesError && !devicesBusy}<p
								class="text-xs text-muted-foreground"
							>
								No workspace devices were returned.
							</p>{/if}
					{/if}
				</div>
			{/if}
			{#if workspace.state?.rootPath && statusRoot === workspace.state.rootPath && syncConfigured}
				<div
					class="flex flex-col gap-3"
					aria-labelledby="sync-invitations-heading"
				>
					<h3 id="sync-invitations-heading" class="text-sm font-medium">
						Workspace invitations
					</h3>
					<p class="text-xs text-muted-foreground">
						Workspace owners can invite another account. After acceptance,
						compare every device fingerprint. Approving the final fingerprint
						automatically rotates keys and prepares fresh encrypted checkpoints.
					</p>
					<Field.Field>
						<Field.FieldLabel for="invitation-role"
							>Invited role</Field.FieldLabel
						>
						<Select.Root
							type="single"
							bind:value={invitationRole}
							disabled={invitationsBusy || busy || importingRecovery}
						>
							<Select.Trigger id="invitation-role"
								>{invitationRole}</Select.Trigger
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
							disabled={invitationsBusy ||
								busy ||
								importingRecovery ||
								joining ||
								syncChanging}
							onclick={createInvitation}>Create invite link</Button
						>
						<Button
							variant="outline"
							disabled={invitationsBusy ||
								busy ||
								importingRecovery ||
								joining ||
								syncChanging}
							onclick={reviewInvitations}
							>{invitationsBusy
								? 'Updating invitations…'
								: 'Review invitations'}</Button
						>
					</div>
					{#if invitationsRoot === workspace.state.rootPath && invitationsAccount === account.deviceId}
						{#if newInvitation}
							<p class="break-all font-mono text-xs select-text">
								{newInvitation.inviteUrl}
							</p>
							<p class="text-xs text-muted-foreground">
								Share this link with the intended recipient. Expires {new Date(
									newInvitation.expiresAt,
								).toLocaleString()}.
							</p>
						{/if}
						<ul class="flex flex-col gap-3">
							{#each invitations as invitation (invitation.id)}
								<li class="flex flex-col gap-3 rounded-xl border p-4">
									<div
										class="flex flex-wrap items-center justify-between gap-2"
									>
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
											<p class="break-all text-xs">Device: {device.deviceId}</p>
											<p class="break-all font-mono text-xs select-text">
												{device.fingerprint}
											</p>
											{#if device.approved}<Badge variant="secondary"
													>Fingerprint approved</Badge
												>
											{:else}
												<Field.Field orientation="horizontal">
													<Checkbox
														id={`invite-${verificationKey}`}
														checked={inviteVerified[verificationKey] ===
															device.fingerprint}
														onCheckedChange={(checked) => {
															inviteVerified[verificationKey] = checked
																? device.fingerprint
																: '';
														}}
														disabled={invitationsBusy ||
															busy ||
															importingRecovery}
													/>
													<Field.FieldLabel for={`invite-${verificationKey}`}
														>I compared the full fingerprint with the
														recipient’s device and it matches.</Field.FieldLabel
													>
												</Field.Field>
												<Button
													variant="outline"
													disabled={invitationsBusy ||
														busy ||
														importingRecovery ||
														inviteVerified[verificationKey] !==
															device.fingerprint}
													onclick={() =>
														approveInvitationDevice(invitation, device)}
													>Approve fingerprint</Button
												>
											{/if}
										{/each}
										{#if invitation.devices.length === 0}<p
												class="text-xs text-muted-foreground"
											>
												The recipient must connect a desktop device before you
												can grant access.
											</p>{/if}
										{#if invitationReady(invitation)}
											<Badge variant="secondary">
												{activatingInvitation === invitation.id
													? 'Preparing encrypted access…'
													: 'Fingerprints approved'}
											</Badge>
											{#if activatingInvitation !== invitation.id}
												<Button
													variant="outline"
													disabled={invitationsBusy ||
														busy ||
														importingRecovery}
													onclick={() => retryInvitationActivation(invitation)}
													>Retry access preparation</Button
												>
											{/if}
										{/if}
									{/if}
									{#if invitation.status === 'pending' || invitation.status === 'accepted'}<Button
											variant="outline"
											disabled={invitationsBusy || busy || importingRecovery}
											onclick={() => revokeInvitation(invitation)}
											>Revoke invitation</Button
										>{/if}
								</li>
							{/each}
						</ul>
					{/if}
					{#if invitationsError}<p
							role="alert"
							class="text-sm text-destructive"
						>
							{invitationsError}
						</p>{/if}
				</div>
			{/if}
			<div class="flex flex-col gap-2">
				<div class="flex flex-wrap gap-2">
					<Button
						variant="outline"
						disabled={busy ||
							exportingRecovery ||
							importingRecovery ||
							joining ||
							syncChanging ||
							!workspace.state?.rootPath ||
							statusRoot !== workspace.state.rootPath ||
							!syncConfigured}
						onclick={exportRecoveryKit}
						>{exportingRecovery
							? 'Saving recovery kit…'
							: 'Export recovery kit'}</Button
					>
					<Button
						variant="outline"
						disabled={busy ||
							exportingRecovery ||
							importingRecovery ||
							joining ||
							syncChanging ||
							!workspace.state?.rootPath ||
							statusRoot !== workspace.state.rootPath ||
							!syncConfigured}
						onclick={importRecoveryKit}
						>{importingRecovery
							? 'Importing recovery kit…'
							: 'Import recovery kit'}</Button
					>
				</div>
				<p class="text-xs text-muted-foreground">
					The recovery kit is a secret file. Store it securely outside your
					workspace. To import, sign in and join or open the matching workspace
					first. Import restores workspace keys and files, and leaves sync
					paused for device verification.
				</p>
				{#if !workspace.state?.rootPath || statusRoot !== workspace.state.rootPath || !syncConfigured}<p
						class="text-xs text-muted-foreground"
					>
						Open a workspace configured for synchronization to use recovery
						kits.
					</p>{/if}
			</div>
			<div>
				<Button
					variant="outline"
					disabled={busy ||
						exportingRecovery ||
						importingRecovery ||
						syncChanging ||
						devicesBusy ||
						joining ||
						remoteBusy}
					onclick={disconnect}
					>{busy ? 'Disconnecting…' : 'Disconnect this device'}</Button
				>
			</div>
		{:else if request}
			<p class="text-sm">
				Approve this code in your browser to connect the device:
			</p>
			<p
				class="rounded-xl border p-4 text-center font-mono text-xl tracking-widest"
				aria-label="Device sign-in code"
			>
				{request.userCode}
			</p>
			<div class="flex flex-wrap gap-2">
				<Button disabled={busy || opening} onclick={openBrowser}
					>{opening ? 'Opening…' : 'Open sign-in in browser'}</Button
				><Button variant="outline" disabled={busy} onclick={() => cancel()}
					>Cancel sign-in</Button
				>
			</div>
			<p class="text-xs text-muted-foreground">
				If the browser does not open, copy this address into your browser and
				enter the code above.
			</p>
			<p class="break-all text-xs select-text">{request.verificationUri}</p>
			<p role="status" class="text-xs text-muted-foreground">
				Waiting for your approval. Keep Settings open until sign-in finishes.
			</p>
		{:else}
			<form
				onsubmit={(event) => {
					event.preventDefault();
					void begin();
				}}
			>
				<Field.FieldGroup>
					<Field.Field
						><Field.FieldLabel for="sync-server-origin"
							>Server address</Field.FieldLabel
						><Input
							id="sync-server-origin"
							type="url"
							placeholder="https://…"
							autocomplete="off"
							required
							bind:value={origin}
							disabled={busy}
						/><Field.FieldDescription
							>Use the server address supplied by your operator, or your
							self-hosted server.</Field.FieldDescription
						></Field.Field
					>
					<div>
						<Button type="submit" disabled={busy}
							>{busy ? 'Starting sign-in…' : 'Sign in to server'}</Button
						>
					</div>
				</Field.FieldGroup>
			</form>
		{/if}
		{#if notice}<p role="status" class="text-xs text-muted-foreground">
				{notice}
			</p>{/if}
		{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
	</div>
</section>
