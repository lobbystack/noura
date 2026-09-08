/** Binary IPC stays inside the transport; callers receive only the requested bytes. */
export function decodePdfResponse(
	buffer: ArrayBuffer | number[],
	expectedLength: number,
): Uint8Array {
	const bytes = new Uint8Array(buffer);
	if (bytes.byteLength !== expectedLength)
		throw new Error('Incomplete PDF range response');
	return bytes;
}
