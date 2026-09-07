// Test-only fixture consumed by the Rust test suite; run with Bun.
import * as Y from 'yjs';
const doc = new Y.Doc();
doc.clientID = 17;
const text = doc.getText('content');
text.insert(0, 'A😀 e\u0301 中文\r\nremove me');
const encode = (value: Uint8Array) => Buffer.from(value).toString('base64');
const base = encode(Y.encodeStateAsUpdate(doc));
const origin = {};
const undo = new Y.UndoManager(text, { trackedOrigins: new Set([origin]) });
const updates: string[] = [];
doc.on('update', (update: Uint8Array) => updates.push(encode(update)));
doc.transact(() => {
	text.delete(1, 2);
	text.insert(1, '🦀');
}, origin);
const afterEdit = text.toString();
undo.stopCapturing();
text.insert(text.length, ' remote');
const beforeUndo = text.toString();
undo.undo();
const afterUndo = text.toString();
undo.redo();
const afterRedo = text.toString();
console.log(
	JSON.stringify(
		{
			version: 1,
			producer: 'yjs@13.6.32',
			base,
			updates,
			states: [afterEdit, beforeUndo, afterUndo, afterRedo],
			text: text.toString(),
			state: encode(Y.encodeStateAsUpdate(doc)),
			vector: encode(Y.encodeStateVector(doc)),
		},
		null,
		2,
	),
);
undo.destroy();
doc.destroy();
