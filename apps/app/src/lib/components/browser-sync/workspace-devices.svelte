<script lang="ts">
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import { Spinner } from '$lib/components/ui/spinner';
	import * as Field from '$lib/components/ui/field';
	import type { BrowserSyncDeviceCard } from '$lib/browser-sync';
	import { errorText } from './copy';
	import type { BrowserSyncPanel } from './panel.svelte';

	let { panel }: { panel: BrowserSyncPanel } = $props();

	let devices = $state<BrowserSyncDeviceCard[]>([]);
	let loaded = $state(false);
	let busy = $state(false);
	let error = $state('');
	let notice = $state('');
	/** Security codes you confirmed, by device id. */
	let verified = $state<Record<string, string>>({});

	function scope(): { workspaceId?: string } {
		return panel.workspaceId ? { workspaceId: panel.workspaceId } : {};
	}

	async function load(): Promise<void> {
		const controller = panel.controller;
		if (!controller) return;
		const result = await controller.listWorkspaceDevices(scope());
		if (!result.ok) {
			devices = [];
			error = result.message;
			return;
		}
		devices = result.value;
		// Forget confirmations for devices that left or whose code changed.
		const stillVerified: Record<string, string> = {};
		for (const device of devices) {
			if (verified[device.deviceId] === device.fingerprint)
				stillVerified[device.deviceId] = device.fingerprint;
		}
		verified = stillVerified;
	}

	async function withBusy(task: () => Promise<void>, fallback: string) {
		if (busy) return;
		busy = true;
		error = '';
		notice = '';
		try {
			await task();
		} catch (cause) {
			error = errorText(cause, fallback);
		} finally {
			busy = false;
		}
	}

	function check() {
		void withBusy(async () => {
			try {
				await load();
			} finally {
				loaded = true;
			}
		}, "Your devices couldn't be loaded.");
	}

	function approve(device: BrowserSyncDeviceCard) {
		const controller = panel.controller;
		if (!controller) return;
		void withBusy(async () => {
			// Pass the code exactly as you saw and confirmed it.
			const result = await controller.approveDevice(
				device.deviceId,
				device.fingerprint,
			);
			if (!result.ok) {
				error = result.message;
				return;
			}
			await load();
			notice = 'Approved. This browser now trusts changes from that device.';
		}, "The device couldn't be approved.");
	}

	function revoke(device: BrowserSyncDeviceCard) {
		const controller = panel.controller;
		if (!controller) return;
		void withBusy(async () => {
			const result = await controller.revokeDeviceApproval(device.deviceId);
			if (!result.ok) {
				error = result.message;
				return;
			}
			await load();
			notice = result.value
				? 'Removed the approval on this browser.'
				: "That device wasn't approved on this browser.";
		}, "The approval couldn't be removed.");
	}
</script>

<div class="flex flex-col gap-3" aria-labelledby="browser-sync-devices-heading">
	<h4 id="browser-sync-devices-heading" class="text-sm font-medium">Devices</h4>
	<p class="text-xs text-muted-foreground">
		Approve a device only after its security code matches the one on that
		device.
	</p>
	<div>
		<Button variant="outline" disabled={panel.busy || busy} onclick={check}>
			{#if busy && !loaded}
				<Spinner data-icon="inline-start" />Checking…
			{:else}
				Check devices
			{/if}
		</Button>
	</div>
	{#if notice}<p role="status" class="text-sm">{notice}</p>{/if}
	{#if error}<p role="alert" class="text-sm text-destructive">{error}</p>{/if}
	{#if loaded}
		{#if devices.length > 0}
			<ul class="flex flex-col gap-3">
				{#each devices as device (device.deviceId)}
					<li class="flex flex-col gap-3 rounded-xl border p-4">
						<div class="flex flex-wrap items-center justify-between gap-2">
							<div class="flex flex-col gap-1">
								<p class="text-xs font-medium">Device ID</p>
								<p class="font-mono text-xs break-all select-text">
									{device.deviceId}
								</p>
							</div>
							{#if device.approved}
								<Badge variant="secondary">Approved</Badge>
							{:else}
								<Badge variant="outline">Not approved</Badge>
							{/if}
						</div>
						<div class="flex flex-col gap-1">
							<p class="text-xs font-medium">Security code</p>
							<p class="font-mono text-xs break-all select-text">
								{device.fingerprint}
							</p>
						</div>
						{#if device.approved}
							<div>
								<Button
									variant="outline"
									disabled={panel.busy || busy}
									onclick={() => revoke(device)}>Remove approval</Button
								>
							</div>
						{:else}
							<Field.Group>
								<Field.Field orientation="horizontal">
									<Checkbox
										id={`browser-sync-verify-${device.deviceId}`}
										checked={verified[device.deviceId] === device.fingerprint}
										onCheckedChange={(checked) => {
											verified[device.deviceId] = checked
												? device.fingerprint
												: '';
										}}
										disabled={panel.busy || busy}
									/>
									<Field.Label for={`browser-sync-verify-${device.deviceId}`}
										>The security code matches the other device.</Field.Label
									>
								</Field.Field>
							</Field.Group>
							<div>
								<Button
									disabled={panel.busy ||
										busy ||
										verified[device.deviceId] !== device.fingerprint}
									onclick={() => approve(device)}>Approve</Button
								>
							</div>
						{/if}
					</li>
				{/each}
			</ul>
		{:else}
			<p role="status" class="text-xs text-muted-foreground">
				No devices found.
			</p>
		{/if}
	{/if}
</div>
