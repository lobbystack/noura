import type {
	DeviceSignInInfo,
	RemoteSyncWorkspace,
	SyncAccount,
	SyncDevice,
	SyncInvitation,
	SyncInvitationLink,
	SyncInvitationRole,
	WorkspaceSyncStatus,
} from '@noura/workspace';
import { getNouraClient, workspace } from '$lib/state.svelte';

export type SyncSettingsSection = 'account' | 'sync' | 'people';

export type RecipientKind = 'browser' | 'desktop';

export const recipientLabels: Record<RecipientKind, string> = {
	browser: 'Browser',
	desktop: 'Desktop',
};

/**
 * Classify a device by its encryption recipient. A browser device publishes an
 * `x25519:` recipient; a desktop device publishes an `age1…` recipient.
 * Unrecognized recipients return `null` instead of guessing.
 */
export function recipientKind(device: SyncDevice): RecipientKind | null {
	const recipient = device.encryptionRecipient;
	if (recipient.startsWith('x25519:')) return 'browser';
	if (recipient.startsWith('age1')) return 'desktop';
	return null;
}

export function invitationReady(invitation: SyncInvitation) {
	return (
		invitation.status === 'accepted' &&
		invitation.devices.length > 0 &&
		invitation.devices.every((device) => device.approved)
	);
}

export const syncErrorMessages: Record<string, string> = {
	sync_connection_changed:
		'Reconnect the original account and device identity to resume this workspace.',
};

export const phaseLabels: Record<WorkspaceSyncStatus['phase'], string> = {
	disabled: 'Not enabled',
	paused: 'Paused',
	idle: 'Waiting for changes',
	syncing: 'Syncing',
	error: 'Needs attention',
};

export const transitionPhaseLabels: Record<
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

export function message(cause: unknown): string {
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

/**
 * State and actions behind the account, sync, and people settings sections.
 * Every request checks that the workspace and account it started with are
 * still current before it writes its result.
 */
export class SyncAccountSettings {
	account = $state<SyncAccount | null>(null);
	request = $state<DeviceSignInInfo | null>(null);
	origin = $state('');
	selfHosted = $state(false);
	configuredOrigin = $state<string | null>(null);
	loading = $state(true);
	busy = $state(false);
	opening = $state(false);
	exportingRecovery = $state(false);
	importingRecovery = $state(false);
	error = $state('');
	notice = $state('');
	syncStatus = $state<WorkspaceSyncStatus | null>(null);
	statusRoot = $state<string | null>(null);
	readonly syncConfigured = $derived(
		this.syncStatus !== null && this.syncStatus.phase !== 'disabled',
	);
	syncError = $state('');
	syncChanging = $state(false);
	devices = $state<SyncDevice[]>([]);
	devicesRoot = $state<string | null>(null);
	devicesAccount = $state('');
	devicesBusy = $state(false);
	devicesError = $state('');
	verified = $state<Record<string, string>>({});
	invitations = $state<SyncInvitation[]>([]);
	invitationsRoot = $state<string | null>(null);
	invitationsAccount = $state('');
	invitationsBusy = $state(false);
	activatingInvitation = $state<string | null>(null);
	invitationsError = $state('');
	invitationRole = $state<SyncInvitationRole>('editor');
	newInvitation = $state<SyncInvitationLink | null>(null);
	inviteVerified = $state<Record<string, string>>({});
	remoteWorkspaces = $state<RemoteSyncWorkspace[]>([]);
	remoteAccount = $state('');
	remoteBusy = $state(false);
	joining = $state(false);
	joinError = $state('');
	joinNotice = $state('');
	selectedRemote = $state('');
	localName = $state('');
	readonly selectedMembership = $derived(
		this.remoteWorkspaces.find((item) => item.id === this.selectedRemote),
	);

	#statusTimer: ReturnType<typeof setTimeout> | undefined;
	#statusGeneration = 0;
	#disposed = false;

	/** Subscribe to the sign-in state and start polling sync status. */
	mount(section: SyncSettingsSection) {
		const signIn = getNouraClient().sync.signIn;
		const unsubscribe = signIn.subscribe((value) => {
			this.account = value.account;
			this.request = value.request;
			this.loading = value.loading;
			this.busy = value.busy;
			this.error = value.error;
			this.notice = value.notice;
			this.configuredOrigin = value.origin;
		});
		void signIn.initialize();
		// The account section shows device state only.
		if (section !== 'account') void this.refreshWorkspaceStatus();
		return () => {
			this.#disposed = true;
			unsubscribe();
			clearTimeout(this.#statusTimer);
		};
	}

	async loadRemoteWorkspaces() {
		const ownId = this.account?.deviceId;
		if (!ownId) return;
		this.remoteBusy = true;
		this.joinError = '';
		this.joinNotice = '';
		this.selectedRemote = '';
		try {
			const values = await getNouraClient().sync.remoteWorkspaces();
			if (!this.#disposed && this.account?.deviceId === ownId) {
				this.remoteWorkspaces = values;
				this.remoteAccount = ownId;
			}
		} catch (cause) {
			if (!this.#disposed && this.account?.deviceId === ownId)
				this.joinError = message(cause);
		} finally {
			if (!this.#disposed) this.remoteBusy = false;
		}
	}

	async joinRemoteWorkspace() {
		const ownId = this.account?.deviceId;
		if (
			!ownId ||
			this.remoteAccount !== ownId ||
			!this.localName.trim() ||
			!this.remoteWorkspaces.some((item) => item.id === this.selectedRemote)
		)
			return;
		this.joining = true;
		this.joinError = '';
		this.joinNotice = '';
		try {
			const joined = await getNouraClient().sync.joinWorkspace(
				this.selectedRemote,
				this.localName.trim(),
			);
			if (joined) {
				await workspace.refresh();
				await workspace.refreshRecents();
				if (!this.#disposed && this.account?.deviceId === ownId) {
					this.selectedRemote = '';
					this.localName = '';
					this.joinNotice =
						'Workspace joined and paused. Compare and approve the full fingerprints on both devices, then choose Resume sync.';
				}
			}
		} catch (cause) {
			if (!this.#disposed && this.account?.deviceId === ownId)
				this.joinError = message(cause);
		} finally {
			if (!this.#disposed) this.joining = false;
		}
	}

	async reviewDevices() {
		const root = workspace.state?.rootPath;
		const deviceId = this.account?.deviceId;
		if (!root || !deviceId) return;
		this.devicesBusy = true;
		this.devicesError = '';
		this.verified = {};
		try {
			const result = await getNouraClient().sync.workspaceDevices();
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === deviceId
			) {
				this.devices = result;
				this.devicesRoot = root;
				this.devicesAccount = deviceId;
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === deviceId
			) {
				this.devicesError = message(cause);
				this.devicesRoot = root;
				this.devicesAccount = deviceId;
				this.devices = [];
			}
		} finally {
			if (!this.#disposed) this.devicesBusy = false;
		}
	}

	/** Record whether the user compared this device's full fingerprint. */
	verifyDevice(device: SyncDevice, checked: boolean) {
		this.verified[device.deviceId] = checked ? device.fingerprint : '';
	}

	async approveDevice(device: SyncDevice) {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		if (
			!root ||
			!ownId ||
			this.devicesRoot !== root ||
			this.devicesAccount !== ownId ||
			device.approved ||
			device.deviceId === ownId ||
			this.verified[device.deviceId] !== device.fingerprint
		)
			return;
		this.devicesBusy = true;
		this.devicesError = '';
		try {
			await getNouraClient().sync.approveWorkspaceDevice(
				device.deviceId,
				device.fingerprint,
			);
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				await this.reviewDevices();
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.devicesError = message(cause);
		} finally {
			if (!this.#disposed) {
				this.devicesBusy = false;
				this.verified = {};
			}
		}
	}

	async reviewInvitations() {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		if (!root || !ownId) return;
		this.invitationsBusy = true;
		this.invitationsError = '';
		this.inviteVerified = {};
		try {
			const result = await getNouraClient().sync.workspaceInvitations();
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			) {
				this.invitations = result;
				this.invitationsRoot = root;
				this.invitationsAccount = ownId;
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			) {
				this.invitationsError = message(cause);
				this.invitationsRoot = root;
				this.invitationsAccount = ownId;
				this.invitations = [];
			}
		} finally {
			if (!this.#disposed) this.invitationsBusy = false;
		}
	}

	async createInvitation() {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		if (!root || !ownId) return;
		this.invitationsBusy = true;
		this.invitationsError = '';
		this.newInvitation = null;
		try {
			const client = getNouraClient().sync;
			const created = await client.createWorkspaceInvitation(
				this.invitationRole,
			);
			if (
				this.#disposed ||
				workspace.state?.rootPath !== root ||
				this.account?.deviceId !== ownId
			)
				return;
			this.newInvitation = created;
			this.invitationsRoot = root;
			this.invitationsAccount = ownId;
			const result = await client.workspaceInvitations();
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			) {
				this.newInvitation = created;
				this.invitations = result;
				this.invitationsRoot = root;
				this.invitationsAccount = ownId;
				this.inviteVerified = {};
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.invitationsError = message(cause);
		} finally {
			if (!this.#disposed) this.invitationsBusy = false;
		}
	}

	/** Record whether the user compared an invited device's fingerprint. */
	verifyInvitationDevice(
		verificationKey: string,
		device: SyncDevice,
		checked: boolean,
	) {
		this.inviteVerified[verificationKey] = checked ? device.fingerprint : '';
	}

	async approveInvitationDevice(
		invitation: SyncInvitation,
		device: SyncDevice,
	) {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		const verificationKey = `${invitation.id}:${device.deviceId}`;
		if (
			!root ||
			!ownId ||
			this.invitationsRoot !== root ||
			this.invitationsAccount !== ownId ||
			device.approved ||
			this.inviteVerified[verificationKey] !== device.fingerprint
		)
			return;
		this.invitationsBusy = true;
		if (
			invitation.devices.every(
				(candidate) =>
					candidate.approved || candidate.deviceId === device.deviceId,
			)
		)
			this.activatingInvitation = invitation.id;
		this.invitationsError = '';
		try {
			const client = getNouraClient().sync;
			await client.approveInvitedDevice(
				invitation.id,
				device.deviceId,
				device.fingerprint,
			);
			const result = await client.workspaceInvitations();
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			) {
				this.invitations = result;
				this.inviteVerified = {};
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.invitationsError = message(cause);
		} finally {
			if (!this.#disposed) {
				this.invitationsBusy = false;
				this.activatingInvitation = null;
			}
		}
	}

	async retryInvitationActivation(invitation: SyncInvitation) {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		if (
			!root ||
			!ownId ||
			this.invitationsRoot !== root ||
			this.invitationsAccount !== ownId ||
			!invitationReady(invitation)
		)
			return;
		this.invitationsBusy = true;
		this.activatingInvitation = invitation.id;
		this.invitationsError = '';
		try {
			const client = getNouraClient().sync;
			await client.finalizeWorkspaceInvitation(invitation.id);
			const result = await client.workspaceInvitations();
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			) {
				this.invitations = result;
				this.inviteVerified = {};
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.invitationsError = message(cause);
		} finally {
			if (!this.#disposed) {
				this.invitationsBusy = false;
				this.activatingInvitation = null;
			}
		}
	}

	async revokeInvitation(invitation: SyncInvitation) {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		if (
			!root ||
			!ownId ||
			this.invitationsRoot !== root ||
			this.invitationsAccount !== ownId ||
			!['pending', 'accepted'].includes(invitation.status)
		)
			return;
		this.invitationsBusy = true;
		this.invitationsError = '';
		try {
			const client = getNouraClient().sync;
			await client.revokeWorkspaceInvitation(invitation.id);
			if (!this.#disposed && this.newInvitation?.id === invitation.id)
				this.newInvitation = null;
			const result = await client.workspaceInvitations();
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			) {
				this.invitations = result;
				this.inviteVerified = {};
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.invitationsError = message(cause);
		} finally {
			if (!this.#disposed) this.invitationsBusy = false;
		}
	}

	async refreshWorkspaceStatus() {
		const root = workspace.state?.rootPath;
		const attempt = this.#statusGeneration;
		try {
			if (!root || this.syncChanging || this.importingRecovery) return;
			const value = await getNouraClient().sync.workspaceStatus();
			if (
				!this.#disposed &&
				attempt === this.#statusGeneration &&
				workspace.state?.rootPath === root
			) {
				this.syncStatus = value;
				this.statusRoot = root;
				this.syncError = '';
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				attempt === this.#statusGeneration &&
				workspace.state?.rootPath === root
			) {
				this.syncStatus = null;
				this.statusRoot = root ?? null;
				this.syncError = message(cause);
			}
		} finally {
			if (!this.#disposed)
				this.#statusTimer = setTimeout(
					() => void this.refreshWorkspaceStatus(),
					5000,
				);
		}
	}

	async changeWorkspaceSync(action: 'enable' | 'pause' | 'resume') {
		const root = workspace.state?.rootPath;
		if (!root) return;
		this.syncChanging = true;
		this.syncError = '';
		this.#statusGeneration += 1;
		try {
			const sync = getNouraClient().sync;
			const value = await (action === 'enable'
				? sync.enableWorkspace()
				: action === 'pause'
					? sync.pauseWorkspace()
					: sync.resumeWorkspace());
			if (!this.#disposed && workspace.state?.rootPath === root) {
				this.syncStatus = value;
				this.statusRoot = root;
			}
		} catch (cause) {
			if (!this.#disposed && workspace.state?.rootPath === root) {
				this.statusRoot = root;
				this.syncError = message(cause);
			}
		} finally {
			if (!this.#disposed) this.syncChanging = false;
		}
	}

	/** Take the status a conflict resolution returned as the current one. */
	conflictsResolved(status: WorkspaceSyncStatus) {
		this.syncStatus = status;
		this.statusRoot = workspace.state?.rootPath ?? null;
		this.#statusGeneration += 1;
		this.syncError = '';
	}

	async begin(signUp = false) {
		await getNouraClient().sync.signIn.begin(
			this.selfHosted ? this.origin.trim() : undefined,
			signUp,
		);
	}

	async openBrowser() {
		this.opening = true;
		await getNouraClient().sync.signIn.openBrowser();
		this.opening = false;
	}

	async cancel() {
		await getNouraClient().sync.signIn.cancel();
	}

	toggleSelfHosted() {
		this.selfHosted = !this.selfHosted;
	}

	async exportRecoveryKit() {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		if (!root || !ownId) return;
		this.exportingRecovery = true;
		this.error = '';
		this.notice = '';
		try {
			const saved = await getNouraClient().sync.exportRecoveryIdentity();
			if (
				!this.#disposed &&
				saved &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.notice =
					'Workspace recovery kit saved. Keep this secret file securely outside your workspace.';
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.error = message(cause);
		} finally {
			if (!this.#disposed) this.exportingRecovery = false;
		}
	}

	async importRecoveryKit() {
		const root = workspace.state?.rootPath;
		const ownId = this.account?.deviceId;
		if (!root || !ownId) return;
		this.importingRecovery = true;
		this.error = '';
		this.notice = '';
		this.#statusGeneration += 1;
		try {
			const restored = await getNouraClient().sync.importRecoveryKit();
			if (restored) {
				await workspace.refresh();
				if (
					this.#disposed ||
					workspace.state?.rootPath !== root ||
					this.account?.deviceId !== ownId
				)
					return;
				const status = await getNouraClient().sync.workspaceStatus();
				if (
					!this.#disposed &&
					workspace.state?.rootPath === root &&
					this.account?.deviceId === ownId
				) {
					this.syncStatus = status;
					this.statusRoot = root;
					this.syncError = '';
					this.devices = [];
					this.devicesRoot = null;
					this.verified = {};
					this.invitations = [];
					this.invitationsRoot = null;
					this.inviteVerified = {};
					this.newInvitation = null;
					this.notice =
						'Workspace recovery kit imported. Sync remains paused. Compare and approve the device fingerprints before resuming.';
				}
			}
		} catch (cause) {
			if (
				!this.#disposed &&
				workspace.state?.rootPath === root &&
				this.account?.deviceId === ownId
			)
				this.error = message(cause);
		} finally {
			if (!this.#disposed) this.importingRecovery = false;
		}
	}

	async disconnect() {
		this.busy = true;
		this.error = '';
		this.notice = '';
		try {
			await getNouraClient().sync.disconnect();
			await getNouraClient().sync.signIn.refresh();
			if (!this.#disposed) {
				this.account = null;
				this.devices = [];
				this.devicesRoot = null;
				this.devicesAccount = '';
				this.remoteWorkspaces = [];
				this.remoteAccount = '';
				this.selectedRemote = '';
				this.localName = '';
				this.joinError = '';
				this.joinNotice = '';
				this.verified = {};
				this.devicesError = '';
				this.invitations = [];
				this.invitationsRoot = null;
				this.invitationsAccount = '';
				this.invitationsError = '';
				this.inviteVerified = {};
				this.newInvitation = null;
				this.notice = 'This device is disconnected from the account.';
			}
		} catch (cause) {
			if (!this.#disposed) this.error = message(cause);
		} finally {
			if (!this.#disposed) this.busy = false;
		}
	}
}
