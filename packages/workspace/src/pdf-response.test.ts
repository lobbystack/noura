import { expect, test } from 'bun:test';
import { decodePdfResponse } from './pdf-response';
test('decodes native binary and fallback array range responses without copying buffers', () => {
	const buffer = new Uint8Array([0, 255, 37]).buffer;
	expect(decodePdfResponse(buffer, 3).buffer).toBe(buffer);
	expect(decodePdfResponse([0, 255, 37], 3)).toEqual(new Uint8Array(buffer));
});
test('rejects incomplete or oversized range responses', () => {
	expect(() => decodePdfResponse(new ArrayBuffer(2), 3)).toThrow();
	expect(() => decodePdfResponse([1, 2, 3], 2)).toThrow();
});
