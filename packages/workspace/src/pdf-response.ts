import type { PdfRead } from './index';

/** The native binary response is a little-endian header length, UTF-8 JSON,
 * then PDF bytes. Binary decoding belongs exclusively to the transport. */
export function decodePdfResponse(buffer: ArrayBuffer): PdfRead {
	if (buffer.byteLength < 4) throw new Error('Invalid PDF response');
	const length = new DataView(buffer).getUint32(0, true);
	if (length > buffer.byteLength - 4) throw new Error('Invalid PDF response');
	const header: unknown = JSON.parse(
		new TextDecoder('utf-8', { fatal: true }).decode(
			new Uint8Array(buffer, 4, length),
		),
	);
	if (
		!header ||
		typeof header !== 'object' ||
		!('relativePath' in header) ||
		typeof header.relativePath !== 'string' ||
		!('revision' in header) ||
		typeof header.revision !== 'string'
	)
		throw new Error('Invalid PDF response');
	// Own just the PDF region: PDF.js transfers its entire underlying buffer.
	return {
		relativePath: header.relativePath,
		revision: header.revision,
		bytes: new Uint8Array(buffer.slice(4 + length)),
	};
}
