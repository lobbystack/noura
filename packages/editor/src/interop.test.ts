import { expect, test } from 'bun:test';
import * as Y from 'yjs';
import fixture from '../../../docs/workspace-format/fixtures/yrs-text-v1.json';

const bytes = (value: string) => new Uint8Array(Buffer.from(value, 'base64'));
test('Yjs reads Yrs update-v1, UTF-16 edits, deletion and state vectors', () => {
	const doc = new Y.Doc();
	Y.applyUpdate(doc, bytes(fixture.base));
	expect(doc.getText('content').toString()).toBe(fixture.baseText);
	expect(Buffer.from(Y.encodeStateVector(doc)).toString('base64')).toBe(
		fixture.baseVector,
	);
	Y.applyUpdate(doc, bytes(fixture.delta));
	Y.applyUpdate(doc, bytes(fixture.delta));
	expect(doc.getText('content').toString()).toBe(fixture.text);
	const restored = new Y.Doc();
	Y.applyUpdate(restored, bytes(fixture.base));
	Y.applyUpdate(
		restored,
		Y.encodeStateAsUpdate(doc, bytes(fixture.baseVector)),
	);
	expect(restored.getText('content').toString()).toBe(fixture.text);
	doc.destroy();
	restored.destroy();
});
