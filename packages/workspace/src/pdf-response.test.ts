import { expect, test } from 'bun:test';
import { decodePdfResponse } from './pdf-response';

test('decodes PDF binary IPC without including the metadata header in the transferred buffer', () => {
	const header = new TextEncoder().encode(
		JSON.stringify({ relativePath: 'étude.pdf', revision: 'r1' }),
	);
	const body = new Uint8Array([37, 80, 68, 70, 45, 0, 255]);
	const output = new Uint8Array(4 + header.length + body.length);
	new DataView(output.buffer).setUint32(0, header.length, true);
	output.set(header, 4);
	output.set(body, 4 + header.length);
	const result = decodePdfResponse(output.buffer);
	expect(result.relativePath).toBe('étude.pdf');
	expect(result.revision).toBe('r1');
	expect(result.bytes).toEqual(body);
	expect(result.bytes.buffer.byteLength).toBe(body.length);
});
test('rejects truncated and invalid PDF envelopes', () => {
	expect(() => decodePdfResponse(new ArrayBuffer(2))).toThrow();
	const data = new Uint8Array([255, 255, 255, 255]);
	expect(() => decodePdfResponse(data.buffer)).toThrow();
	expect(() =>
		decodePdfResponse(new Uint8Array([2, 0, 0, 0, 123, 125]).buffer),
	).toThrow();
});
