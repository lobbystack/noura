import { expect, test } from 'bun:test';
import * as Y from 'yjs';
import {
	CollaborationSession,
	encodeCollaborationUpdate,
	decodeCollaborationUpdate,
	type CollaborationBatch,
	type CollaborationEvent,
} from './collaboration';

function fixture() {
	const doc = new Y.Doc();
	doc.getText('content').insert(0, 'Hello 🌍');
	const bootstrap = {
		objectId: 'object',
		generation: '1',
		sessionId: 'session',
		revision: 'r0',
		readOnly: false,
		role: 'writer' as const,
		status: 'Synced' as const,
		update: encodeCollaborationUpdate(Y.encodeStateAsUpdate(doc)),
	};
	const batches: CollaborationBatch[] = [];
	let receive: (event: CollaborationEvent) => void = () => {};
	let fail = false;
	const session = new CollaborationSession(bootstrap, {
		subscribe(handler) {
			receive = handler;
			return () => {};
		},
		async submit(batch) {
			batches.push(structuredClone(batch));
			if (fail) throw new Error('disk full');
			return { revision: 'r1' };
		},
	});
	return {
		session,
		bootstrap,
		batches,
		receive: (event: CollaborationEvent) => receive(event),
		fail: (value: boolean) => {
			fail = value;
		},
	};
}

test('bootstrap hydrates once and remote changes do not echo or enter local undo', async () => {
	const f = fixture();
	expect(f.session.text.toString()).toBe('Hello 🌍');
	const remote = new Y.Doc();
	Y.applyUpdate(remote, decodeCollaborationUpdate(f.bootstrap.update));
	remote.getText('content').insert(0, 'Remote ');
	f.receive({
		type: 'update',
		generation: '1',
		update: encodeCollaborationUpdate(Y.encodeStateAsUpdate(remote)),
	});
	await f.session.flush();
	expect(f.batches).toHaveLength(0);
	f.session.undoManager.undo();
	expect(f.session.text.toString()).toBe('Remote Hello 🌍');
	await f.session.close();
});

test('batches local edits and flushes immediately with local-only undo', async () => {
	const f = fixture();
	f.session.transact((text) => text.insert(0, 'A'));
	f.session.transact((text) => text.insert(0, 'B'));
	expect(f.batches).toHaveLength(0);
	await f.session.flush();
	expect(f.batches).toHaveLength(1);
	const result = new Y.Doc();
	Y.applyUpdate(result, decodeCollaborationUpdate(f.bootstrap.update));
	Y.applyUpdate(result, decodeCollaborationUpdate(f.batches[0]!.updates[0]!));
	expect(result.getText('content').toString()).toBe('BAHello 🌍');
	expect(f.session.status).toBe('Saved locally');
	f.session.undoManager.undo();
	expect(f.session.text.toString()).toBe('Hello 🌍');
	await f.session.close();
});

test('retry preserves failed batch identity and retains newer edits separately', async () => {
	const f = fixture();
	f.fail(true);
	f.session.transact((text) => text.insert(0, 'A'));
	await expect(f.session.close()).rejects.toThrow('disk full');
	expect(f.session.hasPending).toBe(true);
	f.session.transact((text) => text.insert(0, 'B'));
	f.fail(false);
	await f.session.flush();
	expect(f.batches[0]).toEqual(f.batches[1]);
	expect(f.batches[2]!.batchId).not.toBe(f.batches[0]!.batchId);
	expect(f.session.hasPending).toBe(false);
	await f.session.close();
});

test('concurrent independent clients converge without duplicate initial text', async () => {
	const a = fixture();
	const b = fixture();
	// Both clients must use the exact same native checkpoint, including CRDT identity.
	await b.session.close();
	let batch: CollaborationBatch | undefined;
	const second = new CollaborationSession(a.bootstrap, {
		subscribe: () => () => {},
		submit: async (value) => {
			batch = value;
			return { revision: 'r2' };
		},
	});
	a.session.transact((text) => text.insert(0, 'A'));
	second.transact((text) => text.insert(0, 'B'));
	await Promise.all([a.session.flush(), second.flush()]);
	a.receive({ type: 'update', generation: '1', update: batch!.updates[0]! });
	Y.applyUpdate(
		second.doc,
		decodeCollaborationUpdate(a.batches[0]!.updates[0]!),
	);
	expect(a.session.text.toString()).toBe(second.text.toString());
	expect(a.session.text.toString().match(/Hello/g)).toHaveLength(1);
	await a.session.close();
	await second.close();
});

test('generation mismatch preserves pending draft and blocks old-generation upload', async () => {
	const f = fixture();
	f.session.transact((text) => text.insert(0, 'Draft'));
	f.receive({ type: 'status', generation: '2', status: 'Synced' });
	await expect(f.session.flush()).rejects.toThrow();
	expect(f.session.status).toBe('Needs review');
	expect(f.session.hasPending).toBe(true);
	expect(f.batches).toHaveLength(0);
});

test('automatic batching uses the 100ms window', async () => {
	const f = fixture();
	f.session.transact((text) => text.insert(0, 'A'));
	await new Promise((resolve) => setTimeout(resolve, 160));
	expect(f.batches).toHaveLength(1);
	await f.session.close();
});

test('flushing a clean stale generation does not unlock the editor', async () => {
	const f = fixture();
	f.receive({ type: 'status', generation: '2', status: 'Synced' });
	await f.session.flush();
	expect(f.session.status).toBe('Needs review');
	expect(() => f.session.transact((text) => text.insert(0, 'no'))).toThrow();
	await f.session.close();
});
