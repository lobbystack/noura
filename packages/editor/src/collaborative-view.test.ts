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
			role: 'writer',
			status: 'Synced',
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

test('PDF preview hooks preserve collaborative text and shared undo', async () => {
	if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register();
	const { createPdfPreviewExtensions } = await import('./factory');
	const source = 'Introduction\n\n[Lecture](lecture.pdf#page=7)\n';
	const doc = new Y.Doc();
	doc.getText('content').insert(0, source);
	const session = new CollaborationSession(
		{
			objectId: 'pdf-note',
			generation: '1',
			sessionId: 'pdf-session',
			revision: 'r',
			readOnly: false,
			role: 'writer',
			status: 'Synced',
			update: encodeCollaborationUpdate(Y.encodeStateAsUpdate(doc)),
		},
		{
			subscribe: () => () => {},
			submit: async () => ({ revision: 'r2' }),
		},
	);
	const parent = document.createElement('div');
	document.body.append(parent);
	const view = createCollaborativeView(
		parent,
		session,
		'markdown',
		createPdfPreviewExtensions({
			resolveLink: async () => ({
				kind: 'pdf',
				relativePath: 'lecture.pdf',
				page: 7,
			}),
		}),
	);
	await new Promise((resolve) => setTimeout(resolve, 20));
	expect(parent.querySelector('.cm-pdf-preview')).not.toBeNull();
	expect(session.text.toString()).toBe(source);
	view.dispatch({ changes: { from: 0, insert: 'A' } });
	session.undoManager.undo();
	expect(view.state.doc.toString()).toBe(source);
	view.destroy();
	await session.close();
	doc.destroy();
	parent.remove();
});
