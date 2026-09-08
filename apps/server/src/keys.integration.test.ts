import { describe, expect, test } from 'bun:test';
import { randomBytes } from 'node:crypto';
import { createApp } from './app';
import { SyncStore } from './store';
import { digest } from './protocol';
import { BlobService } from './blobs';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { testS3Client } from './test-storage';
import {
	acceptInvitation,
	createInvitation,
	listInvitations,
} from './invitations';

describe.skipIf(
	!process.env.NOURA_TEST_DATABASE_URL || !process.env.NOURA_NATIVE_KEYS_PROBE,
)('native signed key exchange', () => {
	test('approved age recipients receive keys and apply files; an unpinned server signer is rejected', async () => {
		const store = new SyncStore(process.env.NOURA_TEST_DATABASE_URL!);
		await store.migrate();
		// This accelerated multi-device scenario performs many sync passes in seconds.
		// Rate limiting is exercised separately; keep this test focused on key authorization.
		store.rateLimit = async () => {};
		const directory = await mkdtemp(join(tmpdir(), 'noura-native-blobs-'));
		const app = createApp(store, {
			origin: 'http://127.0.0.1:1900',
			blobs: await BlobService.open(store, directory, testS3Client()),
			checkpointTransitions: true,
		});
		const server = Bun.serve({
			hostname: '127.0.0.1',
			port: 0,
			fetch: app.fetch,
		});
		const ownerAccount = `owner_${crypto.randomUUID()}`;
		const readerAccount = `reader_${crypto.randomUUID()}`;
		const editorAccount = `editor_${crypto.randomUUID()}`;
		const viewerAccount = `viewer_${crypto.randomUUID()}`;
		const editorToken = randomBytes(32).toString('base64url');
		const viewerToken = randomBytes(32).toString('base64url');
		const token = randomBytes(32).toString('base64url');
		const ownerToken = randomBytes(32).toString('base64url');
		const peerToken = randomBytes(32).toString('base64url');
		const child = Bun.spawn([process.env.NOURA_NATIVE_KEYS_PROBE!], {
			stdin: 'pipe',
			stdout: 'pipe',
			stderr: 'pipe',
		});
		let pending = '';
		const stream = child.stdout.getReader();
		const decoder = new TextDecoder();
		async function line() {
			while (!pending.includes('\n')) {
				const result = await stream.read();
				if (result.done) {
					const stderr = await new Response(child.stderr).text();
					throw new Error(
						`Native key probe ended before its response${stderr ? `: ${stderr.trim()}` : ''}`,
					);
				}
				pending += decoder.decode(result.value, { stream: true });
			}
			const end = pending.indexOf('\n');
			const value = pending.slice(0, end);
			pending = pending.slice(end + 1);
			return JSON.parse(value);
		}
		try {
			child.stdin.write(
				JSON.stringify({
					origin: `http://127.0.0.1:${server.port}`,
					ownerAccount,
					readerAccount,
					editorAccount,
					viewerAccount,
					editorToken,
					viewerToken,
					token,
					ownerToken,
					peerToken,
				}) + '\n',
			);
			await child.stdin.flush();
			const fixture = await line();
			expect(JSON.stringify(fixture)).not.toContain('private shared content');
			await store.transaction(async (tx) => {
				for (const [device, account, session] of [
					[fixture.owner, ownerAccount, ownerToken],
					[fixture.reader, readerAccount, token],
					[fixture.editor, editorAccount, editorToken],
					[fixture.viewer, viewerAccount, viewerToken],
				] as const) {
					await tx`INSERT INTO noura_devices(id,account_id,public_key,encryption_recipient) VALUES(${device.deviceId},${account},${device.publicKey},${device.recipient})`;
					await tx`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(session)},${device.deviceId},now()+interval '1 hour')`;
				}
			});
			const actor = await store.authenticate(ownerToken);
			await store.createWorkspace(actor, fixture.workspaceId);
			await store.createObject(actor, fixture.workspaceId, 'object');
			const response = await app.request(
				`/v1/workspaces/${fixture.workspaceId}/access`,
				{
					method: 'PUT',
					headers: {
						Authorization: `Bearer ${ownerToken}`,
						'Content-Type': 'application/json',
					},
					body: JSON.stringify(fixture.policy),
				},
			);
			expect(response.status).toBe(200);
			await store.db`INSERT INTO noura_devices(id,account_id,public_key,encryption_recipient) VALUES(${fixture.peer.deviceId},${ownerAccount},${fixture.peer.publicKey},${fixture.peer.recipient})`;
			await store.db`INSERT INTO noura_sessions(token_hash,device_id,expires_at) VALUES(${digest(peerToken)},${fixture.peer.deviceId},now()+interval '1 hour')`;
			await store.push(actor, fixture.workspaceId, [fixture.operation]);
			const invitation = await createInvitation(
				store,
				actor,
				fixture.workspaceId,
				'viewer',
			);
			await acceptInvitation(store, invitation.token, readerAccount);
			child.stdin.write(
				JSON.stringify({ ready: true, invitationId: invitation.id }) + '\n',
			);
			await child.stdin.end();
			expect(await line()).toEqual({
				decrypted: true,
				reloaded: true,
				rejectedUntrustedSigner: true,
				approvedCoordinatorExchange: true,
				resolvedConflict: true,
				attachmentExchange: true,
				memberReplicas: true,
				recoveryKit: true,
				invitationFinalized: true,
			});
			expect(
				(await listInvitations(store, actor, fixture.workspaceId))
					.invitations[0],
			).toMatchObject({ status: 'completed' });
			expect(await child.exited).toBe(0);
			expect(await new Response(child.stderr).text()).toBe('');
		} finally {
			child.kill();
			await server.stop(true);
			await store.close();
			await rm(directory, { recursive: true, force: true });
		}
	}, 60_000);
});
