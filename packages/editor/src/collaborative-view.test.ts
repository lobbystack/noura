import { expect, test } from 'bun:test';
import { GlobalRegistrator } from '@happy-dom/global-registrator';
import * as Y from 'yjs';
import {
	CollaborationSession,
	encodeCollaborationUpdate,
	type CollaborationEvent,
} from './collaboration';
import { createCollaborativeView } from './collaborative-view';

test('two views share updates, local undo excludes native edits, presence renders safely', async () => {
	if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register();
	const doc = new Y.Doc();
	doc.getText('content').insert(0, 'Hello');
	let receive: (event: CollaborationEvent) => void = () => {};
	const session = new CollaborationSession(
		{
			objectId: 'o',
			generation: '1',
			sessionId: 's',
			revision: 'r',
			readOnly: false,
			update: encodeCollaborationUpdate(Y.encodeStateAsUpdate(doc)),
		},
		{
			subscribe(handler) {
				receive = handler;
				return () => {};
			},
			submit: async () => ({ revision: 'r2' }),
		},
	);
	const parent = document.createElement('div');
	document.body.append(parent);
	const first = createCollaborativeView(parent, session);
	const second = createCollaborativeView(parent, session, 'markdown');
	first.dispatch({ changes: { from: 0, insert: 'A' } });
	expect(second.state.doc.toString()).toBe('AHello');
	doc.getText('content').insert(5, '!');
	receive({
		type: 'update',
		generation: '1',
		update: encodeCollaborationUpdate(Y.encodeStateAsUpdate(doc)),
	});
	session.undoManager.undo();
	expect(first.state.doc.toString()).toBe('Hello!');
	expect(second.state.doc.toString()).toBe('Hello!');
	const relative = encodeCollaborationUpdate(
		Y.encodeRelativePosition(
			Y.createRelativePositionFromTypeIndex(session.text, 2),
		),
	);
	receive({
		type: 'presence',
		generation: '1',
		presence: [
			{
				deviceId: 'd',
				name: '<script>Member</script>',
				color: '#123456',
				anchor: relative,
				head: relative,
			},
		],
	});
	await Promise.resolve();
	expect(parent.textContent).toContain('<script>Member</script>');
	expect(parent.querySelector('script')).toBeNull();
	first.destroy();
	second.destroy();
	await session.close();
	parent.remove();
});
