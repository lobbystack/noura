import postgres from 'postgres';
import type {
	EncryptedOperation,
	SequencedOperation,
	SyncPage,
} from '../../../packages/shared/src/sync';
import type { EncryptedPresence } from '../../../packages/shared/src/generated/EncryptedPresence';
import {
	digest,
	operationDigest,
	SyncError,
	verifyOperation,
} from './protocol';
import { schema } from './schema';

type Db = ReturnType<typeof postgres>;
type Tx = postgres.TransactionSql;
export interface Actor {
	deviceId: string;
	accountId: string;
	publicKey: string;
}

export class SyncStore {
	readonly db: Db;
	private listening: Promise<postgres.ListenMeta> | undefined;
	private presenceListening: Promise<postgres.ListenMeta> | undefined;
	private watchers = new Map<string, Set<() => void>>();
	private presenceWatchers = new Map<string, Set<(payload: string) => void>>();
	private activeWatches = 0;
	get subscriptionCount() {
		return this.activeWatches;
	}
	constructor(url: string) {
		this.db = postgres(url, { max: 10, onnotice: () => {} });
	}
	async transaction<T>(run: (tx: Tx) => T | Promise<T>): Promise<T> {
		// postgres.js 3.4.9 can skip its BEGIN reservation hook at a pipeline
		// boundary. Reserving first keeps every transaction on an exclusively
		// owned physical connection even if that upstream race is triggered.
		const connection = await this.db.reserve();
		try {
			await connection.unsafe('BEGIN');
			try {
				const result = await run(connection as unknown as Tx);
				await connection.unsafe('COMMIT');
				return result;
			} catch (error) {
				await connection.unsafe('ROLLBACK').catch(() => {});
				throw error;
			}
		} finally {
			connection.release();
		}
	}
	async migrate() {
		await this.transaction(async (tx) => {
			await tx`SELECT pg_advisory_xact_lock(192837465)`;
			await tx.unsafe(schema);
		});
	}
	async ready() {
		// Resolve the required release columns even when the tables contain no rows.
		await this
			.db`SELECT w.access_revision,d.encryption_recipient,k.signing_device,k.signature,k.construction,k.recipient_public_key,k.ephemeral_public_key,k.salt,k.nonce,a.policy,p.snapshot,
			 s.token_hash,c.expires_at,r.count,m.role,m.history_after,o.epoch,g.role,g.history_after,u.ciphertext,b.tus_info,b.complete,
			 i.accepted_account_id,i.completed_at,i.revoked_at,t.committed,cp.checkpoint,b.transition_id,wc.capability
		 FROM noura_workspaces w
		 LEFT JOIN noura_devices d ON false
		 LEFT JOIN noura_key_envelopes k ON false
		 LEFT JOIN noura_access_log a ON false
		 LEFT JOIN noura_sessions s ON false
		 LEFT JOIN noura_device_challenges c ON false
		 LEFT JOIN noura_rate_limits r ON false
		 LEFT JOIN noura_members m ON false
		 LEFT JOIN noura_objects o ON false
		 LEFT JOIN noura_grants g ON false
		 LEFT JOIN noura_operations u ON false
		 LEFT JOIN noura_public_links p ON false
		 LEFT JOIN noura_invitations i ON false
		 LEFT JOIN noura_transitions t ON false
			 LEFT JOIN noura_checkpoints cp ON false
			 LEFT JOIN noura_workspace_capabilities wc ON false
			 LEFT JOIN noura_blobs b ON false LIMIT 0`;
	}
	async close() {
		for (const callbacks of this.watchers.values()) {
			for (const callback of callbacks) callback();
		}
		await this.listening
			?.then((listener) => listener.unlisten())
			.catch(() => {});
		await this.presenceListening
			?.then((listener) => listener.unlisten())
			.catch(() => {});
		await this.db.end();
	}

	async authorizeRealtimeWorkspace(actor: Actor, workspace: string) {
		await this.withWorkspace(actor, workspace, async (tx, _state, role) => {
			const [grant] = role
				? [undefined]
				: await tx`SELECT 1 FROM noura_grants WHERE workspace_id=${workspace} AND account_id=${actor.accountId} LIMIT 1`;
			if (!role && !grant) throw new SyncError('sync.forbidden', 403);
		});
	}

	async publishPresence(actor: Actor, value: EncryptedPresence) {
		const payload = JSON.stringify({
			type: 'presence',
			presence: value,
			expiresAt: Date.now() + 30_000,
		});
		if (Buffer.byteLength(payload) >= 8_000)
			throw new SyncError('sync.invalid_presence');
		await this.withWorkspace(
			actor,
			value.workspaceId,
			async (tx, _state, role) => {
				const [object] =
					await tx`SELECT epoch,generation FROM noura_objects WHERE workspace_id=${value.workspaceId} AND id=${value.objectId}`;
				const [grant] = role
					? [undefined]
					: await tx`SELECT role FROM noura_grants WHERE workspace_id=${value.workspaceId} AND object_id=${value.objectId} AND account_id=${actor.accountId}`;
				if (!object || (!role && !grant))
					throw new SyncError('sync.forbidden', 403);
				if (
					Number(object.epoch) !== value.epoch ||
					object.generation !== value.generation
				)
					throw new SyncError('sync.generation_changed', 409);
				await tx`SELECT pg_notify('noura_presence',${payload})`;
			},
		);
	}

	async publishPresenceDeparture(
		workspace: string,
		deviceId: string,
		sessionId: string,
	) {
		const payload = JSON.stringify({
			type: 'presence-left',
			workspaceId: workspace,
			deviceId,
			sessionId,
		});
		await this.db`SELECT pg_notify('noura_presence',${payload})`;
	}

	async watchPresence(
		workspace: string,
		signal: AbortSignal,
		handler: (payload: string) => void,
	) {
		this.presenceListening ??= this.db
			.listen('noura_presence', (payload) => {
				let target: unknown;
				try {
					target = JSON.parse(payload);
				} catch {
					return;
				}
				if (
					!target ||
					typeof target !== 'object' ||
					!('workspaceId' in target)
				) {
					if (
						!target ||
						typeof target !== 'object' ||
						!('presence' in target) ||
						!target.presence ||
						typeof target.presence !== 'object' ||
						!('workspaceId' in target.presence)
					)
						return;
					target = { ...target, workspaceId: target.presence.workspaceId };
				}
				const id = (target as { workspaceId: unknown }).workspaceId;
				if (typeof id !== 'string') return;
				for (const callback of this.presenceWatchers.get(id) ?? [])
					callback(payload);
			})
			.catch((error: unknown) => {
				this.presenceListening = undefined;
				throw error;
			});
		await this.presenceListening;
		if (this.activeWatches >= 1000) throw new SyncError('sync.busy', 503);
		this.activeWatches += 1;
		const callbacks =
			this.presenceWatchers.get(workspace) ??
			new Set<(payload: string) => void>();
		callbacks.add(handler);
		this.presenceWatchers.set(workspace, callbacks);
		let closed = false;
		const close = () => {
			if (closed) return;
			closed = true;
			this.activeWatches -= 1;
			signal.removeEventListener('abort', close);
			callbacks.delete(handler);
			if (!callbacks.size) this.presenceWatchers.delete(workspace);
		};
		signal.addEventListener('abort', close, { once: true });
		if (signal.aborted) close();
		return { close };
	}

	/** Keep realtime invalidation subscribed while authorization is revalidated. */
	async watchChanges(
		workspace: string,
		signal: AbortSignal,
		handler: () => void,
	) {
		this.listening ??= this.db
			.listen(
				'noura_sync',
				(id) => {
					for (const callback of this.watchers.get(id) ?? []) callback();
				},
				() => {
					// A reconnect may have missed notifications; every subscriber must pull.
					for (const callbacks of this.watchers.values())
						for (const callback of callbacks) callback();
				},
			)
			.catch((error: unknown) => {
				this.listening = undefined;
				throw error;
			});
		await this.listening;
		if (this.activeWatches >= 1000) throw new SyncError('sync.busy', 503);
		this.activeWatches += 1;
		const callbacks = this.watchers.get(workspace) ?? new Set<() => void>();
		callbacks.add(handler);
		this.watchers.set(workspace, callbacks);
		let closed = false;
		const close = () => {
			if (closed) return;
			closed = true;
			this.activeWatches -= 1;
			signal.removeEventListener('abort', close);
			callbacks.delete(handler);
			if (!callbacks.size) this.watchers.delete(workspace);
		};
		signal.addEventListener('abort', close, { once: true });
		if (signal.aborted) close();
		return { close };
	}

	/** Subscribe before reading a page, so a commit between read and wait cannot be missed. */
	async watch(workspace: string, signal: AbortSignal, milliseconds = 25_000) {
		this.listening ??= this.db
			.listen(
				'noura_sync',
				(id) => {
					for (const callback of this.watchers.get(id) ?? []) callback();
				},
				() => {
					// A reconnect may have missed notifications; every waiter must re-read its cursor.
					for (const callbacks of this.watchers.values())
						for (const callback of callbacks) callback();
				},
			)
			.catch((error: unknown) => {
				this.listening = undefined;
				throw error;
			});
		await this.listening;
		if (this.activeWatches >= 1000) throw new SyncError('sync.busy', 503);
		this.activeWatches += 1;
		let finish!: () => void;
		const changed = new Promise<void>((resolve) => {
			finish = resolve;
		});
		const callbacks = this.watchers.get(workspace) ?? new Set<() => void>();
		callbacks.add(finish);
		this.watchers.set(workspace, callbacks);
		const timer = setTimeout(finish, milliseconds);
		signal.addEventListener('abort', finish, { once: true });
		if (signal.aborted) finish();
		let closed = false;
		return {
			changed,
			close: () => {
				if (closed) return;
				closed = true;
				this.activeWatches -= 1;
				clearTimeout(timer);
				signal.removeEventListener('abort', finish);
				callbacks.delete(finish);
				if (callbacks.size === 0) this.watchers.delete(workspace);
				finish();
			},
		};
	}
	async rateLimit(actor: Actor) {
		const [row] = await this
			.db`INSERT INTO noura_rate_limits(account_id,window_start,count)
		 VALUES(${actor.accountId},floor(extract(epoch FROM now())/60)::bigint,1)
		 ON CONFLICT(account_id) DO UPDATE SET
		 count=CASE WHEN noura_rate_limits.window_start=EXCLUDED.window_start THEN noura_rate_limits.count+1 ELSE 1 END,
		 window_start=EXCLUDED.window_start RETURNING count`;
		if (row!.count > 120) throw new SyncError('sync.rate_limited', 429);
	}
	/** Per-device token buckets shared across server instances; no content or presence is retained. */
	async collaborationRateLimit(
		actor: Actor,
		bucket: 'durable' | 'presence' = 'durable',
	) {
		const rate = bucket === 'durable' ? 20 : 10;
		const burst = bucket === 'durable' ? 40 : 10;
		const allowed = await this.transaction(async (tx) => {
			await tx`INSERT INTO noura_collaboration_limits(device_id,bucket,tokens,updated_at)
			 VALUES(${actor.deviceId},${bucket},${burst},clock_timestamp()) ON CONFLICT DO NOTHING`;
			const [row] =
				await tx`SELECT tokens,updated_at FROM noura_collaboration_limits
			 WHERE device_id=${actor.deviceId} AND bucket=${bucket} FOR UPDATE`;
			const [balance] = await tx`UPDATE noura_collaboration_limits SET
			 tokens=LEAST(${burst},tokens+GREATEST(0,extract(epoch FROM clock_timestamp()-updated_at))*${rate}),
			 updated_at=clock_timestamp() WHERE device_id=${actor.deviceId} AND bucket=${bucket} RETURNING tokens`;
			if (!row || !balance || Number(balance.tokens) < 1) return false;
			await tx`UPDATE noura_collaboration_limits SET tokens=tokens-1 WHERE device_id=${actor.deviceId} AND bucket=${bucket}`;
			return true;
		});
		if (!allowed) throw new SyncError('sync.rate_limited', 429);
	}
	async authenticate(token: string): Promise<Actor> {
		const [row] = await this.db`
		 SELECT d.id, d.account_id, d.public_key FROM noura_sessions s
		 JOIN noura_devices d ON d.id=s.device_id
		 WHERE s.token_hash=${digest(token)} AND s.expires_at>now() AND NOT d.revoked`;
		if (!row) throw new SyncError('sync.unauthorized', 401);
		return {
			deviceId: row.id,
			accountId: row.account_id,
			publicKey: row.public_key,
		};
	}
	private async lock(tx: Tx, workspaceId: string) {
		const [row] =
			await tx`SELECT * FROM noura_workspaces WHERE id=${workspaceId} FOR UPDATE`;
		if (!row) throw new SyncError('sync.not_found', 404);
		return row;
	}
	private async member(
		tx: Tx,
		actor: Actor,
		workspaceId: string,
	): Promise<string | undefined> {
		const [device] =
			await tx`SELECT id FROM noura_devices WHERE id=${actor.deviceId} AND account_id=${actor.accountId} AND public_key=${actor.publicKey} AND NOT revoked FOR SHARE`;
		if (!device) throw new SyncError('sync.unauthorized', 401);
		const [row] =
			await tx`SELECT role FROM noura_members WHERE workspace_id=${workspaceId} AND account_id=${actor.accountId}`;
		return row?.role;
	}
	async createWorkspace(actor: Actor, id: string) {
		await this.transaction(async (tx) => {
			await this.member(tx, actor, id);
			const inserted =
				await tx`INSERT INTO noura_workspaces(id) VALUES(${id}) ON CONFLICT DO NOTHING RETURNING id`;
			if (!inserted.length) {
				await this.lock(tx, id);
				if ((await this.member(tx, actor, id)) !== 'owner')
					throw new SyncError('sync.forbidden', 403);
				return;
			}
			await tx`INSERT INTO noura_members(workspace_id,account_id,role) VALUES(${id},${actor.accountId},'owner')`;
		});
	}
	async withWorkspace<T>(
		actor: Actor,
		workspace: string,
		run: (tx: Tx, state: postgres.Row, role: string | undefined) => Promise<T>,
	): Promise<T> {
		return (await this.transaction(async (tx) => {
			const state = await this.lock(tx, workspace);
			const role = await this.member(tx, actor, workspace);
			return await run(tx, state, role);
		})) as unknown as T;
	}
	async createObject(actor: Actor, workspace: string, id: string) {
		return await this.withWorkspace(
			actor,
			workspace,
			async (tx, _state, role) => {
				const [existing] =
					await tx`SELECT epoch FROM noura_objects WHERE workspace_id=${workspace} AND id=${id}`;
				if (existing) {
					const [grant] =
						await tx`SELECT 1 FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${id} AND account_id=${actor.accountId}`;
					if (!role && !grant) throw new SyncError('sync.forbidden', 403);
					return Number(existing.epoch);
				}
				if (!role || role === 'viewer')
					throw new SyncError('sync.forbidden', 403);
				const [collaboration] =
					await tx`SELECT 1 FROM noura_workspace_capabilities c JOIN noura_access_log a ON a.workspace_id=c.workspace_id WHERE c.workspace_id=${workspace} AND (a.policy->>'version')::integer=2 ORDER BY a.revision DESC LIMIT 1`;
				if (collaboration)
					throw new SyncError('sync.object_activation_required', 409);
				await tx`INSERT INTO noura_objects(workspace_id,id) VALUES(${workspace},${id}) ON CONFLICT DO NOTHING`;
				return 1;
			},
		);
	}
	async push(
		actor: Actor,
		workspace: string,
		ops: EncryptedOperation[],
	): Promise<string[]> {
		for (const op of ops) {
			if (op.workspaceId !== workspace || op.deviceId !== actor.deviceId)
				throw new SyncError('sync.identity_mismatch', 403);
			verifyOperation(op, actor.publicKey);
		}
		return (await this.transaction(async (tx) => {
			const state = await this.lock(tx, workspace);
			const memberRole = await this.member(tx, actor, workspace);
			const sequences: string[] = [];
			let sequence = BigInt(state.sequence);
			let used = BigInt(state.used_bytes);
			for (const op of ops) {
				const [object] =
					await tx`SELECT epoch,generation,document_mode FROM noura_objects WHERE workspace_id=${workspace} AND id=${op.objectId}`;
				const [grant] =
					await tx`SELECT role FROM noura_grants WHERE workspace_id=${workspace} AND object_id=${op.objectId} AND account_id=${actor.accountId}`;
				if (
					!object ||
					(!['owner', 'admin', 'editor'].includes(memberRole ?? '') &&
						grant?.role !== 'editor')
				)
					throw new SyncError('sync.forbidden', 403);
				const hash = operationDigest(op);
				const [existing] =
					await tx`SELECT sequence,digest FROM noura_operations WHERE workspace_id=${workspace} AND operation_id=${op.operationId}`;
				if (existing) {
					if (existing.digest !== hash)
						throw new SyncError('sync.operation_id_reused', 409);
					sequences.push(String(existing.sequence));
					continue;
				}
				if (BigInt(object.epoch) !== BigInt(op.epoch))
					throw new SyncError('sync.stale_epoch', 409);
				if (
					object.generation &&
					(op.version !== 2 || op.generation !== object.generation)
				)
					throw new SyncError('sync.generation_changed', 409);
				if (object.document_mode === 'text' && op.kind === 'file')
					throw new SyncError('sync.whole_file_replacement_forbidden', 409);
				if (!object.generation && op.version !== 1)
					throw new SyncError('sync.checkpoint_required', 409);
				if (BigInt(op.policyRevision) > BigInt(state.access_revision))
					throw new SyncError('sync.stale_policy', 409);
				const bytes = Buffer.byteLength(op.ciphertext, 'base64');
				used += BigInt(bytes);
				if (used > BigInt(state.quota_bytes))
					throw new SyncError('sync.quota_exceeded', 413);
				sequence++;
				await tx`INSERT INTO noura_operations(workspace_id,operation_id,object_id,device_id,sequence,epoch,policy_revision,nonce,ciphertext,signature,digest,payload_bytes,generation,kind)
				 VALUES(${workspace},${op.operationId},${op.objectId},${op.deviceId},${sequence.toString()},${op.epoch},${op.policyRevision},${op.nonce},${op.ciphertext},${op.signature},${hash},${bytes},${op.generation ?? null},${op.kind ?? null})`;
				sequences.push(sequence.toString());
			}
			await tx`UPDATE noura_workspaces SET sequence=${sequence.toString()},used_bytes=${used.toString()} WHERE id=${workspace}`;
			await tx`SELECT pg_notify('noura_sync',${workspace})`;
			return sequences;
		})) as unknown as string[];
	}
	async pull(
		actor: Actor,
		workspace: string,
		after: string,
	): Promise<SyncPage> {
		return (await this.transaction(async (tx) => {
			const state = await this.lock(tx, workspace);
			await this.member(tx, actor, workspace);
			const [membership] =
				await tx`SELECT role,history_after FROM noura_members WHERE workspace_id=${workspace} AND account_id=${actor.accountId}`;
			const [grant] =
				await tx`SELECT 1 FROM noura_grants WHERE workspace_id=${workspace} AND account_id=${actor.accountId} LIMIT 1`;
			if (!membership && !grant) throw new SyncError('sync.forbidden', 403);
			if (BigInt(after) > BigInt(state.sequence))
				throw new SyncError('sync.cursor_ahead', 409);
			const rows = await tx`SELECT o.* FROM noura_operations o
			 WHERE o.workspace_id=${workspace} AND o.sequence>${after}
				 AND ((${Boolean(membership)} AND o.sequence>${membership?.history_after ?? '0'})
				 OR EXISTS(SELECT 1 FROM noura_grants g WHERE g.workspace_id=o.workspace_id AND g.object_id=o.object_id AND g.account_id=${actor.accountId} AND o.sequence>g.history_after))
			 ORDER BY o.sequence LIMIT 101`;
			const hasMore = rows.length > 100;
			const operations: SequencedOperation[] = rows
				.slice(0, 100)
				.map((row) => ({
					version: row.generation ? 2 : 1,
					...(row.generation
						? { generation: row.generation, kind: row.kind }
						: {}),
					workspaceId: workspace,
					operationId: row.operation_id,
					objectId: row.object_id,
					deviceId: row.device_id,
					epoch: Number(row.epoch),
					policyRevision: String(row.policy_revision),
					nonce: row.nonce,
					ciphertext: row.ciphertext,
					signature: row.signature,
					sequence: String(row.sequence),
				}));
			return {
				operations,
				accessRevision: String(state.access_revision),
				hasMore,
				cursor: hasMore
					? operations[operations.length - 1]!.sequence
					: String(state.sequence),
			};
		})) as unknown as SyncPage;
	}
}
